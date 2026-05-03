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

#[test]
fn auth_login_json_with_existing_auth_outputs_json_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "login",
        ])
        .output()
        .expect("run pcl auth login");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(!stdout.contains("Already logged in"));
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    assert_eq!(envelope["data"]["authenticated"], true);
    assert_eq!(envelope["data"]["email"], "agent@example.com");
    assert_eq!(envelope["data"]["token_valid"], true);
}

#[test]
fn auth_login_json_fresh_flow_outputs_pending_and_terminal_events() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let auth_code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2099-12-31T00:00:00Z"}"#,
        )
        .expect(1)
        .create();
    let auth_status = server
        .mock("GET", "/api/v1/cli/auth/status")
        .match_query(mockito::Matcher::AllOf(vec![
            mockito::Matcher::UrlEncoded(
                "session_id".into(),
                "550e8400-e29b-41d4-a716-446655440000".into(),
            ),
            mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","address":"0x1234567890123456789012345678901234567890","token":"test-token","refresh_token":"refresh-token","email":"agent@example.com"}"#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .env("PCL_AUTH_NO_BROWSER", "1")
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "login",
        ])
        .output()
        .expect("run pcl auth login");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "expected JSONL auth events: {stdout}");
    let pending: serde_json::Value = serde_json::from_str(lines[0]).expect("pending event");
    let terminal: serde_json::Value = serde_json::from_str(lines[1]).expect("terminal event");

    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["event"], "auth.login_instructions");
    assert_eq!(pending["terminal"], false);
    assert_eq!(pending["output_mode"], "jsonl");
    assert_eq!(terminal["status"], "ok");
    assert_eq!(terminal["event"], "auth.login_complete");
    assert_eq!(terminal["terminal"], true);
    assert_eq!(terminal["data"]["authenticated"], true);
    assert_eq!(terminal["data"]["email"], "agent@example.com");
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(config.contains("access_token = \"test-token\""));
    auth_code.assert();
    auth_status.assert();
}

#[test]
fn invalid_config_returns_json_error_without_overwriting_file() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let original_config = "not = [toml\n";
    fs::write(&config_path, original_config).expect("write invalid config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "config",
            "show",
        ])
        .output()
        .expect("run pcl config show");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    let envelope: serde_json::Value = serde_json::from_str(&stderr).expect("json envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "config.parse_failed");
    assert_eq!(
        fs::read_to_string(config_path).expect("read invalid config"),
        original_config
    );
}

#[test]
fn doctor_can_run_with_invalid_config_without_overwriting_file() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let original_config = "not = [toml\n";
    fs::write(&config_path, original_config).expect("write invalid config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "doctor",
            "--offline",
        ])
        .output()
        .expect("run pcl doctor");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    assert_eq!(
        fs::read_to_string(config_path).expect("read invalid config"),
        original_config
    );
}

#[test]
fn workflows_can_run_with_invalid_config_without_overwriting_file() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let original_config = "not = [toml\n";
    fs::write(&config_path, original_config).expect("write invalid config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "workflows",
        ])
        .output()
        .expect("run pcl workflows");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    assert_eq!(
        fs::read_to_string(config_path).expect("read invalid config"),
        original_config
    );
}

#[test]
fn global_llms_flag_outputs_json_without_config_read() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let original_config = "not = [toml\n";
    fs::write(&config_path, original_config).expect("write invalid config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "--llms",
        ])
        .output()
        .expect("run pcl --llms");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    assert_eq!(envelope["data"]["default_output"], "toon");
    assert_eq!(envelope["data"]["no_mcp_required"], true);
    assert_eq!(
        fs::read_to_string(config_path).expect("read invalid config"),
        original_config
    );
}

