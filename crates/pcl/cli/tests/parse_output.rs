use std::process::Command;

struct PclOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

impl PclOutput {
    fn assert_success(&self) {
        assert!(
            self.success,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
    }

    fn assert_failure(&self) {
        assert!(
            !self.success,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
    }
}

fn run_pcl(args: &[&str]) -> PclOutput {
    let config_dir = tempfile::tempdir().expect("temp config dir");
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(config_dir.path())
        .args(args)
        .output()
        .expect("run pcl");
    PclOutput {
        success: output.status.success(),
        stdout: String::from_utf8(output.stdout).expect("utf-8 stdout"),
        stderr: String::from_utf8(output.stderr).expect("utf-8 stderr"),
    }
}

fn assert_toon_error(output: &PclOutput, code: &str) {
    output.assert_failure();
    assert!(output.stdout.is_empty());
    assert!(output.stderr.starts_with("status: error\n"));
    assert!(output.stderr.contains(&format!("code: {code}")));
    assert!(output.stderr.contains("schema_version: pcl.envelope.v1"));
}

#[test]
fn bare_human_invocation_prints_clap_help() {
    let output = run_pcl(&[]);

    output.assert_success();
    assert!(
        output.stderr.is_empty(),
        "help should be written to stdout: {}",
        output.stderr
    );

    for expected in [
        "The Credible CLI for the Credible Layer",
        "Usage: pcl [OPTIONS] <COMMAND>",
        "Commands:",
        "  incidents",
        "  projects",
        "  api",
        "Options:",
    ] {
        assert!(output.stdout.contains(expected));
    }
    assert!(!output.stdout.contains("Core workflows:"));
    assert!(!output.stdout.contains("Developer commands:"));
    assert!(!output.stdout.contains("\nCode:"));
    assert!(!output.stdout.contains("schema_version:"));
}

#[test]
fn pass_through_developer_commands_reject_machine_modes_structurally() {
    for args in [
        ["--toon", "build"].as_slice(),
        ["--json", "build"].as_slice(),
        #[cfg(feature = "credible")]
        ["--toon", "test"].as_slice(),
        #[cfg(feature = "credible")]
        ["--json", "test"].as_slice(),
    ] {
        let output = run_pcl(args);

        output.assert_failure();
        assert!(output.stdout.is_empty(), "{}", output.stdout);
        assert!(
            output.stderr.contains("developer pass-through command"),
            "{}",
            output.stderr
        );
        assert!(
            output.stderr.contains("schema_version")
                || output.stderr.contains("\"schema_version\""),
            "{}",
            output.stderr
        );
    }
}

#[test]
fn new_workflow_subcommands_parse_and_emit_structured_dry_runs() {
    for args in [
        [
            "--json",
            "projects",
            "create",
            "--project-name",
            "demo",
            "--chain-id",
            "1",
            "--dry-run",
        ]
        .as_slice(),
        [
            "--json",
            "projects",
            "update",
            "project-1",
            "--field",
            "github_url=https://github.com/org/repo",
            "--dry-run",
        ]
        .as_slice(),
        [
            "--json",
            "releases",
            "preview",
            "project-1",
            "--body-template",
            "--dry-run",
        ]
        .as_slice(),
        [
            "--json",
            "access",
            "invite",
            "project-1",
            "--body-template",
            "--dry-run",
        ]
        .as_slice(),
    ] {
        let output = run_pcl(args);

        output.assert_success();
        assert!(output.stderr.is_empty(), "{}", output.stderr);
        let envelope: serde_json::Value =
            serde_json::from_str(&output.stdout).expect("json envelope");
        assert_eq!(envelope["status"], "ok", "{envelope}");
        assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
        assert!(
            envelope["next_actions"].as_array().is_some_and(|actions| {
                actions.iter().all(|action| {
                    action
                        .as_str()
                        .is_none_or(|action| !action.contains("--toon"))
                })
            }),
            "{envelope}"
        );
    }
}

#[test]
fn human_parse_errors_use_clap_diagnostics() {
    let output = run_pcl(&["projects", "--mine", "--saved"]);

    output.assert_failure();
    assert!(
        output.stdout.is_empty(),
        "clap diagnostics should be written to stderr: {}",
        output.stdout
    );

    assert!(
        output
            .stderr
            .contains("error: the argument '--mine' cannot be used with '--saved'")
    );
    assert!(output.stderr.contains("Usage:"));
    assert!(!output.stderr.contains("\nCode:"));
    assert!(!output.stderr.contains("\nNext:"));
    assert!(!output.stderr.contains("schema_version:"));
}

#[test]
fn machine_parse_errors_stay_structured() {
    let toon = run_pcl(&["--toon", "projects", "--mine", "--saved"]);
    assert_toon_error(&toon, "cli.argument_conflict");

    let json = run_pcl(&["--json", "projects", "--mine", "--saved"]);
    json.assert_failure();
    assert!(json.stdout.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_str(&json.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "cli.argument_conflict");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");

    let toon_at_end = run_pcl(&["projects", "--mine", "--saved", "--toon"]);
    assert_toon_error(&toon_at_end, "cli.argument_conflict");
}

#[test]
fn api_manifest_defaults_to_human_output() {
    let output = run_pcl(&["api", "manifest"]);

    output.assert_success();
    assert!(
        output.stderr.is_empty(),
        "human success output should be written to stdout: {}",
        output.stderr
    );

    assert!(output.stdout.starts_with("OK\n"), "{}", output.stdout);
    assert!(
        output.stdout.contains("PCL command surface"),
        "{}",
        output.stdout
    );
    assert!(
        !output.stdout.starts_with("status: ok\n"),
        "{}",
        output.stdout
    );
    assert!(
        !output.stdout.contains("schema_version: pcl.envelope.v1"),
        "{}",
        output.stdout
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

        output.assert_success();
        assert!(
            output.stderr.is_empty(),
            "agent success output should be written to stdout: {}",
            output.stderr
        );

        assert!(output.stdout.starts_with("status: "), "{}", output.stdout);
        assert!(
            output.stdout.contains("schema_version: pcl.envelope.v1"),
            "{}",
            output.stdout
        );
    }
}

#[test]
fn llms_machine_next_actions_leave_completion_redirect_raw() {
    let completion_install =
        "pcl completions bash > ~/.local/share/bash-completion/completions/pcl";

    let toon = run_pcl(&["--toon", "--llms"]);
    toon.assert_success();
    assert!(toon.stdout.contains(completion_install), "{}", toon.stdout);
    assert!(
        !toon
            .stdout
            .contains(&format!("{completion_install} --toon")),
        "{}",
        toon.stdout
    );

    let json = run_pcl(&["--json", "--llms"]);
    json.assert_success();
    let envelope: serde_json::Value = serde_json::from_str(&json.stdout).expect("json envelope");
    let actions = envelope["next_actions"]
        .as_array()
        .expect("next_actions array");
    assert!(
        actions
            .iter()
            .any(|action| action.as_str() == Some(completion_install)),
        "{envelope}"
    );
    let json_flagged_install = format!("{completion_install} --json");
    assert!(
        actions
            .iter()
            .all(|action| action.as_str() != Some(json_flagged_install.as_str())),
        "{envelope}"
    );
}
