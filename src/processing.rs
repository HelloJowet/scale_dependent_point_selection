use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use geo_types::Point;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, MAIN_DB, OpenFlags, OptionalExtension, params};

use crate::Tree;

const GEOPACKAGE_APPLICATION_ID: i64 = 0x4750_4b47;
const INITIAL_WEB_MERCATOR_RESOLUTION: f64 = 156_543.033_928_040_97;

#[derive(Clone, Debug)]
/// Configuration for validating and enriching a GeoPackage point layer.
///
/// Higher numeric values in the configured importance column represent more-important points.
pub struct EnrichmentOptions {
    /// Name of the input column containing a unique string or integer ID.
    pub id_property: String,
    /// Name of the input column containing a unique finite numeric importance score.
    pub importance_property: String,
    /// Name of the generated column containing the descending rank, starting at `1`.
    pub rank_property: String,
    /// Name of the generated column containing the isolation distance in metres.
    pub distance_property: String,
    /// Name of the generated column containing the minimum map zoom.
    pub min_zoom_property: String,
    /// Lowest minimum-zoom value that may be generated.
    pub min_zoom: i32,
    /// Highest minimum-zoom value that may be generated.
    pub max_zoom: i32,
    /// Requested on-screen separation between points in pixels.
    pub spacing_pixels: f64,
    /// Maximum distance in metres within which to search for a more-important point.
    pub max_query_distance_metres: f64,
    /// Whether generated columns may replace columns already present in the layer.
    pub overwrite_properties: bool,
}

