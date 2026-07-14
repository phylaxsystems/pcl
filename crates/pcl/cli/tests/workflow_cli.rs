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
    project_id: &str,
    page: &str,
    status: usize,
    request_id: &str,
    body: &str,
) -> mockito::Mock {
    server
        .mock(
            "GET",
            format!("/api/v1/views/projects/{project_id}/incidents").as_str(),
        )
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

fn project_incidents_body(ids: &[&str], page: u64, limit: u64) -> String {
    serde_json::json!({
        "data": {
            "items": ids.iter().map(|id| serde_json::json!({
                "assertionAdopterId": "adopter-1",
                "assertionId": "assertion-1",
                "assertionTitle": null,
                "chainId": 1,
                "contractAddress": "0x0000000000000000000000000000000000000001",
                "contractName": null,
                "createdAt": "2026-01-01T00:00:00Z",
                "environment": "production",
                "incidentId": id,
                "id": id,
                "tracesCompleted": 0,
                "tracesPending": 0,
                "transactionCount": 1,
                "windowStart": "2026-01-01T00:00:00Z",
            })).collect::<Vec<_>>(),
            "pagination": {
                "hasNext": false,
                "hasPrev": false,
                "limit": limit,
                "page": page,
                "total": ids.len(),
                "totalPages": 1,
            },
        },
        "_meta": {
            "fetchedAt": "2026-01-01T00:00:00Z",
            "sources": ["offchain"],
        },
    })
    .to_string()
}

#[test]
fn workflow_body_templates_cover_project_release_families() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());
    let api = "http://127.0.0.1:9";
    assert_body_template_fields(
        temp_dir.path(),
        "projects",
        &["projects", "--api-url", api, "create", "--body-template"],
        &["project_name", "chain_id"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "contracts",
        &[
            "contracts",
            "--api-url",
            api,
            "--assign-project",
            "--body-template",
        ],
        &["project_id", "assertion_adopter_ids"],
    );
    assert_body_template_fields(
        temp_dir.path(),
        "releases",
        &[
            "releases",
            "--api-url",
            api,
            "preview",
            "project-1",
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
            "invite",
            "project-1",
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
}

#[test]
fn workflow_body_template_json_flag_emits_json_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(temp_dir.path())
        .arg("--json")
        .args([
            "projects",
            "--api-url",
            "http://127.0.0.1:9",
            "create",
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
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok", "{envelope}");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    assert!(stdout.contains("project_name"), "{stdout}");
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
            "create",
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
    let project_id = "11111111-1111-4111-8111-111111111111";
    let page_1 = mock_incident_export_page(
        &mut server,
        project_id,
        "1",
        200,
        "req-export-1",
        &project_incidents_body(&["i1", "i2"], 1, 2),
    );
    let page_2 = mock_incident_export_page(
        &mut server,
        project_id,
        "2",
        500,
        "req-export-500",
        r#"{"error":"temporary page failure"}"#,
    );
    let page_3 = mock_incident_export_page(
        &mut server,
        project_id,
        "3",
        200,
        "req-export-3",
        &project_incidents_body(&["i3"], 3, 2),
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
            "--project-id",
            project_id,
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
    assert!(out_lines.contains(r#""incidentId":"i1""#), "{out_lines}");
    assert!(out_lines.contains(r#""incidentId":"i2""#), "{out_lines}");
    assert!(out_lines.contains(r#""incidentId":"i3""#), "{out_lines}");

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
