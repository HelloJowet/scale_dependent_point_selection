use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};

use anyhow::{Context, Result, anyhow, bail};
use geo_types::Point;
use serde_json::{Map, Number, Value};

use crate::Tree;

const INITIAL_WEB_MERCATOR_RESOLUTION: f64 = 156_543.033_928_040_97;

#[derive(Clone, Debug)]
/// Configuration for validating and enriching GeoJSON point features.
///
/// Higher numeric values in the configured importance property represent more-important points.
pub struct EnrichmentOptions {
    /// Name of the input property containing a unique string or integer ID.
    pub id_property: String,
    /// Name of the input property containing a unique finite numeric importance score.
    pub importance_property: String,
    /// Name of the generated property containing the descending rank, starting at `1`.
    pub rank_property: String,
    /// Name of the generated property containing the isolation distance in metres.
    pub distance_property: String,
    /// Name of the generated property containing the minimum map zoom.
    pub min_zoom_property: String,
    /// Lowest minimum-zoom value that may be generated.
    pub min_zoom: i32,
    /// Highest minimum-zoom value that may be generated.
    pub max_zoom: i32,
    /// Requested on-screen separation between points in pixels.
    pub spacing_pixels: f64,
    /// Maximum distance in metres within which to search for a more-important point.
    pub max_query_distance_metres: f64,
    /// Whether generated properties may replace properties already present in an input feature.
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
    Integer(i128),
}

struct ParsedFeature {
    feature: Value,
    point: Point,
    importance: f64,
    line_number: usize,
}

/// Reads, validates, and enriches point features from a GeoJSON Text Sequence stream.
///
/// Each nonempty input line must contain exactly one GeoJSON Feature and may begin with the ASCII record separator byte (`0x1e`).
/// All input is validated before enrichment starts, and the original JSON values are retained except for the configured generated properties.
/// The returned features remain in input order.
pub fn enrich_geojson_text_sequence<R: BufRead>(reader: R, options: &EnrichmentOptions) -> Result<Vec<Value>> {
    validate_options(options)?;

    let mut parsed_features = Vec::new();
    let mut ids = HashMap::new();
    let mut importance_values = HashMap::new();

    for (line_index, line_result) in reader.split(b'\n').enumerate() {
        let line_number = line_index + 1;
        let mut line = line_result.with_context(|| format!("failed to read input record at line {line_number}"))?;
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            continue;
        }
        if line.first() == Some(&0x1e) {
            line.remove(0);
        }

        let feature: Value = serde_json::from_slice(&line).with_context(|| format!("malformed JSON at line {line_number}"))?;
        let (id, point, importance) = validate_feature(&feature, line_number, options)?;

        if let Some(previous_line) = ids.insert(id, line_number) {
            bail!("duplicate ID at line {line_number}; it was first used at line {previous_line}");
        }

        let importance_key = normalized_f64_bits(importance);
        if let Some(previous_line) = importance_values.insert(importance_key, line_number) {
            bail!("duplicate or indistinguishable importance value at line {line_number}; it was first used at line {previous_line}");
        }

        parsed_features.push(ParsedFeature {
            feature,
            point,
            importance,
            line_number,
        });
    }

    enrich_features(parsed_features, options)
}

/// Writes features using GeoJSON Text Sequence framing.
///
/// Every feature is serialized as an ASCII record separator byte (`0x1e`), compact JSON, and a line feed.
/// The writer is flushed before the function returns.
pub fn write_geojson_text_sequence<W: Write>(features: &[Value], mut writer: W) -> Result<()> {
    for feature in features {
        writer.write_all(&[0x1e]).context("failed to write GeoJSON record separator")?;
        serde_json::to_writer(&mut writer, feature).context("failed to serialize GeoJSON feature")?;
        writer.write_all(b"\n").context("failed to terminate GeoJSON record")?;
    }
    writer.flush().context("failed to flush GeoJSON output")
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
            bail!("{description} property name must not be empty");
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
        bail!("rank, distance, and minimum-zoom property names must be distinct");
    }
    for output_name in output_names {
        if output_name == &options.id_property || output_name == &options.importance_property {
            bail!("generated property name '{output_name}' conflicts with an input property name");
        }
    }

    Ok(())
}