impl Default for EnrichmentOptions {
    fn default() -> Self {
        Self {
            id_property: "id".to_string(),
            importance_property: "importance".to_string(),
            rank_property: "rank".to_string(),
            distance_property: "distance_metres".to_string(),
            min_zoom_property: "min_zoom".to_string(),
            min_zoom: 0,
            max_zoom: 22,
            spacing_pixels: 16.0,
            max_query_distance_metres: 20_004_000.0,
            overwrite_properties: false,
        }
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
enum FeatureId {
    String(String),
    Integer(i64),
}

struct Feature {
    primary_key: i64,
    point: Point,
    importance: f64,
}

struct ResultRow {
    primary_key: i64,
    rank: i64,
    distance: f64,
    min_zoom: i32,
}

struct Column {
    name: String,
    declared_type: String,
    primary_key: bool,
    hidden: bool,
}

/// Copies a GeoPackage and enriches one EPSG:4326 Point layer in the copy.
///
/// The output is written atomically. Existing output files are replaced only when `overwrite_output` is true.
pub fn enrich_geopackage(input: impl AsRef<Path>, output: impl AsRef<Path>, layer: &str, options: &EnrichmentOptions, overwrite_output: bool) -> Result<()> {
    validate_options(options)?;
    if layer.is_empty() {
        bail!("layer name must not be empty");
    }

    let input = input.as_ref();
    let output = output.as_ref();
    validate_paths(input, output, overwrite_output)?;
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let temporary = tempfile::Builder::new()
        .prefix(".scale-dependent-point-selection-")
        .suffix(".gpkg")
        .tempfile_in(parent)
        .with_context(|| format!("failed to create temporary output in '{}'", parent.display()))?;

    copy_database(input, temporary.path())?;
    {
        let mut connection = Connection::open(temporary.path()).context("failed to open temporary GeoPackage")?;
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .context("failed to configure temporary GeoPackage")?;
        process_layer(&mut connection, layer, options)?;
        connection.close().map_err(|(_, error)| error).context("failed to close temporary GeoPackage")?;
    }
    temporary.as_file().sync_all().context("failed to synchronize temporary GeoPackage")?;

    if overwrite_output {
        temporary
            .persist(output)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to replace output file '{}'", output.display()))?;
    } else {
        temporary
            .persist_noclobber(output)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to create output file '{}'; it may already exist", output.display()))?;
    }
    Ok(())
}

fn validate_paths(input: &Path, output: &Path, overwrite_output: bool) -> Result<()> {
    if !input.is_file() {
        bail!("input file '{}' does not exist or is not a regular file", input.display());
    }
    if output.exists() {
        if same_file::is_same_file(input, output).with_context(|| format!("failed to compare input '{}' and output '{}'", input.display(), output.display()))? {
            bail!("input and output refer to the same physical file");
        }
        if !overwrite_output {
            bail!("output file '{}' already exists; use --force to replace it", output.display());
        }
    }
    Ok(())
}

fn copy_database(input: &Path, output: &Path) -> Result<()> {
    let source = Connection::open_with_flags(input, OpenFlags::SQLITE_OPEN_READ_ONLY).with_context(|| format!("failed to open input GeoPackage '{}'", input.display()))?;
    let application_id: i64 = source
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .context("failed to read GeoPackage application ID")?;
    if application_id != GEOPACKAGE_APPLICATION_ID {
        bail!("input is not a GeoPackage");
    }
    source.backup(MAIN_DB, output, None).context("failed to copy input GeoPackage")
}

fn process_layer(connection: &mut Connection, layer: &str, options: &EnrichmentOptions) -> Result<()> {
    let transaction = connection.transaction().context("failed to start GeoPackage transaction")?;
    let (geometry_column, primary_key) = validate_layer(&transaction, layer, options)?;
    let features = read_features(&transaction, layer, &primary_key, &geometry_column, options)?;
    let results = calculate_results(&features, options)?;
    prepare_output_columns(&transaction, layer, &primary_key, &geometry_column, options)?;
    write_results(&transaction, layer, &primary_key, &results, options)?;
    transaction
        .execute(
            "UPDATE gpkg_contents SET last_change = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE table_name = ?1",
            [layer],
        )
        .context("failed to update layer timestamp")?;
    transaction.commit().context("failed to commit GeoPackage changes")
}

fn validate_layer(connection: &Connection, layer: &str, options: &EnrichmentOptions) -> Result<(String, String)> {
    let metadata: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT gc.column_name, gc.geometry_type_name, gc.srs_id FROM gpkg_geometry_columns gc JOIN gpkg_contents c ON c.table_name = gc.table_name WHERE gc.table_name = ?1 AND lower(c.data_type) = 'features'",
            [layer],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .context("failed to read layer metadata")?;
    let (geometry_column, geometry_type, srs_id) = metadata.ok_or_else(|| anyhow!("feature layer '{layer}' was not found"))?;
    if !geometry_type.eq_ignore_ascii_case("POINT") {
        bail!("layer '{layer}' must be a Point layer");
    }
    if srs_id != 4326 {
        bail!("layer '{layer}' must use EPSG:4326");
    }

    let object_type: Option<String> = connection
        .query_row("SELECT type FROM sqlite_schema WHERE name = ?1", [layer], |row| row.get(0))
        .optional()
        .context("failed to inspect layer table")?;
    if object_type.as_deref() != Some("table") {
        bail!("layer '{layer}' must be stored as a table");
    }

    let columns = table_columns(connection, layer)?;
    let primary_keys: Vec<&Column> = columns.iter().filter(|column| column.primary_key).collect();
    if primary_keys.len() != 1 || !primary_keys[0].declared_type.eq_ignore_ascii_case("INTEGER") || primary_keys[0].hidden {
        bail!("layer '{layer}' must have one INTEGER primary key column");
    }
    for name in [&geometry_column, &options.id_property, &options.importance_property] {
        if !columns.iter().any(|column| column.name == *name && !column.hidden) {
            bail!("layer '{layer}' is missing column '{name}'");
        }
    }
    Ok((geometry_column, primary_keys[0].name.clone()))
}

fn table_columns(connection: &Connection, layer: &str) -> Result<Vec<Column>> {
    let sql = format!("PRAGMA table_xinfo({})", quote_identifier(layer));
    let mut statement = connection.prepare(&sql).context("failed to inspect layer columns")?;
    let rows = statement
        .query_map([], |row| {
            Ok(Column {
                name: row.get(1)?,
                declared_type: row.get(2)?,
                primary_key: row.get::<_, i64>(5)? > 0,
                hidden: row.get::<_, i64>(6)? != 0,
            })
        })
        .context("failed to inspect layer columns")?;
    rows.collect::<rusqlite::Result<Vec<_>>>().context("failed to inspect layer columns")
}

fn read_features(connection: &Connection, layer: &str, primary_key: &str, geometry_column: &str, options: &EnrichmentOptions) -> Result<Vec<Feature>> {
    let sql = format!(
        "SELECT {}, {}, {}, {} FROM {} ORDER BY {}",
        quote_identifier(primary_key),
        quote_identifier(geometry_column),
        quote_identifier(&options.id_property),
        quote_identifier(&options.importance_property),
        quote_identifier(layer),
        quote_identifier(primary_key),
    );
    let mut statement = connection.prepare(&sql).context("failed to read layer")?;
    let mut rows = statement.query([]).context("failed to read layer")?;
    let mut features = Vec::new();
    let mut ids = HashMap::new();
    let mut importance_values = HashMap::new();

    while let Some(row) = rows.next().context("failed to read feature")? {
        let primary_key: i64 = row.get(0).context("feature has an invalid primary key")?;
        let geometry = match row.get_ref(1).context("failed to read geometry")? {
            ValueRef::Blob(value) => value,
            _ => bail!("feature {primary_key} has null or invalid geometry"),
        };
        let point = decode_point(geometry).with_context(|| format!("feature {primary_key} has invalid geometry"))?;
        let id = parse_id(row.get_ref(2).context("failed to read ID")?, primary_key, &options.id_property)?;
        let importance = parse_importance(row.get_ref(3).context("failed to read importance")?, primary_key, &options.importance_property)?;

        if let Some(previous) = ids.insert(id, primary_key) {
            bail!("duplicate ID at feature {primary_key}; it was first used by feature {previous}");
        }
        if let Some(previous) = importance_values.insert(normalized_f64_bits(importance), primary_key) {
            bail!("duplicate or indistinguishable importance at feature {primary_key}; it was first used by feature {previous}");
        }
        features.push(Feature { primary_key, point, importance });
    }
    Ok(features)
}

fn parse_id(value: ValueRef<'_>, primary_key: i64, name: &str) -> Result<FeatureId> {
    match value {
        ValueRef::Integer(value) => Ok(FeatureId::Integer(value)),
        ValueRef::Text(value) => Ok(FeatureId::String(std::str::from_utf8(value).context("ID contains invalid UTF-8")?.to_string())),
        _ => bail!("ID column '{name}' at feature {primary_key} must contain TEXT or INTEGER values"),
    }
}

fn parse_importance(value: ValueRef<'_>, primary_key: i64, name: &str) -> Result<f64> {
    let value = match value {
        ValueRef::Integer(value) => value as f64,
        ValueRef::Real(value) => value,
        _ => bail!("importance column '{name}' at feature {primary_key} must contain INTEGER or REAL values"),
    };
    if !value.is_finite() {
        bail!("importance column '{name}' at feature {primary_key} must contain a finite number");
    }
    Ok(value)
}

fn calculate_results(features: &[Feature], options: &EnrichmentOptions) -> Result<Vec<ResultRow>> {
    let mut tree = Tree::new();
    for feature in features {
        tree.insert_tree_item(&feature.primary_key.to_string(), feature.point, feature.importance);
    }

    let mut ranked_indices: Vec<usize> = (0..features.len()).collect();
    ranked_indices.sort_unstable_by(|left, right| features[*right].importance.partial_cmp(&features[*left].importance).unwrap_or(Ordering::Equal));
    let mut ranks = vec![0_i64; features.len()];
    for (rank, index) in ranked_indices.into_iter().enumerate() {
        ranks[index] = i64::try_from(rank + 1).context("too many features to represent rank")?;
    }

    Ok(features
        .iter()
        .enumerate()
        .map(|(index, feature)| {
            let distance = tree.get_distance_to_nearest_more_important_neighbor(feature.importance, feature.point, options.max_query_distance_metres);
            ResultRow {
                primary_key: feature.primary_key,
                rank: ranks[index],
                distance,
                min_zoom: minimum_zoom(distance, feature.point.y(), options),
            }
        })
        .collect())
}

fn prepare_output_columns(connection: &Connection, layer: &str, primary_key: &str, geometry_column: &str, options: &EnrichmentOptions) -> Result<()> {
    let columns = table_columns(connection, layer)?;
    for (name, declared_type) in [
        (&options.rank_property, "INTEGER"),
        (&options.distance_property, "REAL"),
        (&options.min_zoom_property, "INTEGER"),
    ] {
        if name == primary_key || name == geometry_column {
            bail!("generated column '{name}' conflicts with a protected layer column");
        }
        if let Some(column) = columns.iter().find(|column| column.name == *name) {
            if !options.overwrite_properties {
                bail!("generated column '{name}' already exists; use --overwrite-properties to replace it");
            }
            if column.hidden {
                bail!("generated column '{name}' is not writable");
            }
        } else {
            let sql = format!("ALTER TABLE {} ADD COLUMN {} {declared_type}", quote_identifier(layer), quote_identifier(name));
            connection.execute(&sql, []).with_context(|| format!("failed to add generated column '{name}'"))?;
        }
    }
    Ok(())
}

fn write_results(connection: &Connection, layer: &str, primary_key: &str, results: &[ResultRow], options: &EnrichmentOptions) -> Result<()> {
    let sql = format!(
        "UPDATE {} SET {} = ?1, {} = ?2, {} = ?3 WHERE {} = ?4",
        quote_identifier(layer),
        quote_identifier(&options.rank_property),
        quote_identifier(&options.distance_property),
        quote_identifier(&options.min_zoom_property),
        quote_identifier(primary_key),
    );
    let mut statement = connection.prepare(&sql).context("failed to prepare generated values")?;
    for result in results {
        let changed = statement
            .execute(params![result.rank, result.distance, result.min_zoom, result.primary_key])
            .with_context(|| format!("failed to update feature {}", result.primary_key))?;
        if changed != 1 {
            bail!("feature {} disappeared while it was being updated", result.primary_key);
        }
    }
    Ok(())
}

fn decode_point(bytes: &[u8]) -> Result<Point> {
    if bytes.len() < 13 || &bytes[..2] != b"GP" || bytes[2] != 0 {
        bail!("malformed GeoPackage geometry header");
    }
    let flags = bytes[3];
    if flags & 0b1110_0000 != 0 || flags & 0b0001_0000 != 0 {
        bail!("empty or extended geometries are not supported");
    }
    let header_little_endian = flags & 1 != 0;
    if read_i32(&bytes[4..8], header_little_endian) != 4326 {
        bail!("geometry does not use EPSG:4326");
    }
    let envelope_size = match (flags >> 1) & 0b111 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => bail!("invalid GeoPackage geometry envelope"),
    };
    let offset = 8 + envelope_size;
    if bytes.len() < offset + 21 {
        bail!("truncated Point geometry");
    }

    let little_endian = match bytes[offset] {
        0 => false,
        1 => true,
        _ => bail!("invalid WKB byte order"),
    };
    let geometry_type = read_u32(&bytes[offset + 1..offset + 5], little_endian);
    let dimensions = match geometry_type {
        1 => 2,
        1001 | 2001 => 3,
        3001 => 4,
        _ => bail!("geometry is not a Point"),
    };
    if bytes.len() != offset + 5 + dimensions * 8 {
        bail!("Point geometry has an invalid length");
    }
    let longitude = read_f64(&bytes[offset + 5..offset + 13], little_endian);
    let latitude = read_f64(&bytes[offset + 13..offset + 21], little_endian);
    if !longitude.is_finite() || !latitude.is_finite() {
        bail!("Point coordinates must be finite");
    }
    if !(-180.0..=180.0).contains(&longitude) {
        bail!("longitude is outside [-180, 180]");
    }
    if !(-90.0..=90.0).contains(&latitude) {
        bail!("latitude is outside [-90, 90]");
    }
    Ok(Point::new(longitude, latitude))
}

fn read_u32(bytes: &[u8], little_endian: bool) -> u32 {
    let bytes: [u8; 4] = bytes.try_into().expect("four-byte slice");
    if little_endian { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) }
}

