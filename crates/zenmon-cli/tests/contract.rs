//! Deterministic (no-router) integration tests for the `contract` subcommand,
//! which runs entirely offline against a vendored fixture contract.

use std::process::Command;

fn zenmon() -> Command {
    Command::new(env!("CARGO_BIN_EXE_zenmon"))
}

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/sample.contract.yaml"
);

#[test]
fn lint_reports_counts_and_warnings() {
    let out = zenmon()
        .args(["--json", "contract", "lint", FIXTURE])
        .output()
        .expect("run zenmon");
    assert!(out.status.success(), "lint should exit 0");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["topics"], 4);
    assert_eq!(v["types"], 1);
    assert_eq!(v["services"], 2);
    let warnings = v["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w.as_str().unwrap().contains("not-implemented")),
        "should warn about the not-implemented topic: {warnings:?}"
    );
}

#[test]
fn list_emits_every_topic() {
    let out = zenmon()
        .args(["--json", "contract", "list", FIXTURE])
        .output()
        .expect("run zenmon");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["count"], 4);
    let keys: Vec<&str> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&"topic/navigation/robot_pose"));
    assert!(keys.contains(&"topic/sensor/pcd/{sensor_id}"));
}

#[test]
fn show_matches_placeholder_and_reports_encoding_override() {
    // A concrete sensor key resolves to the {sensor_id} entry.
    let out = zenmon()
        .args(["--json", "contract", "show", "topic/sensor/pcd/front", FIXTURE])
        .output()
        .expect("run zenmon");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["key"], "topic/sensor/pcd/{sensor_id}");
    assert_eq!(v["encoding"], "application/msgpack");
    assert_eq!(v["enveloped"], false);
}

#[test]
fn show_expands_ref_in_payload() {
    let out = zenmon()
        .args(["--json", "contract", "show", "topic/navigation/robot_pose", FIXTURE])
        .output()
        .expect("run zenmon");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    // The $ref: Pose2D must be expanded to its fields, not left as a ref.
    assert_eq!(v["payload"]["x"], "f64");
    assert_eq!(v["payload"]["theta"], "f64");
    assert!(v["payload"].get("$ref").is_none());
}

#[test]
fn show_undeclared_key_exits_not_found() {
    let out = zenmon()
        .args(["--json", "contract", "show", "topic/nope", FIXTURE])
        .output()
        .expect("run zenmon");
    assert_eq!(out.status.code(), Some(5), "not_found should exit 5");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json error");
    assert_eq!(v["error"]["kind"], "not_found");
}

#[test]
fn contract_path_resolves_from_global_flag() {
    // No positional path; the global --contract flag supplies it.
    let out = zenmon()
        .args(["--json", "--contract", FIXTURE, "contract", "lint"])
        .output()
        .expect("run zenmon");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["topics"], 4);
}

#[test]
fn contract_path_resolves_from_env() {
    let out = zenmon()
        .args(["--json", "contract", "lint"])
        .env("ZENMON_CONTRACT", FIXTURE)
        .output()
        .expect("run zenmon");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(v["topics"], 4);
}

#[test]
fn missing_contract_path_is_invalid_input() {
    let out = zenmon()
        .args(["--json", "contract", "lint"])
        .env_remove("ZENMON_CONTRACT")
        .output()
        .expect("run zenmon");
    assert_eq!(out.status.code(), Some(2), "no contract should exit 2");
    let stderr = String::from_utf8(out.stderr).unwrap();
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json error");
    assert_eq!(v["error"]["kind"], "invalid_input");
}

#[test]
fn unreadable_contract_path_is_invalid_input() {
    let out = zenmon()
        .args(["--json", "contract", "lint", "/no/such/contract.yaml"])
        .output()
        .expect("run zenmon");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).unwrap();
    let v: serde_json::Value = serde_json::from_str(stderr.trim()).expect("json error");
    assert_eq!(v["error"]["kind"], "invalid_input");
}