#[test]
fn completions_can_run_with_invalid_config_without_overwriting_file() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let original_config = "not = [toml\n";
    fs::write(&config_path, original_config).expect("write invalid config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "completions",
            "bash",
        ])
        .output()
        .expect("run pcl completions");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.contains("_pcl"));
    assert!(stdout.contains("complete"));

    let json_output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "completions",
            "bash",
        ])
        .output()
        .expect("run pcl completions --json");
    assert!(
        json_output.status.success(),
        "json command failed: {}",
        String::from_utf8_lossy(&json_output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&json_output.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["shell"], "bash");
    assert!(
        envelope["data"]["script"]
            .as_str()
            .is_some_and(|script| script.contains("_pcl"))
    );
    assert_eq!(
        fs::read_to_string(config_path).expect("read invalid config"),
        original_config
    );
}

#[test]
fn api_dry_run_project_create_does_not_hit_network() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "api",
            "--api-url",
            "http://127.0.0.1:9",
            "--dry-run",
            "projects",
            "--create",
            "--project-name",
            "demo",
            "--chain-id",
            "1",
        ])
        .output()
        .expect("run pcl api projects dry-run");

    assert!(
        output.status.success(),
        "dry-run attempted network/auth path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["request"]["method"], "POST");
    assert_eq!(envelope["data"]["request"]["path"], "/projects");
}

#[test]
fn api_dry_run_assertion_submit_does_not_hit_network() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "api",
            "--api-url",
            "http://127.0.0.1:9",
            "--dry-run",
            "assertions",
            "--project-id",
            "project-1",
            "--submit",
            "--body",
            r#"{"assertions":[]}"#,
        ])
        .output()
        .expect("run pcl api assertions dry-run");

    assert!(
        output.status.success(),
        "dry-run attempted network/auth path: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["dry_run"], true);
    assert_eq!(envelope["data"]["request"]["method"], "POST");
    assert_eq!(
        envelope["data"]["request"]["path"],
        "/projects/project-1/submitted-assertions"
    );
}

#[test]
fn api_dry_run_auth_metadata_keeps_required_separate_from_attachment() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "api",
            "--allow-unauthenticated",
            "--dry-run",
            "projects",
            "--home",
        ])
        .output()
        .expect("run pcl api dry-run");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["data"]["request"]["auth"]["required"], true);
    assert_eq!(
        envelope["data"]["request"]["auth"]["will_attach_stored_token"],
        false
    );
}

#[test]
fn top_level_project_workflow_matches_api_alias() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let api_output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "api",
            "--api-url",
            "http://127.0.0.1:9",
            "--dry-run",
            "projects",
            "--create",
            "--project-name",
            "demo",
            "--chain-id",
            "1",
        ])
        .output()
        .expect("run pcl api projects dry-run");

    let top_level_output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "projects",
            "--api-url",
            "http://127.0.0.1:9",
            "--dry-run",
            "--create",
            "--project-name",
            "demo",
            "--chain-id",
            "1",
        ])
        .output()
        .expect("run pcl projects dry-run");

    assert!(
        api_output.status.success(),
        "api alias failed: {}",
        String::from_utf8_lossy(&api_output.stderr)
    );
    assert!(
        top_level_output.status.success(),
        "top-level workflow failed: {}",
        String::from_utf8_lossy(&top_level_output.stderr)
    );
    let api_envelope: serde_json::Value =
        serde_json::from_slice(&api_output.stdout).expect("api json envelope");
    let top_level_envelope: serde_json::Value =
        serde_json::from_slice(&top_level_output.stdout).expect("top-level json envelope");
    assert_eq!(top_level_envelope["status"], "ok");
    assert_eq!(top_level_envelope["data"], api_envelope["data"]);
}

