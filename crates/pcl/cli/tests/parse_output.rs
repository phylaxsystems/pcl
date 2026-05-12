use std::process::Command;

fn run_pcl(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(args)
        .output()
        .expect("run pcl")
}

#[test]
fn bare_human_invocation_prints_clap_help() {
    let output = run_pcl(&[]);

    assert!(
        output.status.success(),
        "pcl failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "help should be written to stdout: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.contains("The Credible CLI for the Credible Layer"));
    assert!(stdout.contains("Usage: pcl [OPTIONS] <COMMAND>"));
    assert!(stdout.contains("Commands:"));
    assert!(!stdout.contains("\nCode:"));
    assert!(!stdout.contains("schema_version:"));
}

#[test]
fn human_parse_errors_use_clap_diagnostics() {
    let output = run_pcl(&["projects", "--mine", "--saved"]);

    assert!(
        !output.status.success(),
        "conflicting flags should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "clap diagnostics should be written to stderr: {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(stderr.contains("error: the argument '--mine' cannot be used with '--saved'"));
    assert!(stderr.contains("Usage:"));
    assert!(!stderr.contains("\nCode:"));
    assert!(!stderr.contains("\nNext:"));
    assert!(!stderr.contains("schema_version:"));
}

#[test]
fn machine_parse_errors_stay_structured() {
    let toon = run_pcl(&["--toon", "projects", "--mine", "--saved"]);

    assert!(
        !toon.status.success(),
        "conflicting flags should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&toon.stdout),
        String::from_utf8_lossy(&toon.stderr)
    );
    assert!(toon.stdout.is_empty());
    let toon_stderr = String::from_utf8(toon.stderr).expect("utf-8 toon stderr");
    assert!(toon_stderr.starts_with("status: error\n"));
    assert!(toon_stderr.contains("code: cli.argument_conflict"));
    assert!(toon_stderr.contains("schema_version: pcl.envelope.v1"));

    let json = run_pcl(&["--json", "projects", "--mine", "--saved"]);

    assert!(
        !json.status.success(),
        "conflicting flags should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&json.stdout),
        String::from_utf8_lossy(&json.stderr)
    );
    assert!(json.stdout.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_slice(&json.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "cli.argument_conflict");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
}
