#[cfg(feature = "full")]
use std::{
    fs,
    path::Path,
    process::Command,
};

#[cfg(feature = "full")]
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create fixture destination");
    for entry in fs::read_dir(from).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &destination);
        } else {
            fs::copy(&source, &destination).expect("copy fixture file");
        }
    }
}

#[cfg(feature = "full")]
fn fixture_project() -> tempfile::TempDir {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify-project");
    let temp_dir = tempfile::tempdir().expect("create temp project");
    copy_dir(&fixture, temp_dir.path());
    temp_dir
}

#[cfg(feature = "full")]
fn assert_verify_success(output: std::process::Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    let summary = &envelope["data"];
    assert_eq!(summary["outcome"], "success");
    assert_eq!(summary["total"], 1);
    assert_eq!(summary["passed"], 1);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["assertions"][0]["name"], "NoArgsAssertion");
    assert_eq!(summary["assertions"][0]["status"], "success");
    assert_eq!(
        summary["assertions"][0]["triggers"]["0x0f04ec21"],
        "allCall"
    );
}

#[cfg(feature = "full")]
fn assert_command_success(output: &std::process::Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "full")]
#[test]
fn build_cli_succeeds_for_fixture_project() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "build",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
        ])
        .output()
        .expect("run pcl build");

    assert_command_success(&output, "pcl build");
}

#[cfg(feature = "full")]
#[test]
fn test_cli_succeeds_for_fixture_project() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "test",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
        ])
        .output()
        .expect("run pcl test");

    assert_command_success(&output, "pcl test");
}

#[cfg(feature = "full")]
#[test]
fn apply_dry_run_builds_and_verifies_fixture_payload_without_api() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--json",
            "apply",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "--dry-run",
        ])
        .output()
        .expect("run pcl apply dry-run");

    assert_command_success(&output, "pcl apply --dry-run");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    let summary = &envelope["data"];
    assert_eq!(summary["outcome"], "dry_run");
    assert_eq!(
        summary["project_id"],
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(summary["applied"], false);
    assert_eq!(summary["preview"], serde_json::Value::Null);
    assert_eq!(summary["release"], serde_json::Value::Null);
    assert_eq!(summary["verification"]["status"], "success");
    assert_eq!(summary["verification"]["passed"], 1);
    assert_eq!(
        summary["payload"]["contracts"]["mock"]["assertions"][0]["contractName"],
        "NoArgsAssertion"
    );
    assert!(
        summary["payload"]["contracts"]["mock"]["assertions"][0]["bytecode"]
            .as_str()
            .is_some_and(|bytecode| bytecode.starts_with("0x"))
    );
}

#[cfg(feature = "full")]
#[test]
fn apply_dry_run_json_preserves_failed_assertion_summary() {
    let project = fixture_project();
    fs::write(
        project.path().join("assertions/src/NoArgsAssertion.a.sol"),
        r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

abstract contract Assertion {
    function triggers() external view virtual;
}

contract NoArgsAssertion is Assertion {
    function triggers() external view override {}

    function assertionCheckBool() external pure returns (bool) {
        return true;
    }
}
"#,
    )
    .expect("write failing assertion fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--json",
            "apply",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "--dry-run",
        ])
        .output()
        .expect("run pcl apply dry-run");

    assert!(
        !output.status.success(),
        "pcl apply unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "apply.assertions_failed");
    assert_eq!(envelope["data"]["status"], "failure");
    assert_eq!(envelope["data"]["failed"], 1);
    assert_eq!(envelope["data"]["assertions"][0]["name"], "NoArgsAssertion");
    assert_eq!(envelope["data"]["assertions"][0]["status"], "no_triggers");
    assert_eq!(
        envelope["next_actions"][0],
        "Inspect data.assertions for failing assertions"
    );
}

#[cfg(feature = "full")]
#[test]
fn verify_cli_succeeds_for_explicit_fixture_assertion() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "verify",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "assertions/src/NoArgsAssertion.a.sol:NoArgsAssertion",
            "--json",
        ])
        .output()
        .expect("run pcl verify");

    assert_verify_success(output);
}

#[cfg(feature = "full")]
#[test]
fn verify_cli_succeeds_for_credible_toml_fixture() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "verify",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "--json",
        ])
        .output()
        .expect("run pcl verify");

    assert_verify_success(output);
}