#[test]
fn top_level_public_incidents_workflow_matches_api_alias() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");

    let api_output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "api",
            "--dry-run",
            "incidents",
            "--limit",
            "5",
        ])
        .output()
        .expect("run pcl api incidents dry-run");

    let top_level_output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "incidents",
            "--dry-run",
            "--limit",
            "5",
        ])
        .output()
        .expect("run pcl incidents dry-run");

    assert!(
        api_output.status.success(),
        "api alias failed: {}",
        String::from_utf8_lossy(&api_output.stderr)
    );
    assert!(
        top_level_output.status.success(),
        "top-level workflow failed: {}",
        String::from_utf8_lossy(&top_level_output.stderr)
    );
    let api_envelope: serde_json::Value =
        serde_json::from_slice(&api_output.stdout).expect("api json envelope");
    let top_level_envelope: serde_json::Value =
        serde_json::from_slice(&top_level_output.stdout).expect("top-level json envelope");
    assert_eq!(top_level_envelope["status"], "ok");
    assert_eq!(top_level_envelope["data"], api_envelope["data"]);
}

#[test]
fn agent_product_surfaces_emit_json_envelopes() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());
    let config_dir = temp_dir.path().to_str().expect("utf-8 temp path");

    for command in [
        vec!["doctor", "--offline"],
        vec!["whoami"],
        vec!["workflows", "show", "incident-investigation"],
        vec!["schema", "get", "incidents", "--action", "list_public"],
        vec!["llms"],
        vec!["jobs", "path"],
        vec!["artifacts", "path"],
        vec!["requests", "path"],
        vec![
            "export",
            "incidents",
            "--project-id",
            "project-1",
            "--dry-run",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
            .args(["--config-dir", config_dir, "--json"])
            .args(command)
            .output()
            .expect("run pcl product surface");

        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("json envelope");
        assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
        assert!(
            envelope["status"].as_str().is_some(),
            "missing status in {envelope}"
        );
    }
}

#[test]
fn api_request_logs_respect_config_dir() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_dir = temp_dir.path().to_str().expect("utf-8 temp path");
    let mut server = mockito::Server::new();
    let health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-config-dir")
        .with_body(r#"{"healthy":true}"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            config_dir,
            "--json",
            "api",
            "--api-url",
            &server.url(),
            "--allow-unauthenticated",
            "call",
            "get",
            "/health",
        ])
        .output()
        .expect("run pcl api call");

    assert!(
        output.status.success(),
        "api call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    health.assert();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["--config-dir", config_dir, "--json", "requests", "list"])
        .output()
        .expect("run pcl requests list");

    assert!(
        output.status.success(),
        "requests list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(
        envelope["data"]["request_log"],
        temp_dir.path().join("requests.jsonl").display().to_string()
    );
    assert!(
        envelope["data"]["records"]
            .as_array()
            .is_some_and(|records| {
                records
                    .iter()
                    .any(|record| record["request_id"] == "req-config-dir")
            }),
        "{envelope}"
    );
}

#[test]
fn default_error_output_is_structured_toon_envelope() {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["api", "call", "get", "health"])
        .output()
        .expect("run pcl api call");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(stderr.contains("status: error"), "{stderr}");
    assert!(stderr.contains("code: input.invalid_path"), "{stderr}");
    assert!(stderr.contains("next_actions[2]:"), "{stderr}");
    assert!(
        stderr.contains("schema_version: pcl.envelope.v1"),
        "{stderr}"
    );
}

#[test]
fn api_manifest_json_exposes_agent_contract_fields() {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["--json", "api", "manifest"])
        .output()
        .expect("run pcl api manifest");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");

    let commands = envelope["data"]["commands"]
        .as_array()
        .expect("commands array");
    let incidents = commands
        .iter()
        .find(|command| {
            command["command"]
                .as_str()
                .is_some_and(|command| command.starts_with("pcl incidents "))
        })
        .expect("incidents manifest entry");
    let actions = incidents["actions"].as_array().expect("actions array");
    assert!(actions.iter().any(|action| {
        action["name"] == "retry_trace"
            && action["method"] == "POST"
            && action["required_flags"]
                .as_array()
                .is_some_and(|flags| flags.iter().any(|flag| flag == "--tx-id"))
    }));
}
