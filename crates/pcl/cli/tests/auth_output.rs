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