fn read_i32(bytes: &[u8], little_endian: bool) -> i32 {
    let bytes: [u8; 4] = bytes.try_into().expect("four-byte slice");
    if little_endian { i32::from_le_bytes(bytes) } else { i32::from_be_bytes(bytes) }
}

fn read_f64(bytes: &[u8], little_endian: bool) -> f64 {
    let bytes: [u8; 8] = bytes.try_into().expect("eight-byte slice");
    if little_endian { f64::from_le_bytes(bytes) } else { f64::from_be_bytes(bytes) }
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn validate_options(options: &EnrichmentOptions) -> Result<()> {
    let names = [
        ("ID", options.id_property.as_str()),
        ("importance", options.importance_property.as_str()),
        ("rank", options.rank_property.as_str()),
        ("distance", options.distance_property.as_str()),
        ("minimum-zoom", options.min_zoom_property.as_str()),
    ];
    for (description, name) in names {
        if name.is_empty() {
            bail!("{description} column name must not be empty");
        }
    }
    if options.min_zoom > options.max_zoom {
        bail!("minimum zoom must not exceed maximum zoom");
    }
    if !options.spacing_pixels.is_finite() || options.spacing_pixels <= 0.0 {
        bail!("pixel spacing must be a positive finite number");
    }
    if !options.max_query_distance_metres.is_finite() || options.max_query_distance_metres <= 0.0 {
        bail!("maximum query distance must be a positive finite number");
    }

    let output_names = [&options.rank_property, &options.distance_property, &options.min_zoom_property];
    if output_names.iter().collect::<HashSet<_>>().len() != output_names.len() {
        bail!("rank, distance, and minimum-zoom column names must be distinct");
    }
    for output_name in output_names {
        if output_name == &options.id_property || output_name == &options.importance_property {
            bail!("generated column name '{output_name}' conflicts with an input column name");
        }
    }
    Ok(())
}

fn normalized_f64_bits(value: f64) -> u64 {
    if value == 0.0 { 0.0f64.to_bits() } else { value.to_bits() }
}

fn minimum_zoom(isolation_metres: f64, latitude: f64, options: &EnrichmentOptions) -> i32 {
    if !isolation_metres.is_finite() || isolation_metres <= 0.0 {
        return options.max_zoom;
    }
    let resolution_at_latitude = INITIAL_WEB_MERCATOR_RESOLUTION * latitude.to_radians().cos();
    let required_zoom = (options.spacing_pixels * resolution_at_latitude / isolation_metres).log2().ceil();
    if required_zoom.is_nan() {
        options.max_zoom
    } else {
        required_zoom.clamp(f64::from(options.min_zoom), f64::from(options.max_zoom)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_two_and_three_dimensional_points() {
        assert_eq!(decode_point(&point_blob(13.4, 52.5, false)).unwrap(), Point::new(13.4, 52.5));
        assert_eq!(decode_point(&point_blob(13.4, 52.5, true)).unwrap(), Point::new(13.4, 52.5));
    }

    #[test]
    fn rejects_invalid_point_data() {
        assert!(decode_point(b"not a geometry").is_err());
        assert!(decode_point(&point_blob(181.0, 0.0, false)).is_err());
    }

    #[test]
    fn calculates_latitude_dependent_zoom() {
        let options = EnrichmentOptions {
            min_zoom: 2,
            max_zoom: 10,
            ..EnrichmentOptions::default()
        };
        assert_eq!(minimum_zoom(10_000.0, 0.0, &options), 8);
        assert_eq!(minimum_zoom(10_000.0, 60.0, &options), 7);
        assert_eq!(minimum_zoom(0.0, 0.0, &options), 10);
    }

    fn point_blob(x: f64, y: f64, with_z: bool) -> Vec<u8> {
        let mut bytes = b"GP\0\x01".to_vec();
        bytes.extend_from_slice(&4326_i32.to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(if with_z { 1001_u32 } else { 1_u32 }).to_le_bytes());
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        if with_z {
            bytes.extend_from_slice(&10_f64.to_le_bytes());
        }
        bytes
    }
}
