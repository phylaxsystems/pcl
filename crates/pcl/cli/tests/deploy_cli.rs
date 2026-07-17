//! CLI-process tests for the `pcl deploy` machine error contract: every
//! failure must produce a structured envelope with a stable code,
//! recoverability, and next actions — never `code: unknown`.

#![cfg(feature = "credible")]

use std::process::Command;

fn run_deploy(config_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--json")
        .arg("deploy")
        .args(args)
        .output()
        .expect("run pcl deploy")
}

fn error_envelope(output: &std::process::Output) -> serde_json::Value {
    assert!(
        !output.status.success(),
        "expected failure, got stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "expected a JSON error envelope ({error}), stderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn deploy_json_without_yes_returns_a_stable_error_envelope() {
    let temp_dir = tempfile::tempdir().expect("temp config dir");

    let output = run_deploy(temp_dir.path(), &[]);

    let envelope = error_envelope(&output);
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "deploy.yes_required");
    assert_eq!(envelope["error"]["recoverable"], true);
    assert!(
        envelope["next_actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty()),
        "{envelope}"
    );
}

#[test]
fn deploy_delegates_wrapped_apply_errors_to_their_envelope() {
    let temp_dir = tempfile::tempdir().expect("temp config dir");
    let missing_root = temp_dir.path().join("does-not-exist");

    let output = run_deploy(
        temp_dir.path(),
        &["--yes", "--root", missing_root.to_str().expect("utf-8")],
    );

    let envelope = error_envelope(&output);
    assert_eq!(envelope["status"], "error");
    // Wrapped apply errors keep their own structured contract instead of
    // flattening to `unknown`.
    assert_eq!(envelope["error"]["code"], "apply.failed");
    assert!(
        envelope["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Project root not found")),
        "{envelope}"
    );
}
