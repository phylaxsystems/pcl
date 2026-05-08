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
    let summary: serde_json::Value = serde_json::from_str(&stdout).expect("json summary");
    assert_eq!(summary["status"], "success");
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
