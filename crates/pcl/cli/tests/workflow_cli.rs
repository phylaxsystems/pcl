use mockito::Matcher;
use std::{
    fs,
    process::Command,
};

fn write_valid_auth_config(config_dir: &std::path::Path) {
    fs::write(
        config_dir.join("config.toml"),
        r#"[auth]
access_token = "test-token"
refresh_token = "refresh-token"
expires_at = 4102444800
email = "agent@example.com"
"#,
    )
    .expect("write test config");
}

fn run_pcl(config_dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--json")
        .args(args)
        .output()
        .expect("run pcl")
}

fn assert_json_success(output: &std::process::Output, command: &str) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{command} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{command} wrote stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("json envelope")
}

fn assert_body_template_fields(
    config_dir: &std::path::Path,
    name: &str,
    args: &[&str],
    fields: &[&str],
) {
    let output = run_pcl(config_dir, args);
    let envelope = assert_json_success(&output, name);
    assert_eq!(envelope["status"], "ok", "{name}: {envelope}");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1", "{name}");
    assert!(
        envelope["next_actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty()),
        "{name}: {envelope}"
    );
    for field in fields {
        assert!(
            envelope["data"].get(field).is_some(),
            "{name} missing template field {field}: {envelope}"
        );
    }
}

fn mock_incident_export_page(
    server: &mut mockito::Server,
    page: &str,
    status: usize,
    request_id: &str,
    body: &str,
) -> mockito::Mock {
    server
        .mock("GET", "/api/v1/views/public/incidents")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("environment".into(), "production".into()),
            Matcher::UrlEncoded("page".into(), page.into()),
            Matcher::UrlEncoded("limit".into(), "2".into()),
        ]))
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", request_id)
        .with_body(body)
        .expect(1)
        .create()
}