fn validate_feature(feature: &Value, line_number: usize, options: &EnrichmentOptions) -> Result<(FeatureId, Point, f64)> {
    let object = feature.as_object().ok_or_else(|| anyhow!("record at line {line_number} is not a GeoJSON object"))?;
    if object.get("type").and_then(Value::as_str) != Some("Feature") {
        bail!("record at line {line_number} is not a GeoJSON Feature");
    }

    let geometry = object
        .get("geometry")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("feature at line {line_number} has null or invalid geometry"))?;
    if geometry.get("type").and_then(Value::as_str) != Some("Point") {
        bail!("feature at line {line_number} does not have Point geometry");
    }
    let coordinates = geometry
        .get("coordinates")
        .and_then(Value::as_array)
        .filter(|coordinates| coordinates.len() >= 2)
        .ok_or_else(|| anyhow!("feature at line {line_number} has invalid Point coordinates"))?;
    let longitude = finite_coordinate(&coordinates[0], "longitude", line_number)?;
    let latitude = finite_coordinate(&coordinates[1], "latitude", line_number)?;
    if !(-180.0..=180.0).contains(&longitude) {
        bail!("longitude at line {line_number} is outside [-180, 180]");
    }
    if !(-90.0..=90.0).contains(&latitude) {
        bail!("latitude at line {line_number} is outside [-90, 90]");
    }

    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("feature at line {line_number} must have a properties object"))?;
    let id_value = properties
        .get(&options.id_property)
        .ok_or_else(|| anyhow!("feature at line {line_number} is missing ID property '{}'", options.id_property))?;
    let id = parse_id(id_value, line_number, &options.id_property)?;

    let importance_value = properties
        .get(&options.importance_property)
        .ok_or_else(|| anyhow!("feature at line {line_number} is missing importance property '{}'", options.importance_property))?;
    let importance = importance_value.as_f64().filter(|value| value.is_finite()).ok_or_else(|| {
        anyhow!(
            "importance property '{}' at line {line_number} must be a finite number representable as f64",
            options.importance_property
        )
    })?;

    if !options.overwrite_properties {
        for property in [&options.rank_property, &options.distance_property, &options.min_zoom_property] {
            if properties.contains_key(property) {
                bail!("generated property '{property}' already exists in feature at line {line_number}; use --overwrite-properties to replace it");
            }
        }
    }

    Ok((id, Point::new(longitude, latitude), importance))
}

fn finite_coordinate(value: &Value, name: &str, line_number: usize) -> Result<f64> {
    value
        .as_f64()
        .filter(|coordinate| coordinate.is_finite())
        .ok_or_else(|| anyhow!("{name} at line {line_number} must be a finite number"))
}

fn parse_id(value: &Value, line_number: usize, property_name: &str) -> Result<FeatureId> {
    if let Some(value) = value.as_str() {
        return Ok(FeatureId::String(value.to_string()));
    }
    if let Some(value) = value.as_i64() {
        return Ok(FeatureId::Integer(i128::from(value)));
    }
    if let Some(value) = value.as_u64() {
        return Ok(FeatureId::Integer(i128::from(value)));
    }

    bail!("ID property '{property_name}' at line {line_number} must be a string or integer")
}

fn normalized_f64_bits(value: f64) -> u64 {
    // Positive and negative zero compare as equal scores, so they must also share a uniqueness key.
    if value == 0.0 { 0.0f64.to_bits() } else { value.to_bits() }
}

