use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::json;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scale-dependent-point-selection"))
}

fn input_feature(extra_property: Option<(&str, serde_json::Value)>) -> String {
    let mut feature = json!({
        "type": "Feature",
        "geometry": {"type": "Point", "coordinates": [10, 50]},
        "properties": {"id": "point-1", "importance": 1}
    });
    if let Some((name, value)) = extra_property {
        feature["properties"][name] = value;
    }
    format!("{feature}\n")
}

fn run_with_stdin(arguments: &[&str], input: &str) -> Output {
    let mut child = binary()
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn supports_stdin_stdout_with_clean_standard_output() {
    let output = run_with_stdin(&["-", "-"], &input_feature(None));
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout.first(), Some(&0x1e));
    assert_eq!(output.stdout.last(), Some(&b'\n'));
    let value: serde_json::Value = serde_json::from_slice(&output.stdout[1..output.stdout.len() - 1]).unwrap();
    assert_eq!(value["properties"]["rank"], 1);
}

#[test]
fn reads_and_atomically_writes_named_files() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.geojsonseq");
    let output = directory.path().join("output.geojsonseq");
    fs::write(&input, input_feature(None)).unwrap();
    let result = binary().arg(&input).arg(&output).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(fs::read(&output).unwrap().starts_with(&[0x1e]));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);

    fs::write(&input, input_feature(Some(("updated", json!(true))))).unwrap();
    let replacement = binary().arg("--force").arg(&input).arg(&output).output().unwrap();
    assert!(replacement.status.success(), "{}", String::from_utf8_lossy(&replacement.stderr));
    assert!(String::from_utf8_lossy(&fs::read(&output).unwrap()).contains("updated"));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn refuses_overwrite_and_preserves_destination_on_failure() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.geojsonseq");
    let output = directory.path().join("output.geojsonseq");
    fs::write(&input, input_feature(None)).unwrap();
    fs::write(&output, b"original").unwrap();
    let refusal = binary().arg(&input).arg(&output).output().unwrap();
    assert!(!refusal.status.success());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("--force"));
    assert_eq!(fs::read(&output).unwrap(), b"original");

    fs::write(&input, b"malformed\n").unwrap();
    let failure = binary().arg("--force").arg(&input).arg(&output).output().unwrap();
    assert!(!failure.status.success());
    assert_eq!(fs::read(&output).unwrap(), b"original");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn processing_failure_does_not_create_a_destination() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.geojsonseq");
    let output = directory.path().join("output.geojsonseq");
    fs::write(&input, b"malformed\n").unwrap();

    let result = binary().arg(&input).arg(&output).output().unwrap();

    assert!(!result.status.success());
    assert!(!output.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn rejects_the_same_physical_input_and_output() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input.geojsonseq");
    let alias = directory.path().join("alias.geojsonseq");
    fs::write(&input, input_feature(None)).unwrap();
    fs::hard_link(&input, &alias).unwrap();
    let result = binary().arg("--force").arg(&input).arg(&alias).output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("same physical file"));
}

#[test]
fn property_overwrite_requires_its_own_flag() {
    let input = input_feature(Some(("rank", json!("old"))));
    let refusal = run_with_stdin(&["-", "-"], &input);
    assert!(!refusal.status.success());
    assert!(refusal.stdout.is_empty());
    assert!(String::from_utf8_lossy(&refusal.stderr).contains("--overwrite-properties"));

    let accepted = run_with_stdin(&["--overwrite-properties", "-", "-"], &input);
    assert!(accepted.status.success(), "{}", String::from_utf8_lossy(&accepted.stderr));
    let value: serde_json::Value = serde_json::from_slice(&accepted.stdout[1..accepted.stdout.len() - 1]).unwrap();
    assert_eq!(value["properties"]["rank"], 1);
}

#[test]
fn invalid_numeric_configuration_is_only_on_stderr() {
    let result = run_with_stdin(&["--spacing-pixels", "0", "-", "-"], &input_feature(None));
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("positive finite"));
}
