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

fn assert_json_error(output: &PclOutput, code: &str) {
    output.assert_failure();
    assert!(output.stdout.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_str(&output.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error", "{envelope}");
    assert_eq!(envelope["error"]["code"], code, "{envelope}");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
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
fn removed_toon_flag_is_rejected() {
    let output = run_pcl(&["--toon", "api", "manifest"]);
    output.assert_failure();
    assert!(
        output.stderr.contains("--toon") || output.stderr.contains("unexpected argument"),
        "{}",
        output.stderr
    );

    let format_toon = run_pcl(&["--format", "toon", "api", "manifest"]);
    format_toon.assert_failure();
}

#[test]
fn pass_through_developer_commands_reject_machine_modes_structurally() {
    for args in [
        ["--json", "build"].as_slice(),
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
fn subcommand_help_advertises_global_json_mode() {
    fn assert_help(command: &str) {
        let output = run_pcl(&[command, "--help"]);
        output.assert_success();
        assert!(output.stderr.is_empty(), "{command}: {}", output.stderr);
        assert!(
            output.stdout.contains("--json"),
            "{command} help should show global --json:\n{}",
            output.stdout
        );
        assert!(
            !output.stdout.contains("Deprecated; use global --json"),
            "{command} help should not show stale compatibility help:\n{}",
            output.stdout
        );

        let trailing_json = run_pcl(&[command, "--json", "--help"]);
        trailing_json.assert_success();
        assert!(
            trailing_json.stdout.contains("--json") || trailing_json.stderr.contains("--json"),
            "{command} should still accept global --json after the subcommand:\nstdout:\n{}\nstderr:\n{}",
            trailing_json.stdout,
            trailing_json.stderr
        );
    }

    for command in ["apply", "download"] {
        assert_help(command);
    }

    #[cfg(feature = "credible")]
    assert_help("verify");
}

#[test]
fn machine_help_requests_stay_structured() {
    let help = run_pcl(&["projects", "--help", "--json"]);
    help.assert_success();
    assert!(help.stderr.is_empty(), "{}", help.stderr);
    let envelope: serde_json::Value = serde_json::from_str(&help.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok", "{envelope}");
    assert_eq!(envelope["data"]["kind"], "cli.help", "{envelope}");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");

    let root_help = run_pcl(&["--help", "--json"]);
    root_help.assert_success();
    assert!(root_help.stderr.is_empty(), "{}", root_help.stderr);
    let envelope: serde_json::Value =
        serde_json::from_str(&root_help.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok", "{envelope}");
    assert_eq!(envelope["data"]["kind"], "cli.help", "{envelope}");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
}

/// Every `--body-template` invocation the CLI accepts, including the ones whose
/// flag arrives through a flattened or nested args struct — those are the easy
/// ones to record as always needing a platform, because the outer subcommand
/// variant carries no `body_template` field of its own.
const LOCAL_TEMPLATE_INVOCATIONS: &[&[&str]] = &[
    &["--json", "assertions", "--body-template"],
    &["--json", "account", "--body-template"],
    &["--json", "contracts", "--assign-project", "--body-template"],
    &["--json", "deployments", "--confirm", "--body-template"],
    &["--json", "integrations", "--body-template"],
    &["--json", "protocol-manager", "--set", "--body-template"],
    &["--json", "projects", "create", "--body-template"],
    &[
        "--json",
        "projects",
        "update",
        "project-1",
        "--body-template",
    ],
    &[
        "--json",
        "releases",
        "create",
        "project-1",
        "--body-template",
    ],
    &[
        "--json",
        "releases",
        "preview",
        "project-1",
        "--body-template",
    ],
    &["--json", "releases", "deploy", "--body-template"],
    &[
        "--json",
        "releases",
        "remove",
        "project-1",
        "release-1",
        "--body-template",
    ],
    &[
        "--json",
        "releases",
        "retry-check",
        "project-1",
        "release-1",
        "check-1",
        "--body-template",
    ],
    &["--json", "access", "accept", "token-1", "--body-template"],
    &["--json", "access", "invite", "--body-template"],
    &["--json", "access", "invite", "project-1", "--body-template"],
    &[
        "--json",
        "access",
        "resend",
        "project-1",
        "invitation-1",
        "--body-template",
    ],
    &[
        "--json",
        "access",
        "revoke",
        "project-1",
        "invitation-1",
        "--body-template",
    ],
    &[
        "--json",
        "access",
        "role",
        "update",
        "project-1",
        "user-1",
        "--body-template",
    ],
    &[
        "--json",
        "access",
        "member",
        "remove",
        "project-1",
        "user-1",
        "--body-template",
    ],
];

/// `--body-template` prints a static, compiled-in schema. Run with an *empty*
/// config dir and no `PCL_API_URL` on purpose: these commands are how an agent
/// discovers a request body shape, so needing a platform first would make them
/// unusable on a clean install.
#[test]
fn new_workflow_subcommands_parse_and_emit_structured_templates() {
    for args in LOCAL_TEMPLATE_INVOCATIONS {
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

/// `export incidents --dry-run` prints a local plan and fetches nothing, so it
/// must work on a clean non-interactive install. It also must not invent an
/// `--api-url` for the resume command it prints: with no platform chosen, the
/// eventual execution resolves one the same way any other command would.
#[test]
fn incident_export_dry_run_needs_no_platform() {
    let output = run_pcl(&["--json", "export", "incidents", "--dry-run"]);

    output.assert_success();
    let envelope: serde_json::Value = serde_json::from_str(&output.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok", "{envelope}");
    let resume = envelope["data"]["resume_command"]
        .as_str()
        .unwrap_or_default();
    assert!(
        !resume.contains("--api-url"),
        "the resume command must not pin a platform that was never chosen: {resume}"
    );
}

#[test]
fn human_parse_errors_use_clap_diagnostics() {
    let output = run_pcl(&[
        "api",
        "call",
        "get",
        "/health",
        "--body",
        "{}",
        "--body-file",
        "body.json",
    ]);

    output.assert_failure();
    assert!(
        output.stdout.is_empty(),
        "clap diagnostics should be written to stderr: {}",
        output.stdout
    );

    assert!(output.stderr.contains(
        "error: the argument '--body <BODY>' cannot be used with '--body-file <BODY_FILE>'"
    ));
    assert!(output.stderr.contains("Usage:"));
    assert!(!output.stderr.contains("\nCode:"));
    assert!(!output.stderr.contains("\nNext:"));
    assert!(!output.stderr.contains("schema_version:"));
}

#[test]
fn machine_parse_errors_stay_structured() {
    let json = run_pcl(&[
        "--json",
        "api",
        "call",
        "get",
        "/health",
        "--body",
        "{}",
        "--body-file",
        "body.json",
    ]);
    assert_json_error(&json, "cli.argument_conflict");
    let envelope: serde_json::Value =
        serde_json::from_str(&json.stderr).expect("json error envelope");
    let next_actions = envelope["next_actions"]
        .as_array()
        .expect("next actions array");
    assert!(
        next_actions.iter().any(|action| {
            action
                .as_str()
                .is_some_and(|action| action.contains(" --json"))
        }),
        "{envelope}"
    );

    let json_at_end = run_pcl(&[
        "api",
        "call",
        "get",
        "/health",
        "--body",
        "{}",
        "--body-file",
        "body.json",
        "--json",
    ]);
    assert_json_error(&json_at_end, "cli.argument_conflict");
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
fn documented_agent_leaf_commands_accept_json_after_subcommands() {
    for args in [
        ["--json", "--llms"].as_slice(),
        ["api", "manifest", "--json"].as_slice(),
        ["doctor", "--offline", "--json"].as_slice(),
        ["whoami", "--offline", "--json"].as_slice(),
        ["llms", "--json"].as_slice(),
        ["workflows", "--json"].as_slice(),
        ["schema", "list", "--json"].as_slice(),
        ["jobs", "list", "--json"].as_slice(),
        ["artifacts", "list", "--json"].as_slice(),
        ["requests", "list", "--json"].as_slice(),
    ] {
        let output = run_pcl(args);

        output.assert_success();
        assert!(
            output.stderr.is_empty(),
            "agent success output should be written to stdout: {}",
            output.stderr
        );

        let envelope: serde_json::Value =
            serde_json::from_str(&output.stdout).expect("json envelope");
        assert!(envelope["status"].as_str().is_some(), "{envelope}");
        assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    }
}

#[test]
fn schema_list_exposes_output_contract_summary() {
    let output = run_pcl(&["schema", "list", "--json"]);

    output.assert_success();
    assert!(
        output.stderr.is_empty(),
        "schema list should write JSON to stdout: {}",
        output.stderr
    );

    let envelope: serde_json::Value = serde_json::from_str(&output.stdout).expect("json envelope");
    let schemas = envelope["data"]["schemas"]
        .as_array()
        .expect("schemas array");
    assert_eq!(schemas.len(), 12);
    assert!(
        schemas.iter().all(|schema| schema["workflow"] != "api"),
        "{schemas:?}"
    );
    let deployments = schemas
        .iter()
        .find(|schema| schema["workflow"] == "deployments")
        .expect("deployments schema");

    assert_eq!(
        deployments["output_policy"],
        "machine_raw_human_compact_artifacts"
    );
    assert!(
        deployments["output"]
            .as_str()
            .is_some_and(|output| output.contains("deployment")),
        "{deployments:?}"
    );
}

#[test]
fn llms_machine_next_actions_leave_completion_redirect_raw() {
    let completion_install =
        "pcl completions bash > ~/.local/share/bash-completion/completions/pcl";

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

#[test]
fn completions_machine_next_action_is_raw_redirect() {
    let json = run_pcl(&["--json", "completions", "bash"]);
    json.assert_success();
    let envelope: serde_json::Value = serde_json::from_str(&json.stdout).expect("json envelope");
    assert!(
        envelope["data"]["script"]
            .as_str()
            .is_some_and(|script| script.contains("_pcl()")),
        "{envelope}"
    );
    assert_eq!(
        envelope["next_actions"],
        serde_json::json!(["pcl completions bash > <completion-file>"])
    );
}