fn enrich_features(mut features: Vec<ParsedFeature>, options: &EnrichmentOptions) -> Result<Vec<Value>> {
    let mut tree = Tree::new();
    for (index, feature) in features.iter().enumerate() {
        tree.insert_tree_item(&index.to_string(), feature.point, feature.importance);
    }

    // Sorting indices assigns ranks without changing the required input order of the output features.
    let mut ranked_indices: Vec<usize> = (0..features.len()).collect();
    ranked_indices.sort_unstable_by(|left, right| features[*right].importance.partial_cmp(&features[*left].importance).unwrap_or(Ordering::Equal));
    let mut ranks = vec![0_u64; features.len()];
    for (rank, index) in ranked_indices.into_iter().enumerate() {
        ranks[index] = u64::try_from(rank + 1).context("too many features to represent rank")?;
    }

    for (index, parsed) in features.iter_mut().enumerate() {
        let distance = tree.get_distance_to_nearest_more_important_neighbor(parsed.importance, parsed.point, options.max_query_distance_metres);
        let zoom = minimum_zoom(distance, parsed.point.y(), options);
        let properties = feature_properties_mut(&mut parsed.feature, parsed.line_number)?;
        properties.insert(options.rank_property.clone(), Value::Number(Number::from(ranks[index])));
        properties.insert(
            options.distance_property.clone(),
            Value::Number(Number::from_f64(distance).ok_or_else(|| anyhow!("calculated a non-finite distance at line {}", parsed.line_number))?),
        );
        properties.insert(options.min_zoom_property.clone(), Value::Number(Number::from(i64::from(zoom))));
    }

    Ok(features.into_iter().map(|parsed| parsed.feature).collect())
}