#[test]
fn workflow_body_templates_cover_project_release_families() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());
    let api = "http://127.0.0.1:9";
    assert_body_template_fields(
        temp_dir.path(),
        "projects",
        &["projects", "--api-url", api, "--create", "--body-template"],
        &["project_name", "chain_id"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "contracts",
        &["contracts", "--api-url", api, "--create", "--body-template"],
        &["network", "address", "contract_name", "project_id"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "releases",
        &[
            "releases",
            "--api-url",
            api,
            "--project",
            "project-1",
            "--preview",
            "--body-template",
        ],
        &["environment", "assertionsDir", "contracts"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "deployments",
        &[
            "deployments",
            "--api-url",
            api,
            "--project",
            "project-1",
            "--confirm",
            "--body-template",
        ],
        &["tx_hash", "chainId", "assertions"],
    );
}

#[test]
fn workflow_body_templates_cover_access_manager_families() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());
    let api = "http://127.0.0.1:9";
    assert_body_template_fields(
        temp_dir.path(),
        "access",
        &[
            "access",
            "--api-url",
            api,
            "--project",
            "project-1",
            "--invite",
            "--body-template",
        ],
        &["identifier", "identifier_type", "role"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "integrations",
        &[
            "integrations",
            "--api-url",
            api,
            "--project",
            "project-1",
            "--provider",
            "slack",
            "--configure",
            "--body-template",
        ],
        &["webhook_url", "enabled"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "protocol-manager",
        &[
            "protocol-manager",
            "--api-url",
            api,
            "--project",
            "project-1",
            "--set",
            "--body-template",
        ],
        &["address", "signature", "nonce"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "transfers",
        &["transfers", "--api-url", api, "--reject", "--body-template"],
        &["ponder_transfer_id"],
    );
}

#[test]
fn workflow_body_template_toon_flag_emits_toon_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(temp_dir.path())
        .arg("--toon")
        .args([
            "projects",
            "--api-url",
            "http://127.0.0.1:9",
            "--create",
            "--body-template",
        ])
        .output()
        .expect("run pcl projects body-template");

    assert!(
        output.status.success(),
        "body template failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.starts_with("status: ok\n"), "{stdout}");
    assert!(
        stdout.contains("schema_version: pcl.envelope.v1"),
        "{stdout}"
    );
    assert!(stdout.contains("project_name:"), "{stdout}");
}

#[test]
fn workflow_mutating_server_error_preserves_request_provenance() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());
    let mut server = mockito::Server::new();
    let create = server
        .mock("POST", "/api/v1/projects")
        .match_header("authorization", "Bearer test-token")
        .match_header(
            "content-type",
            Matcher::Regex("application/json.*".to_string()),
        )
        .match_body(Matcher::Json(serde_json::json!({
            "project_name": "demo",
            "chain_id": 1
        })))
        .with_status(500)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-project-500")
        .with_body(r#"{"message":"created project but failed after commit"}"#)
        .expect(1)
        .create();

    let output = run_pcl(
        temp_dir.path(),
        &[
            "projects",
            "--api-url",
            &server.url(),
            "--create",
            "--project-name",
            "demo",
            "--chain-id",
            "1",
        ],
    );

    assert!(
        !output.status.success(),
        "expected server error, got stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "api.server_error");
    assert_eq!(envelope["error"]["http"]["method"], "POST");
    assert_eq!(envelope["error"]["http"]["path"], "/projects");
    assert_eq!(envelope["error"]["http"]["status"], 500);
    assert_eq!(envelope["error"]["http"]["request_id"], "req-project-500");
    assert_eq!(envelope["error"]["request_id"], "req-project-500");
    assert_eq!(envelope["request_id"], "req-project-500");
    assert_eq!(envelope["outcome_ambiguous"], true);
    assert_eq!(envelope["error"]["mutation"]["side_effecting"], true);
    assert_eq!(envelope["error"]["mutation"]["outcome_ambiguous"], true);
    assert!(
        envelope["suggested_next_actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action == "reconcile_mutation")),
        "{envelope}"
    );
    assert!(
        envelope["next_actions"].as_array().is_some_and(|actions| {
            actions.iter().any(|action| {
                action
                    .as_str()
                    .is_some_and(|value| value.contains("Do not retry immediately"))
            })
        }),
        "{envelope}"
    );
    create.assert();
}

#[test]
fn export_incidents_cli_writes_checkpoint_errors_and_resume_command() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let page_1 = mock_incident_export_page(
        &mut server,
        "1",
        200,
        "req-export-1",
        r#"{"incidents":[{"id":"i1"},{"id":"i2"}]}"#,
    );
    let page_2 = mock_incident_export_page(
        &mut server,
        "2",
        500,
        "req-export-500",
        r#"{"message":"temporary page failure"}"#,
    );
    let page_3 = mock_incident_export_page(
        &mut server,
        "3",
        200,
        "req-export-3",
        r#"{"incidents":[{"id":"i3"}]}"#,
    );

    let out = temp_dir.path().join("incidents.jsonl");
    let errors = temp_dir.path().join("errors.jsonl");
    let checkpoint = temp_dir.path().join("checkpoint.json");
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(temp_dir.path())
        .arg("--json")
        .args([
            "export",
            "incidents",
            "--api-url",
            &server.url(),
            "--allow-unauthenticated",
            "--environment",
            "production",
            "--limit",
            "2",
            "--max-pages",
            "3",
            "--max-retries",
            "0",
            "--continue-on-error",
            "--out",
        ])
        .arg(&out)
        .arg("--errors")
        .arg(&errors)
        .arg("--checkpoint")
        .arg(&checkpoint)
        .output()
        .expect("run pcl export incidents");

    let envelope = assert_json_success(&output, "pcl export incidents");
    assert_eq!(envelope["data"]["pages_fetched"], 2);
    assert_eq!(envelope["data"]["incidents_written"], 3);
    assert_eq!(envelope["data"]["errors_written"], 1);
    assert!(
        envelope["data"]["resume_command"]
            .as_str()
            .is_some_and(|command| {
                command.contains("pcl export incidents")
                    && command.contains("--resume")
                    && command.contains("--continue-on-error")
            }),
        "{envelope}"
    );

    let out_lines = fs::read_to_string(&out).expect("read export output");
    assert!(out_lines.contains(r#""id":"i1""#), "{out_lines}");
    assert!(out_lines.contains(r#""id":"i2""#), "{out_lines}");
    assert!(out_lines.contains(r#""id":"i3""#), "{out_lines}");

    let error_lines = fs::read_to_string(&errors).expect("read export errors");
    assert!(error_lines.contains(r#""page":2"#), "{error_lines}");
    assert!(error_lines.contains(r#""status":500"#), "{error_lines}");
    assert!(error_lines.contains("req-export-500"), "{error_lines}");

    let checkpoint: serde_json::Value =
        serde_json::from_slice(&fs::read(&checkpoint).expect("read checkpoint"))
            .expect("checkpoint json");
    assert_eq!(checkpoint["next_page"], 4);
    assert_eq!(checkpoint["items_written"], 3);

    page_1.assert();
    page_2.assert();
    page_3.assert();
}
