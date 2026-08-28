use std::fs;
use std::path::Path;
use std::process::Command;

use rusqlite::Connection;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scale-dependent-point-selection"))
}

fn point_blob(x: f64, y: f64, with_z: bool) -> Vec<u8> {
    let mut bytes = b"GP\0\x01".to_vec();
    bytes.extend_from_slice(&4326_i32.to_le_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&(if with_z { 1001_u32 } else { 1_u32 }).to_le_bytes());
    bytes.extend_from_slice(&x.to_le_bytes());
    bytes.extend_from_slice(&y.to_le_bytes());
    if with_z {
        bytes.extend_from_slice(&25_f64.to_le_bytes());
    }
    bytes
}

fn create_geopackage(path: &Path, existing_rank: bool) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(&format!(
            "
            PRAGMA application_id = 1196444487;
            PRAGMA user_version = 10300;
            CREATE TABLE gpkg_spatial_ref_sys (
                srs_name TEXT NOT NULL,
                srs_id INTEGER NOT NULL PRIMARY KEY,
                organization TEXT NOT NULL,
                organization_coordsys_id INTEGER NOT NULL,
                definition TEXT NOT NULL,
                description TEXT
            );
            CREATE TABLE gpkg_contents (
                table_name TEXT NOT NULL PRIMARY KEY,
                data_type TEXT NOT NULL,
                identifier TEXT UNIQUE,
                description TEXT DEFAULT '',
                last_change DATETIME NOT NULL,
                min_x DOUBLE,
                min_y DOUBLE,
                max_x DOUBLE,
                max_y DOUBLE,
                srs_id INTEGER
            );
            CREATE TABLE gpkg_geometry_columns (
                table_name TEXT NOT NULL,
                column_name TEXT NOT NULL,
                geometry_type_name TEXT NOT NULL,
                srs_id INTEGER NOT NULL,
                z TINYINT NOT NULL,
                m TINYINT NOT NULL,
                PRIMARY KEY (table_name, column_name)
            );
            CREATE TABLE points (
                fid INTEGER PRIMARY KEY AUTOINCREMENT,
                geom POINT NOT NULL,
                id TEXT NOT NULL,
                importance REAL NOT NULL,
                note TEXT{}
            );
            CREATE TABLE auxiliary (name TEXT PRIMARY KEY, value TEXT);
            INSERT INTO gpkg_spatial_ref_sys VALUES ('WGS 84', 4326, 'EPSG', 4326, 'WGS 84', '');
            INSERT INTO gpkg_contents VALUES ('points', 'features', 'points', '', '2024-01-01T00:00:00.000Z', NULL, NULL, NULL, NULL, 4326);
            INSERT INTO gpkg_geometry_columns VALUES ('points', 'geom', 'POINT', 4326, 2, 0);
            INSERT INTO auxiliary VALUES ('kept', 'unchanged');
            ",
            if existing_rank { ", rank INTEGER" } else { "" }
        ))
        .unwrap();
    if existing_rank {
        connection
            .execute(
                "INSERT INTO points (geom, id, importance, note, rank) VALUES (?1, 'point-a', 20, NULL, 99)",
                [point_blob(13.4, 52.52, true)],
            )
            .unwrap();
    } else {
        connection
            .execute(
                "INSERT INTO points (geom, id, importance, note) VALUES (?1, 'point-a', 20, NULL)",
                [point_blob(13.4, 52.52, true)],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO points (geom, id, importance, note) VALUES (?1, 'point-b', 10, 'example')",
            [point_blob(13.41, 52.52, false)],
        )
        .unwrap();
}