fn feature_properties_mut(feature: &mut Value, line_number: usize) -> Result<&mut Map<String, Value>> {
    feature
        .as_object_mut()
        .and_then(|object| object.get_mut("properties"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow!("feature properties unexpectedly changed while processing line {line_number}"))
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
    use std::io::Cursor;

    use serde_json::json;

    use super::*;

    fn feature(id: Value, importance: Value, longitude: f64, latitude: f64) -> Value {
        json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [longitude, latitude, 7]},
            "properties": {"id": id, "importance": importance}
        })
    }

    fn sequence(features: &[Value], record_separator: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        for feature in features {
            if record_separator {
                bytes.push(0x1e);
            }
            serde_json::to_writer(&mut bytes, feature).unwrap();
            bytes.push(b'\n');
        }
        bytes
    }

    #[test]
    fn enriches_and_preserves_complete_features() {
        let first = json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [0.0, 0.0, 25]},
            "properties": {"id": "a", "importance": 10, "nullable": null, "large": -9223372036854775808i64},
            "foreign": {"kept": true}
        });
        let second = feature(json!(2), json!(5.5), 0.01, 0.0);
        let output = enrich_geojson_text_sequence(Cursor::new(sequence(&[first, second], true)), &EnrichmentOptions::default()).unwrap();

        assert_eq!(output[0]["properties"]["rank"], 1);
        assert_eq!(output[1]["properties"]["rank"], 2);
        assert_eq!(output[0]["properties"]["distance_metres"], 20_004_000.0);
        assert!((output[1]["properties"]["distance_metres"].as_f64().unwrap() - 1_113.194_907_932_735_8).abs() < 1e-9);
        assert_eq!(output[0]["properties"]["nullable"], Value::Null);
        assert_eq!(output[0]["properties"]["large"], i64::MIN);
        assert_eq!(output[0]["geometry"]["coordinates"][2], 25);
        assert_eq!(output[0]["foreign"]["kept"], true);
        assert_eq!(output[0]["properties"]["importance"], 10);
    }

    #[test]
    fn accepts_unprefixed_records_and_writes_standard_framing() {
        let input = sequence(&[feature(json!("a"), json!(1), 0.0, 0.0)], false);
        let output = enrich_geojson_text_sequence(Cursor::new(input), &EnrichmentOptions::default()).unwrap();
        let mut serialized = Vec::new();
        write_geojson_text_sequence(&output, &mut serialized).unwrap();
        assert_eq!(serialized.first(), Some(&0x1e));
        assert_eq!(serialized.last(), Some(&b'\n'));
        assert_eq!(serialized.iter().filter(|byte| **byte == 0x1e).count(), 1);
    }

    #[test]
    fn rejects_precision_collisions_after_f64_conversion() {
        let input = concat!(
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[0,0]},\"properties\":{\"id\":1,\"importance\":9007199254740992}}\n",
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[1,0]},\"properties\":{\"id\":2,\"importance\":9007199254740993}}\n"
        );
        let error = enrich_geojson_text_sequence(Cursor::new(input), &EnrichmentOptions::default()).unwrap_err();
        assert!(error.to_string().contains("indistinguishable importance"));
    }

    #[test]
    fn rejects_duplicate_ids_and_generated_properties() {
        let duplicate_ids = sequence(&[feature(json!(1), json!(2), 0.0, 0.0), feature(json!(1), json!(1), 1.0, 0.0)], false);
        assert!(
            enrich_geojson_text_sequence(Cursor::new(duplicate_ids), &EnrichmentOptions::default())
                .unwrap_err()
                .to_string()
                .contains("duplicate ID")
        );

        let mut existing = feature(json!(1), json!(1), 0.0, 0.0);
        existing["properties"]["rank"] = json!(99);
        assert!(
            enrich_geojson_text_sequence(Cursor::new(sequence(&[existing], false)), &EnrichmentOptions::default())
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
    }

    #[test]
    fn explicit_property_overwrite_replaces_generated_values() {
        let mut input = feature(json!(1), json!(1), 0.0, 0.0);
        input["properties"]["rank"] = json!(99);
        let options = EnrichmentOptions {
            overwrite_properties: true,
            ..EnrichmentOptions::default()
        };
        let output = enrich_geojson_text_sequence(Cursor::new(sequence(&[input], false)), &options).unwrap();
        assert_eq!(output[0]["properties"]["rank"], 1);
    }

    #[test]
    fn validates_configuration_and_input_contract() {
        let invalid_options = EnrichmentOptions {
            min_zoom: 5,
            max_zoom: 4,
            ..EnrichmentOptions::default()
        };
        assert!(enrich_geojson_text_sequence(Cursor::new(Vec::<u8>::new()), &invalid_options).is_err());

        let invalid_records = [
            "not json\n",
            "{}\n",
            "{\"type\":\"Feature\",\"geometry\":null,\"properties\":{\"id\":1,\"importance\":1}}\n",
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"LineString\",\"coordinates\":[]},\"properties\":{\"id\":1,\"importance\":1}}\n",
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[181,0]},\"properties\":{\"id\":1,\"importance\":1}}\n",
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[0,91]},\"properties\":{\"id\":1,\"importance\":1}}\n",
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[0,0]},\"properties\":{\"importance\":1}}\n",
            "{\"type\":\"Feature\",\"geometry\":{\"type\":\"Point\",\"coordinates\":[0,0]},\"properties\":{\"id\":1}}\n",
        ];
        for record in invalid_records {
            assert!(
                enrich_geojson_text_sequence(Cursor::new(record), &EnrichmentOptions::default()).is_err(),
                "accepted {record}"
            );
        }
    }

    #[test]
    fn calculates_latitude_dependent_zoom_and_clamps_it() {
        let options = EnrichmentOptions {
            min_zoom: 2,
            max_zoom: 10,
            spacing_pixels: 16.0,
            ..EnrichmentOptions::default()
        };
        assert_eq!(minimum_zoom(10_000.0, 0.0, &options), 8);
        assert_eq!(minimum_zoom(10_000.0, 60.0, &options), 7);
        assert_eq!(minimum_zoom(f64::INFINITY, 0.0, &options), 10);
        assert_eq!(minimum_zoom(0.0, 0.0, &options), 10);
        assert_eq!(minimum_zoom(1.0e20, 0.0, &options), 2);
        assert_eq!(minimum_zoom(0.01, 0.0, &options), 10);
    }

    #[test]
    fn enrichment_is_independent_of_input_order() {
        let features = [
            feature(json!("a"), json!(3), 0.0, 0.0),
            feature(json!("b"), json!(2), 0.1, 0.0),
            feature(json!("c"), json!(1), 0.3, 0.0),
        ];
        let forward = enrich_geojson_text_sequence(Cursor::new(sequence(&features, false)), &EnrichmentOptions::default()).unwrap();
        let reversed_input: Vec<Value> = features.into_iter().rev().collect();
        let reversed = enrich_geojson_text_sequence(Cursor::new(sequence(&reversed_input, true)), &EnrichmentOptions::default()).unwrap();
        let by_id = |values: Vec<Value>| {
            values
                .into_iter()
                .map(|value| (value["properties"]["id"].as_str().unwrap().to_string(), value["properties"].clone()))
                .collect::<HashMap<_, _>>()
        };
        assert_eq!(by_id(forward), by_id(reversed));
    }
}
