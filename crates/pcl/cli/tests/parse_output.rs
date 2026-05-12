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

    let toon_at_end = run_pcl(&["projects", "--mine", "--saved", "--toon"]);

    assert!(
        !toon_at_end.status.success(),
        "conflicting flags should fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&toon_at_end.stdout),
        String::from_utf8_lossy(&toon_at_end.stderr)
    );
    assert!(toon_at_end.stdout.is_empty());
    let toon_at_end_stderr = String::from_utf8(toon_at_end.stderr).expect("utf-8 toon stderr");
    assert!(toon_at_end_stderr.starts_with("status: error\n"));
    assert!(toon_at_end_stderr.contains("code: cli.argument_conflict"));
    assert!(toon_at_end_stderr.contains("schema_version: pcl.envelope.v1"));
}

#[test]
fn api_manifest_defaults_to_human_output() {
    let output = run_pcl(&["api", "manifest"]);

    assert!(
        output.status.success(),
        "api manifest failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "human success output should be written to stdout: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.starts_with("OK\n"), "{stdout}");
    assert!(stdout.contains("PCL command surface"), "{stdout}");
    assert!(!stdout.starts_with("status: ok\n"), "{stdout}");
    assert!(
        !stdout.contains("schema_version: pcl.envelope.v1"),
        "{stdout}"
    );
}

#[test]
fn documented_agent_leaf_commands_accept_toon_after_subcommands() {
    for args in [
        ["--toon", "--llms"].as_slice(),
        ["api", "manifest", "--toon"].as_slice(),
        ["doctor", "--offline", "--toon"].as_slice(),
        ["whoami", "--offline", "--toon"].as_slice(),
        ["llms", "--toon"].as_slice(),
        ["workflows", "--toon"].as_slice(),
        ["schema", "list", "--toon"].as_slice(),
        ["jobs", "list", "--toon"].as_slice(),
        ["artifacts", "list", "--toon"].as_slice(),
        ["requests", "list", "--toon"].as_slice(),
    ] {
        let output = run_pcl(args);

        assert!(
            output.status.success(),
            "command failed: pcl {}\nstdout:\n{}\nstderr:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "agent success output should be written to stdout: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
        assert!(stdout.starts_with("status: "), "{stdout}");
        assert!(
            stdout.contains("schema_version: pcl.envelope.v1"),
            "{stdout}"
        );
    }
}