#[test]
fn enriches_a_layer_and_preserves_the_package() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let output = directory.path().join("output.gpkg");
    create_geopackage(&input, false);
    let original_geometry: Vec<u8> = Connection::open(&input)
        .unwrap()
        .query_row("SELECT geom FROM points WHERE fid = 1", [], |row| row.get(0))
        .unwrap();

    let result = binary().args(["--layer", "points"]).arg(&input).arg(&output).output().unwrap();

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let connection = Connection::open(&output).unwrap();
    let first: (i64, f64, i64, Option<String>, Vec<u8>) = connection
        .query_row("SELECT rank, distance_metres, min_zoom, note, geom FROM points WHERE fid = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .unwrap();
    let second_rank: i64 = connection.query_row("SELECT rank FROM points WHERE fid = 2", [], |row| row.get(0)).unwrap();
    let auxiliary: String = connection.query_row("SELECT value FROM auxiliary WHERE name = 'kept'", [], |row| row.get(0)).unwrap();
    assert_eq!(first.0, 1);
    assert_eq!(first.1, 20_004_000.0);
    assert_eq!(first.2, 0);
    assert_eq!(first.3, None);
    assert_eq!(first.4, original_geometry);
    assert_eq!(second_rank, 2);
    assert_eq!(auxiliary, "unchanged");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn refuses_overwrites_and_preserves_output_on_failure() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let output = directory.path().join("output.gpkg");
    create_geopackage(&input, false);
    fs::write(&output, b"original").unwrap();

    let refusal = binary().args(["--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("--force"));
    assert_eq!(fs::read(&output).unwrap(), b"original");

    let connection = Connection::open(&input).unwrap();
    connection.execute("UPDATE points SET geom = X'00' WHERE fid = 1", []).unwrap();
    drop(connection);
    let failure = binary().args(["--force", "--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(!failure.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"original");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn force_and_property_overwrite_are_separate() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let output = directory.path().join("output.gpkg");
    create_geopackage(&input, true);

    let refusal = binary().args(["--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("--overwrite-properties"));
    assert!(!output.exists());

    let accepted = binary().args(["--overwrite-properties", "--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(accepted.status.success(), "{}", String::from_utf8_lossy(&accepted.stderr));
    let rank: i64 = Connection::open(&output)
        .unwrap()
        .query_row("SELECT rank FROM points WHERE fid = 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rank, 1);
}

#[test]
fn requires_a_layer_and_rejects_wrong_layer_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let output = directory.path().join("output.gpkg");
    create_geopackage(&input, false);

    let missing_argument = binary().arg(&input).arg(&output).output().unwrap();
    assert!(!missing_argument.status.success());
    assert!(String::from_utf8_lossy(&missing_argument.stderr).contains("--layer"));

    let missing_layer = binary().args(["--layer", "missing"]).arg(&input).arg(&output).output().unwrap();
    assert!(!missing_layer.status.success());
    assert!(String::from_utf8_lossy(&missing_layer.stderr).contains("was not found"));

    Connection::open(&input)
        .unwrap()
        .execute("UPDATE gpkg_geometry_columns SET srs_id = 3857 WHERE table_name = 'points'", [])
        .unwrap();
    let wrong_crs = binary().args(["--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(!wrong_crs.status.success());
    assert!(String::from_utf8_lossy(&wrong_crs.stderr).contains("EPSG:4326"));
}

#[test]
fn rejects_duplicate_and_wrongly_typed_values() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let output = directory.path().join("output.gpkg");
    create_geopackage(&input, false);
    let connection = Connection::open(&input).unwrap();
    connection.execute("UPDATE points SET importance = 20 WHERE fid = 2", []).unwrap();

    let duplicate = binary().args(["--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("duplicate or indistinguishable importance"));
    assert!(!output.exists());

    connection.execute("UPDATE points SET importance = 10, id = X'01' WHERE fid = 2", []).unwrap();
    drop(connection);
    let wrong_type = binary().args(["--layer", "points"]).arg(&input).arg(&output).output().unwrap();
    assert!(!wrong_type.status.success());
    assert!(String::from_utf8_lossy(&wrong_type.stderr).contains("TEXT or INTEGER"));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn rejects_the_same_physical_input_and_output() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let alias = directory.path().join("alias.gpkg");
    create_geopackage(&input, false);
    fs::hard_link(&input, &alias).unwrap();

    let result = binary().args(["--force", "--layer", "points"]).arg(&input).arg(&alias).output().unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("same physical file"));
}

#[test]
fn supports_quoted_column_names() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.gpkg");
    let output = directory.path().join("output.gpkg");
    create_geopackage(&input, false);

    let result = binary()
        .args(["--layer", "points", "--rank-property", "display rank", "--distance-property", "distance\"metres"])
        .arg(&input)
        .arg(&output)
        .output()
        .unwrap();

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let values: (i64, f64) = Connection::open(&output)
        .unwrap()
        .query_row("SELECT \"display rank\", \"distance\"\"metres\" FROM points WHERE fid = 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(values, (1, 20_004_000.0));
}
