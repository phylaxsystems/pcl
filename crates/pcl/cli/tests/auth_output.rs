#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    process::Command,
};

/// The platform recorded by fixtures that just need "a logged-in user".
///
/// Every config written after a login records its platform explicitly — there
/// is no production default whose absence stands in for one. A config with
/// credentials but no `platform_url` is a pre-upgrade artifact, and is covered
/// separately by the platform-boundary unit tests.
const TEST_PLATFORM_URL: &str = "https://linea.phylax.systems";

fn write_valid_auth_config(config_dir: &std::path::Path) {
    write_valid_auth_config_for_platform(config_dir, TEST_PLATFORM_URL);
}

/// Like [`write_valid_auth_config`], but records the platform that issued the
/// credentials, so commands aimed at that URL pass the platform-boundary
/// check.
///
/// `issuer_platform_url` under `[auth]` is what the boundary check reads; the
/// top-level `platform_url` only says which platform is remembered. A fixture
/// for "logged in to this platform" has to set both, because that is what a
/// real login writes.
fn write_valid_auth_config_for_platform(config_dir: &std::path::Path, platform_url: &str) {
    let platform_url = platform_url.trim_end_matches('/');
    fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"platform_url = "{platform_url}"

[auth]
access_token = "test-token"
refresh_token = "refresh-token"
expires_at = 4102444800
email = "agent@example.com"
issuer_platform_url = "{platform_url}"
"#
        ),
    )
    .expect("write test config");
}

fn write_legacy_short_expiry_jwt_config(config_dir: &std::path::Path) {
    fs::write(
        config_dir.join("config.toml"),
        r#"[auth]
access_token = "e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig"
refresh_token = "refresh-token"
expires_at = 1
email = "agent@example.com"
"#,
    )
    .expect("write legacy test config");
}

fn write_expired_refreshable_auth_config_for_platform(
    config_dir: &std::path::Path,
    platform_url: &str,
) {
    let platform_url = platform_url.trim_end_matches('/');
    fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"platform_url = "{platform_url}"

[auth]
access_token = "expired-token"
refresh_token = "refresh-token"
expires_at = 1
email = "agent@example.com"
issuer_platform_url = "{platform_url}"
"#
        ),
    )
    .expect("write expired test config");
}

fn write_expired_refreshable_auth_config(config_dir: &std::path::Path) {
    write_expired_refreshable_auth_config_for_platform(config_dir, TEST_PLATFORM_URL);
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
fn auth_ensure_json_with_existing_auth_outputs_single_ok_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "ensure",
        ])
        .output()
        .expect("run pcl auth ensure");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["authenticated"], true);
    assert_eq!(envelope["data"]["token_valid"], true);
}

#[test]
fn auth_ensure_default_output_is_human() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "auth",
            "ensure",
        ])
        .output()
        .expect("run pcl auth ensure");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(stdout.starts_with("OK\n"), "{stdout}");
    assert!(stdout.contains("Authentication"), "{stdout}");
    assert!(stdout.contains("Status: authenticated"), "{stdout}");
    assert!(stdout.contains("Time remaining:"), "{stdout}");
    assert!(stdout.contains("Next:"), "{stdout}");
    assert!(!stdout.contains("Details:"), "{stdout}");
    assert!(!stdout.contains("Schema: pcl.envelope.v1"), "{stdout}");
}

#[test]
fn auth_status_human_expired_token_recommends_refresh() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_expired_refreshable_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "auth",
            "status",
        ])
        .output()
        .expect("run pcl auth status");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(stderr.contains("Error\n"), "{stderr}");
    assert!(stderr.contains("Run `pcl auth refresh`"), "{stderr}");
    assert!(
        !stderr.contains("Run `pcl auth login --force` again"),
        "{stderr}"
    );
    assert!(stderr.contains("1. pcl auth refresh"), "{stderr}");
    assert!(!stderr.contains("pcl auth refresh --json"), "{stderr}");
}

#[test]
fn auth_status_json_expired_token_recommends_json_refresh() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_expired_refreshable_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "status",
        ])
        .output()
        .expect("run pcl auth status --json");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    let envelope: serde_json::Value = serde_json::from_str(&stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error", "{envelope}");
    assert_eq!(
        envelope["error"]["code"], "auth.expired_token",
        "{envelope}"
    );
    assert!(
        envelope["next_actions"].as_array().is_some_and(|actions| {
            actions
                .iter()
                .any(|action| action.as_str() == Some("pcl auth refresh --json"))
        }),
        "{envelope}"
    );
}

#[test]
fn auth_ensure_json_without_auth_outputs_login_challenge() {
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

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "ensure",
        ])
        .output()
        .expect("run pcl auth ensure");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "action_required");
    assert_eq!(envelope["data"]["state"], "login_required");
    assert_eq!(envelope["data"]["reason"], "missing_auth");
    assert_eq!(envelope["data"]["requires_user"], true);
    assert_eq!(envelope["data"]["refresh_supported"], true);
    assert!(envelope["data"].get("device_secret").is_none());
    assert!(
        envelope["data"]["poll_command"]
            .as_str()
            .is_some_and(|command| {
                !command.contains("--device-secret") && !command.contains("test_secret")
            })
    );
    assert!(
        envelope["data"]["poll_command"]
            .as_str()
            .expect("poll command")
            .contains(" auth --auth-url")
    );
    assert!(
        envelope["data"]["poll_command"]
            .as_str()
            .expect("poll command")
            .contains("--expires-at")
    );
    auth_code.assert();
}

#[test]
fn auth_ensure_json_falls_back_to_login_when_refresh_endpoint_is_missing() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    write_expired_refreshable_auth_config_for_platform(temp_dir.path(), &server.url());
    let refresh = server
        .mock("POST", "/api/v1/auth/refresh")
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req_refresh_missing")
        .with_body(r#"{"error":"Not Found"}"#)
        .expect(1)
        .create();
    let auth_code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2099-12-31T00:00:00Z"}"#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "ensure",
        ])
        .output()
        .expect("run pcl auth ensure");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "action_required");
    assert_eq!(envelope["data"]["reason"], "refresh_unavailable");
    assert_eq!(envelope["data"]["refresh_supported"], false);
    assert_eq!(envelope["data"]["refresh_attempted"], true);
    assert_eq!(envelope["data"]["code"], "123456");
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(config.contains("[auth]"));
    refresh.assert();
    auth_code.assert();
}

#[test]
fn auth_refresh_json_without_auth_outputs_login_challenge() {
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

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "refresh",
        ])
        .output()
        .expect("run pcl auth refresh");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "action_required");
    assert_eq!(envelope["data"]["reason"], "missing_auth");
    assert_eq!(envelope["data"]["refresh_supported"], true);
    assert!(envelope["data"].get("device_secret").is_none());
    assert!(
        envelope["data"]["poll_command"]
            .as_str()
            .is_some_and(|command| {
                !command.contains("--device-secret") && !command.contains("test_secret")
            })
    );
    auth_code.assert();
}

#[test]
fn auth_refresh_json_with_valid_auth_reports_noop_single_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "refresh",
        ])
        .output()
        .expect("run pcl auth refresh");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["refreshed"], false);
    assert_eq!(envelope["data"]["refresh_supported"], true);
    assert_eq!(envelope["data"]["reason"], "token_still_valid");
}

#[test]
fn auth_status_json_normalizes_legacy_short_expiry_from_jwt_exp() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_legacy_short_expiry_jwt_config(temp_dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "status",
        ])
        .output()
        .expect("run pcl auth status");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["token_valid"], true);
    assert_eq!(envelope["data"]["expires_at"], "2100-01-01T00:00:00+00:00");

    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(config.contains("expires_at = 4102444800"));
}

#[test]
fn auth_login_no_wait_json_outputs_single_challenge() {
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
            "--no-wait",
        ])
        .output()
        .expect("run pcl auth login --no-wait");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "action_required");
    assert!(envelope["data"]["poll_command"].as_str().is_some());
    auth_code.assert();
}

#[test]
fn auth_login_no_wait_uses_pcl_api_url_when_auth_url_is_unset() {
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

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .env("PCL_AUTH_NO_BROWSER", "1")
        .env("PCL_API_URL", server.url())
        .env_remove("PCL_AUTH_URL")
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "login",
            "--no-wait",
        ])
        .output()
        .expect("run pcl auth login --no-wait with PCL_API_URL");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "action_required");
    let expected_device_prefix = format!("{}/device?", server.url());
    assert!(
        envelope["data"]["device_url"]
            .as_str()
            .expect("device_url string")
            .starts_with(&expected_device_prefix)
    );
    assert!(
        envelope["data"]["poll_command"]
            .as_str()
            .expect("poll_command string")
            .contains(&format!("--auth-url={}", server.url()))
    );
    auth_code.assert();
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
        .mock("POST", "/api/v1/cli/auth/status")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "session_id": "550e8400-e29b-41d4-a716-446655440000",
                "device_secret": "test_secret",
            })))
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
    assert_secret_free(&stdout, "JSONL login events");
    assert!(
        pending_device_login_files(temp_dir.path()).is_empty(),
        "a verified waiting login left the pending secret on disk"
    );
    auth_code.assert();
    auth_status.assert();
}

#[test]
fn auth_login_force_starts_fresh_flow_even_with_existing_auth() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());
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
        .mock("POST", "/api/v1/cli/auth/status")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "session_id": "550e8400-e29b-41d4-a716-446655440000",
                "device_secret": "test_secret",
            })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","token":"e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig","refresh_token":"new-refresh-token","email":"agent@example.com"}"#,
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
            "--force",
        ])
        .output()
        .expect("run pcl auth login --force");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 2, "expected JSONL auth events: {stdout}");
    let terminal: serde_json::Value = serde_json::from_str(lines[1]).expect("terminal event");
    assert_eq!(terminal["event"], "auth.login_complete");
    assert_eq!(terminal["data"]["expires_at"], "2100-01-01T00:00:00+00:00");
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(config.contains("access_token = \"e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig\""));
    assert!(config.contains("expires_at = 4102444800"));
    auth_code.assert();
    auth_status.assert();
}

/// The platform that issued the credentials a boundary test starts from.
///
/// `.invalid` never resolves, so a request that escapes to the issuing platform
/// fails loudly instead of silently succeeding against something real.
const OTHER_PLATFORM_URL: &str = "https://platform-a.invalid";

#[test]
fn login_against_another_platform_does_not_short_circuit_on_the_old_token() {
    // Startup persists an explicit `--auth-url` as the remembered platform
    // before the login runs. If the boundary check read that field it would be
    // comparing the target against itself, find no switch, and short-circuit —
    // leaving the old platform's token bound to the new platform for every
    // later command.
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config_for_platform(temp_dir.path(), OTHER_PLATFORM_URL);
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
        .mock("POST", "/api/v1/cli/auth/status")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "session_id": "550e8400-e29b-41d4-a716-446655440000",
                "device_secret": "test_secret",
            })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","token":"token-from-platform-b","refresh_token":"refresh-from-platform-b","email":"agent@example.com"}"#,
        )
        .expect(1)
        .create();
    // Created last: mockito matches in creation order, so this only catches
    // requests the login flow above did not already claim.
    let never_authenticated = server
        .mock("GET", mockito::Matcher::Any)
        .match_header("authorization", "Bearer test-token")
        .with_status(200)
        .expect(0)
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
        .expect("run pcl auth login against another platform");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A real device-auth flow ran rather than "already authenticated".
    auth_code.assert();
    auth_status.assert();
    never_authenticated.assert();

    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(
        config.contains("access_token = \"token-from-platform-b\""),
        "the new platform's token must replace the old one: {config}"
    );
    assert!(
        !config.contains("test-token"),
        "the old platform's token must not survive the switch: {config}"
    );
    assert!(
        config.contains(&format!(
            "issuer_platform_url = \"{}\"",
            server.url().trim_end_matches('/')
        )),
        "the stored credentials must record the platform that issued them: {config}"
    );
}

#[test]
fn credentials_without_a_recorded_issuer_are_refused_against_a_remembered_platform() {
    // The upgrade path, and the state a fresh interactive selection leaves
    // behind: credentials from a release that recorded no issuer, plus a
    // remembered platform written by resolution rather than by a login. The
    // promised one-time re-login only happens if the boundary check refuses to
    // read that remembered value as provenance.
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let never_requested = server
        .mock("GET", mockito::Matcher::Any)
        .with_status(200)
        .expect(0)
        .create();
    fs::write(
        temp_dir.path().join("config.toml"),
        format!(
            r#"platform_url = "{}"

[auth]
access_token = "legacy-token"
refresh_token = "legacy-refresh"
expires_at = 4102444800
email = "agent@example.com"
"#,
            server.url().trim_end_matches('/')
        ),
    )
    .expect("write legacy config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "projects",
            "mine",
        ])
        .output()
        .expect("run pcl projects mine");

    assert!(!output.status.success(), "the command must be refused");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    let envelope: serde_json::Value =
        serde_json::from_str(if stdout.is_empty() { &stderr } else { &stdout })
            .expect("json envelope");
    assert_eq!(envelope["error"]["code"], "auth.platform_mismatch");
    never_requested.assert();
}

#[test]
fn auth_poll_json_verified_stores_auth_and_returns_terminal_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let auth_status = server
        .mock("POST", "/api/v1/cli/auth/status")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "session_id": "550e8400-e29b-41d4-a716-446655440000",
                "device_secret": "test_secret",
            })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","token":"opaque-token","refresh_token":"new-refresh-token","email":"agent@example.com"}"#,
        )
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "poll",
            "--session-id",
            "550e8400-e29b-41d4-a716-446655440000",
            "--device-secret",
            "test_secret",
            "--expires-at",
            "2030-01-01T00:00:00Z",
        ])
        .output()
        .expect("run pcl auth poll");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["event"], "auth.login_complete");
    assert_eq!(envelope["terminal"], true);
    assert_eq!(envelope["data"]["authenticated"], true);
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(
        config.contains("access_token = \"opaque-token\""),
        "config did not contain expected token:\n{config}"
    );
    assert!(
        config.contains("expires_at = 1893456000"),
        "config did not contain fallback expiry:\n{config}"
    );
    auth_status.assert();
}

#[test]
fn auth_poll_json_pending_returns_pending_envelope_without_writing_auth() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let auth_status = server
        .mock("POST", "/api/v1/cli/auth/status")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "device_secret": "test_secret",
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"verified":false}"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "poll",
            "--session-id",
            "550e8400-e29b-41d4-a716-446655440000",
            "--device-secret",
            "test_secret",
        ])
        .output()
        .expect("run pcl auth poll");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "pending");
    assert_eq!(envelope["event"], "auth.login_pending");
    assert_eq!(envelope["terminal"], false);
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).unwrap_or_default();
    assert!(!config.contains("[auth]"));
    auth_status.assert();
}

#[test]
fn auth_logout_json_clears_local_config_after_remote_logout() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    // The credentials must belong to the mock platform for the remote logout
    // to send the token there.
    write_valid_auth_config_for_platform(temp_dir.path(), &server.url());
    let logout = server
        .mock("POST", "/api/v1/web/auth/logout")
        .match_header("authorization", "Bearer test-token")
        .match_header(
            "content-type",
            mockito::Matcher::Regex("application/json.*".to_string()),
        )
        .match_body(mockito::Matcher::Json(serde_json::json!({})))
        .with_status(200)
        .with_header("x-request-id", "req_logout_ok")
        .with_body(r#"{"success":true}"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &server.url(),
            "logout",
        ])
        .output()
        .expect("run pcl auth logout");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["authenticated"], false);
    assert_eq!(
        envelope["data"]["remote_logout"],
        serde_json::json!({
            "attempted": true,
            "success": true,
            "mode": "remote",
            "endpoint": "/api/v1/web/auth/logout",
            "http_status": 200,
            "request_id": "req_logout_ok",
        })
    );
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(!config.contains("[auth]"));
    logout.assert();
}

/// The network prompt is meant to be one-time, so the chosen platform has to be
/// recorded before the command runs. Persisting it only after a successful
/// command loses the choice on any failure and prompts again on the next run.
#[test]
fn login_records_its_platform_even_when_the_login_itself_fails() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");

    // Port 9 (discard) refuses the device-code request, so resolution succeeds
    // and the command then fails.
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "auth",
            "--auth-url",
            "http://127.0.0.1:9",
            "login",
        ])
        .output()
        .expect("run pcl auth login");

    assert!(
        !output.status.success(),
        "login against a dead host should fail: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let config = fs::read_to_string(&config_path).expect("config written despite the failure");
    assert!(
        config.contains(r#"platform_url = "http://127.0.0.1:9""#),
        "the resolved platform must survive a failing command: {config}"
    );
}

/// A failure while persisting the resolved platform still has to arrive as the
/// one JSON envelope a machine caller parses. It used to escape the structured
/// error boundary and print a color-eyre diagnostic instead.
#[test]
fn platform_write_failure_under_json_emits_a_single_error_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config_for_platform(temp_dir.path(), OTHER_PLATFORM_URL);
    // Read and execute but not write: resolution succeeds, then persisting the
    // explicitly requested platform fails.
    let mut permissions = fs::metadata(temp_dir.path())
        .expect("read temp dir metadata")
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(temp_dir.path(), permissions).expect("make config dir read-only");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .env("PCL_AUTH_NO_BROWSER", "1")
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            "http://127.0.0.1:9",
            "login",
        ])
        .output()
        .expect("run pcl auth login");

    // Restore write access so the temp dir can be cleaned up.
    let mut permissions = fs::metadata(temp_dir.path())
        .expect("read temp dir metadata")
        .permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(temp_dir.path(), permissions).expect("restore config dir permissions");

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(
        !stderr.contains("Location:"),
        "a color-eyre diagnostic escaped the error boundary: {stderr}"
    );
    let rendered = if stdout.is_empty() { &stderr } else { &stdout };
    let envelope: serde_json::Value =
        serde_json::from_str(rendered.trim()).expect("a single json envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
}

/// A command allowed to repair an unreadable config must not destroy it before
/// it has actually produced its replacement: a cancelled or failed repair would
/// take recoverable credentials and RPC settings with it.
#[test]
fn a_failed_repair_preserves_the_unreadable_config() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let damaged =
        "platform_url = \"http://127.0.0.1:9\"\n\n[auth]\nexpires_at = \"2099-12-31T00:00:00Z\"\n";
    fs::write(&config_path, damaged).expect("write damaged config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .env("PCL_AUTH_NO_BROWSER", "1")
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            "http://127.0.0.1:9",
            "login",
        ])
        .output()
        .expect("run pcl auth login");

    assert!(
        !output.status.success(),
        "login against a dead host should fail"
    );
    let after = fs::read_to_string(&config_path).expect("read config");
    assert_eq!(
        after, damaged,
        "a failed repair must leave the original file intact"
    );
}

/// A repair that does replace an unreadable config keeps the original bytes
/// alongside it, so credentials or RPC settings inside remain recoverable.
#[test]
fn repairing_an_unreadable_config_preserves_the_original_bytes() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    let damaged = "platform_url = \"https://linea.phylax.systems\"\n\n[auth]\naccess_token = \"recoverable\"\nexpires_at = \"2099-12-31T00:00:00Z\"\n";
    fs::write(&config_path, damaged).expect("write damaged config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "config",
            "delete",
        ])
        .output()
        .expect("run pcl config delete");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let backup = fs::read_to_string(temp_dir.path().join("config.toml.invalid"))
        .expect("the unreadable config is preserved alongside its replacement");
    assert_eq!(backup, damaged);
    assert!(
        !fs::read_to_string(&config_path)
            .expect("read replacement")
            .contains("recoverable"),
        "the replacement config must not carry the unparsed credentials forward"
    );
}

#[test]
fn auth_logout_local_can_repair_invalid_config() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let config_path = temp_dir.path().join("config.toml");
    fs::write(&config_path, "not = [toml\n").expect("write invalid config");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "logout",
            "--local",
        ])
        .output()
        .expect("run pcl auth logout --local");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(
        envelope["data"]["remote_logout"],
        serde_json::json!({
            "attempted": false,
            "success": null,
            "mode": "local",
            "reason": "local_only_requested",
        })
    );
    assert_eq!(fs::read_to_string(config_path).expect("read config"), "");
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
        .expect("run pcl --json --llms");

    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    assert_eq!(envelope["data"]["default_output"], "human");
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
    assert!(!stdout.contains("--config-dir"));

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
            .is_some_and(|script| script.contains("_pcl") && !script.contains("--config-dir"))
    );
    assert_eq!(
        fs::read_to_string(config_path).expect("read invalid config"),
        original_config
    );
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
fn default_error_output_is_human_readable() {
    let config_dir = tempfile::tempdir().expect("config dir");
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--config-dir")
        .arg(config_dir.path())
        .args([
            "api",
            "--api-url",
            "http://127.0.0.1:9",
            "call",
            "get",
            "health",
        ])
        .output()
        .expect("run pcl api call");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr).expect("utf-8 stderr");
    assert!(stderr.contains("Error\n"), "{stderr}");
    assert!(!stderr.contains("Code:"), "{stderr}");
    assert!(stderr.contains("Next:"), "{stderr}");
    assert!(!stderr.contains("Schema: pcl.envelope.v1"), "{stderr}");
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

#[test]
fn json_global_flag_emits_json_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    write_valid_auth_config(temp_dir.path());

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
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
}

#[test]
fn parser_errors_honor_json_before_successful_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--json",
            "api",
            "call",
            "get",
            "/health",
            "--body",
            "{}",
            "--body-file",
            "body.json",
        ])
        .output()
        .expect("run pcl parser error");

    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "cli.argument_conflict");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
}

#[test]
fn api_call_accepts_inline_query_string_under_json() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let health = server
        .mock("GET", "/api/v1/health")
        .match_query(mockito::Matcher::UrlEncoded("limit".into(), "5".into()))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_header("x-request-id", "req-inline-query")
        .with_body(r#"{"healthy":true}"#)
        .expect(1)
        .create();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "api",
            "--api-url",
            &server.url(),
            "--allow-unauthenticated",
            "call",
            "get",
            "/health?limit=5",
        ])
        .output()
        .expect("run pcl api call");

    assert!(
        output.status.success(),
        "api call failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["request"]["query"][0]["name"], "limit");
    assert_eq!(envelope["data"]["request"]["query"][0]["value"], "5");
    assert_eq!(
        envelope["data"]["response"]["request_id"],
        "req-inline-query"
    );
    health.assert();
}

/// Runs a command the CLI printed exactly as a user would paste it: through
/// the shell, with `pcl` resolved from PATH and a HOME that holds no config,
/// so the command only works if it names everything it needs.
#[cfg(unix)]
fn run_emitted_command(command: &str) -> std::process::Output {
    let bin_dir = std::path::Path::new(env!("CARGO_BIN_EXE_pcl"))
        .parent()
        .expect("pcl binary directory");
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let empty_home = tempfile::tempdir().expect("create empty home");
    Command::new("sh")
        .args(["-c", command])
        .env("PATH", path)
        .env("HOME", empty_home.path())
        .env("XDG_CONFIG_HOME", empty_home.path())
        .env("PCL_AUTH_NO_BROWSER", "1")
        // A command that does not name its platform must fail here, not
        // reach a real one.
        .env_remove("PCL_AUTH_URL")
        .env("PCL_API_URL", "http://127.0.0.1:9")
        .output()
        .expect("run emitted command")
}

fn device_login_challenge_body() -> &'static str {
    r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2099-12-31T00:00:00Z"}"#
}

fn run_ensure(config_dir: &std::path::Path, auth_url: &str, json: bool) -> std::process::Output {
    let mut args = vec![
        "--config-dir",
        config_dir.to_str().expect("utf-8 temp path"),
    ];
    if json {
        args.push("--json");
    }
    args.extend(["auth", "--auth-url", auth_url, "ensure"]);
    Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(args)
        .output()
        .expect("run pcl auth ensure")
}

#[test]
fn auth_ensure_login_output_omits_device_secret() {
    for json in [true, false] {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let mut server = mockito::Server::new();
        let auth_code = server
            .mock("GET", "/api/v1/cli/auth/code")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(device_login_challenge_body())
            .expect(1)
            .create();

        let output = run_ensure(temp_dir.path(), &server.url(), json);
        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !rendered.contains("test_secret"),
            "login output leaked the device secret: {rendered}"
        );
        assert!(
            !rendered.contains("--device-secret"),
            "login output included a secret-bearing poll command: {rendered}"
        );
        if json {
            let envelope: serde_json::Value =
                serde_json::from_slice(&output.stdout).expect("json envelope");
            assert!(envelope["data"].get("device_secret").is_none());
            assert!(
                envelope["data"]["poll_command"]
                    .as_str()
                    .is_some_and(|command| !command.contains("--device-secret"))
            );
        }
        auth_code.assert();
    }
}

#[cfg(unix)]
#[test]
fn auth_ensure_stores_pending_device_secret_owner_only() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let auth_code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(device_login_challenge_body())
        .expect(1)
        .create();

    let output = run_ensure(temp_dir.path(), &server.url(), true);
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut pending = Vec::new();
    for entry in fs::read_dir(temp_dir.path()).expect("read config dir") {
        let path = entry.expect("config dir entry").path();
        if fs::read_to_string(&path).ok().as_deref() == Some("test_secret") {
            pending.push(path);
        }
    }
    assert_eq!(
        pending.len(),
        1,
        "expected one owner-only file holding the pending device secret"
    );
    let mode = fs::metadata(&pending[0])
        .expect("pending secret metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "pending device secret file must be mode 0600");
    auth_code.assert();
}

#[cfg(unix)]
#[test]
fn auth_poll_reads_pending_device_secret_without_cli_flag() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let mut server = mockito::Server::new();
    let auth_code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(device_login_challenge_body())
        .expect(1)
        .create();
    let auth_status = server
        .mock("POST", "/api/v1/cli/auth/status")
        .match_body(mockito::Matcher::Json(serde_json::json!({
            "session_id": "550e8400-e29b-41d4-a716-446655440000",
            "device_secret": "test_secret",
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"verified":false}"#)
        .expect(1)
        .create();

    let ensured = run_ensure(temp_dir.path(), &server.url(), true);
    assert!(
        ensured.status.success(),
        "ensure failed: {}",
        String::from_utf8_lossy(&ensured.stderr)
    );
    let challenge: serde_json::Value =
        serde_json::from_slice(&ensured.stdout).expect("ensure envelope");
    let poll_command = challenge["data"]["poll_command"]
        .as_str()
        .expect("poll command");
    assert!(!poll_command.contains("--device-secret"));
    assert!(!poll_command.contains("test_secret"));

    let output = run_emitted_command(poll_command);
    assert!(
        output.status.success(),
        "poll without --device-secret failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = String::from_utf8_lossy(&output.stdout);
    assert!(!rendered.contains("test_secret"));
    auth_code.assert();
    auth_status.assert();
}

const PENDING_DEVICE_LOGIN_FILE: &str = "device-login-550e8400-e29b-41d4-a716-446655440000";

fn device_login_code_server() -> (mockito::ServerGuard, mockito::Mock) {
    let mut server = mockito::Server::new();
    let auth_code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(device_login_challenge_body())
        .create();
    (server, auth_code)
}

/// A file planted at the pending-secret path (a symlink, or a file someone
/// already holds open) must never receive the secret.
#[cfg(unix)]
#[test]
fn auth_ensure_never_writes_the_secret_through_a_planted_path() {
    use std::io::Read;

    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let target = temp_dir.path().join("elsewhere");
    fs::write(&target, "").expect("write symlink target");
    std::os::unix::fs::symlink(&target, temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE))
        .expect("plant symlink");
    let (server, _auth_code) = device_login_code_server();

    let output = run_ensure(temp_dir.path(), &server.url(), true);
    assert!(
        !output.status.success(),
        "ensure must refuse a planted pending-secret path"
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read symlink target"),
        "",
        "the secret was written through a planted symlink"
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(rendered.contains("already exists"), "{rendered}");

    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let planted = temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE);
    fs::write(&planted, "").expect("plant file");
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o644)).expect("chmod planted");
    let mut reader = fs::File::open(&planted).expect("hold planted file open");
    let output = run_ensure(temp_dir.path(), &server.url(), true);
    assert!(!output.status.success());
    let mut leaked = String::new();
    reader
        .read_to_string(&mut leaked)
        .expect("read planted file");
    assert_eq!(leaked, "", "an fd opened before ensure read the secret");
}

#[cfg(unix)]
#[test]
fn auth_ensure_creates_a_missing_config_dir_owner_only() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let config_dir = temp_dir.path().join("new").join("pcl");
    let (server, _auth_code) = device_login_code_server();

    let output = run_ensure(&config_dir, &server.url(), true);
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mode = fs::metadata(&config_dir)
        .expect("config dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode, 0o700,
        "a config dir created for the secret must be 0700"
    );
    assert!(config_dir.join(PENDING_DEVICE_LOGIN_FILE).exists());

    // An existing user dir keeps its mode.
    let existing = tempfile::tempdir().expect("create temp dir");
    fs::set_permissions(existing.path(), fs::Permissions::from_mode(0o755)).expect("chmod");
    let output = run_ensure(existing.path(), &server.url(), true);
    assert!(output.status.success());
    let mode = fs::metadata(existing.path())
        .expect("existing dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "an existing config dir must not be chmodded");
}

fn run_poll(config_dir: &std::path::Path, auth_url: &str, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "--config-dir",
        config_dir.to_str().expect("utf-8 temp path"),
        "--json",
        "auth",
        "--auth-url",
        auth_url,
        "poll",
        "--session-id",
        "550e8400-e29b-41d4-a716-446655440000",
        "--expires-at",
        "2099-12-31T00:00:00Z",
    ];
    args.extend(extra);
    Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(args)
        .output()
        .expect("run pcl auth poll")
}

fn verified_status_mock(server: &mut mockito::ServerGuard) -> mockito::Mock {
    server
        .mock("POST", "/api/v1/cli/auth/status")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","token":"opaque-token","refresh_token":"new-refresh-token","email":"agent@example.com"}"#,
        )
        .create()
}

/// An unreadable pending file is not "no pending login": the real I/O error
/// must reach the caller.
#[test]
fn auth_poll_reports_why_the_pending_secret_could_not_be_read() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    fs::create_dir(temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE)).expect("plant dir");
    let server = mockito::Server::new();

    let output = run_poll(temp_dir.path(), &server.url(), &[]);
    assert!(!output.status.success());
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !rendered.contains("No pending device login"),
        "a read failure was reported as a missing login: {rendered}"
    );
    assert!(rendered.contains("directory"), "{rendered}");
}

/// A pending secret that could not be removed after a successful claim is
/// reported, not silently left on disk.
#[test]
fn auth_poll_warns_when_the_pending_secret_cannot_be_removed() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let stuck = temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE);
    fs::create_dir(&stuck).expect("plant dir");
    fs::write(stuck.join("keep"), "").expect("make dir non-empty");
    let mut server = mockito::Server::new();
    let _status = verified_status_mock(&mut server);

    let output = run_poll(
        temp_dir.path(),
        &server.url(),
        &["--device-secret", "test_secret"],
    );
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    let warnings = envelope["warnings"].as_array().cloned().unwrap_or_default();
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "auth.pending_device_login_cleanup_failed"),
        "missing cleanup warning: {envelope}"
    );
}

fn pending_device_login_files(config_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(config_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("device-login-"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The human blocking login prints no poll command, so it has no reason to
/// leave the secret on disk while it waits.
#[test]
fn auth_login_human_blocking_does_not_store_the_pending_secret() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let (mut server, _auth_code) = device_login_code_server();
    let status = server
        .mock("POST", "/api/v1/cli/auth/status")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"verified":false}"#)
        .create();

    let mut child = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .env("PCL_AUTH_NO_BROWSER", "1")
        .args([
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "auth",
            "--auth-url",
            &server.url(),
            "login",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn pcl auth login");
    let started = std::time::Instant::now();
    while !status.matched() && started.elapsed() < std::time::Duration::from_secs(20) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let pending = pending_device_login_files(temp_dir.path());
    let _ = child.kill();
    let _ = child.wait();
    assert!(status.matched(), "login never polled");
    assert!(
        pending.is_empty(),
        "human blocking login stored a pending secret it never prints a poll command for: {pending:?}"
    );
}

fn status_error_mock(
    server: &mut mockito::ServerGuard,
    status: usize,
    body: &str,
) -> mockito::Mock {
    server
        .mock("POST", "/api/v1/cli/auth/status")
        .with_status(status)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create()
}

/// A poll that ends the session for good removes the pending secret; a
/// transient failure keeps it so the next poll can still claim the login.
#[test]
fn auth_poll_clears_the_pending_secret_only_on_terminal_failures() {
    let cases = [
        (400, r#"{"error":"expired","code":"SESSION_EXPIRED"}"#, true),
        (
            400,
            r#"{"error":"missing","code":"SESSION_NOT_FOUND"}"#,
            true,
        ),
        (400, r#"{"error":"no user","code":"USER_NOT_FOUND"}"#, true),
        (500, r#"{"error":"boom","code":"INTERNAL_ERROR"}"#, false),
    ];
    for (status, body, terminal) in cases {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let pending = temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE);
        fs::write(&pending, "test_secret").expect("write pending secret");
        let mut server = mockito::Server::new();
        let _status = status_error_mock(&mut server, status, body);

        let output = run_poll(temp_dir.path(), &server.url(), &[]);
        assert!(!output.status.success(), "HTTP {status} {body} must fail");
        assert_eq!(
            !pending.exists(),
            terminal,
            "HTTP {status} {body}: pending secret removed must be {terminal}"
        );
    }

    // A network failure is transient too.
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let pending = temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE);
    fs::write(&pending, "test_secret").expect("write pending secret");
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", closed.local_addr().expect("addr"));
    drop(closed);
    let output = run_poll(temp_dir.path(), &url, &[]);
    assert!(!output.status.success());
    assert!(
        pending.exists(),
        "a network failure removed the pending secret"
    );
}

/// The waiting JSONL login removes the pending secret when the wait ends in
/// a terminal failure.
#[test]
fn auth_login_json_clears_the_pending_secret_on_terminal_failure() {
    for (code_body, status_body) in [
        (
            device_login_challenge_body().to_string(),
            r#"{"error":"missing","code":"SESSION_NOT_FOUND"}"#,
        ),
        (
            device_login_challenge_body().replace("2099-12-31", "2020-01-01"),
            r#"{"verified":false}"#,
        ),
    ] {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let mut server = mockito::Server::new();
        let _auth_code = server
            .mock("GET", "/api/v1/cli/auth/code")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&code_body)
            .create();
        let _status = status_error_mock(&mut server, 400, status_body);

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
        assert!(!output.status.success(), "{code_body} must fail");
        assert!(
            pending_device_login_files(temp_dir.path()).is_empty(),
            "terminal login failure left the pending secret ({code_body})"
        );
    }
}

/// Logging out ends every pending device login for this config dir, remote
/// or --local, and reports a file it could not remove.
#[test]
fn auth_logout_removes_pending_device_logins() {
    for local in [false, true] {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        let mut server = mockito::Server::new();
        write_valid_auth_config_for_platform(temp_dir.path(), &server.url());
        let _logout = server
            .mock("POST", "/api/v1/web/auth/logout")
            .with_status(200)
            .with_body(r#"{"success":true}"#)
            .create();
        fs::write(
            temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE),
            "test_secret",
        )
        .expect("write pending secret");
        fs::write(
            temp_dir
                .path()
                .join("device-login-11111111-1111-1111-1111-111111111111"),
            "other_secret",
        )
        .expect("write second pending secret");
        let stuck = temp_dir
            .path()
            .join("device-login-22222222-2222-2222-2222-222222222222");
        fs::create_dir(&stuck).expect("plant dir");
        fs::write(stuck.join("keep"), "").expect("make dir non-empty");

        let url = server.url();
        let mut args = vec![
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
            "--json",
            "auth",
            "--auth-url",
            &url,
            "logout",
        ];
        if local {
            args.push("--local");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
            .args(args)
            .output()
            .expect("run pcl auth logout");
        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            pending_device_login_files(temp_dir.path()),
            vec![stuck.clone()],
            "logout (local: {local}) left pending device secrets"
        );
        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("json envelope");
        let warnings = envelope["warnings"].as_array().cloned().unwrap_or_default();
        assert!(
            warnings.iter().any(|warning| {
                warning["code"] == "auth.pending_device_login_cleanup_failed"
                    && warning["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("22222222"))
            }),
            "logout (local: {local}) hid a removal failure: {envelope}"
        );
    }
}

/// --device-secret stays accepted for poll commands printed by released pcl
/// versions, but is no longer advertised.
#[test]
fn auth_poll_help_does_not_advertise_device_secret() {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["auth", "poll", "--help"])
        .output()
        .expect("run pcl auth poll --help");
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--session-id"), "{help}");
    assert!(!help.contains("--device-secret"), "{help}");
}

fn assert_secret_free(rendered: &str, what: &str) {
    assert!(
        !rendered.contains("test_secret"),
        "{what} leaked the device secret: {rendered}"
    );
    assert!(
        !rendered.contains("--device-secret"),
        "{what} printed a secret-bearing poll command: {rendered}"
    );
}

/// Every login challenge output, human and --json, keeps the device secret
/// out: login --no-wait, ensure and the refresh challenge.
#[test]
fn login_challenge_outputs_never_include_the_device_secret() {
    for subcommand in [&["login", "--no-wait"][..], &["ensure"], &["refresh"]] {
        for json in [false, true] {
            let temp_dir = tempfile::tempdir().expect("create temp config dir");
            let (server, _auth_code) = device_login_code_server();
            let url = server.url();
            let mut args = vec![
                "--config-dir",
                temp_dir.path().to_str().expect("utf-8 temp path"),
            ];
            if json {
                args.push("--json");
            }
            args.extend(["auth", "--auth-url", &url]);
            args.extend(subcommand);
            let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
                .env("PCL_AUTH_NO_BROWSER", "1")
                .args(&args)
                .output()
                .expect("run pcl auth");
            assert!(
                output.status.success(),
                "{args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let rendered = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(rendered.contains("poll"), "{args:?}: {rendered}");
            assert_secret_free(&rendered, &format!("{args:?}"));
        }
    }
}

/// A verified poll from the emitted command claims the login and removes the
/// pending secret.
#[cfg(unix)]
#[test]
fn auth_poll_verified_removes_the_pending_secret() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let (mut server, _auth_code) = device_login_code_server();
    let _status = verified_status_mock(&mut server);

    let ensured = run_ensure(temp_dir.path(), &server.url(), true);
    assert!(ensured.status.success());
    assert_eq!(pending_device_login_files(temp_dir.path()).len(), 1);
    let challenge: serde_json::Value =
        serde_json::from_slice(&ensured.stdout).expect("ensure envelope");
    let output = run_emitted_command(
        challenge["data"]["poll_command"]
            .as_str()
            .expect("poll command"),
    );
    assert!(
        output.status.success(),
        "poll failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("poll envelope");
    assert_eq!(envelope["event"], "auth.login_complete");
    assert!(
        pending_device_login_files(temp_dir.path()).is_empty(),
        "a verified poll left the pending secret on disk"
    );
}

/// The JSON `wait_command` must target the same config dir and platform as the
/// challenge that printed it.
#[cfg(unix)]
#[test]
fn auth_ensure_json_wait_command_targets_the_originating_dir_and_platform() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    let (mut server, auth_code) = device_login_code_server();
    let _status = verified_status_mock(&mut server);

    let ensured = run_ensure(temp_dir.path(), &server.url(), true);
    assert!(ensured.status.success());
    // A fresh login gets a fresh session.
    auth_code.remove();
    let _next_code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(device_login_challenge_body().replace("550e8400", "660e8400"))
        .create();
    let challenge: serde_json::Value =
        serde_json::from_slice(&ensured.stdout).expect("ensure envelope");
    let wait_command = challenge["data"]["wait_command"]
        .as_str()
        .expect("wait command");

    let output = run_emitted_command(wait_command);
    assert!(
        output.status.success(),
        "wait_command {wait_command} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let config = fs::read_to_string(temp_dir.path().join("config.toml")).expect("read config");
    assert!(
        config.contains("access_token = \"opaque-token\""),
        "wait_command did not log in to the originating config dir:\n{config}"
    );
}

/// Whatever command first creates the config dir (a config write, a logged
/// API request) creates it owner-only.
#[cfg(unix)]
#[test]
fn commands_create_a_missing_config_dir_owner_only() {
    let mut server = mockito::Server::new();
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{}")
        .create();
    let url = server.url();
    let cases: [&[&str]; 2] = [
        // Records the platform before the (failing) login runs.
        &["auth", "--auth-url", "http://127.0.0.1:9", "login"],
        &[
            "api",
            "call",
            "get",
            "/health",
            "--allow-unauthenticated",
            "--api-url",
            &url,
        ],
    ];
    for command in cases {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let config_dir = temp_dir.path().join("new").join("pcl");
        let mut args = vec![
            "--config-dir",
            config_dir.to_str().expect("utf-8 temp path"),
            "--json",
        ];
        args.extend(command);
        let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
            .env_remove("PCL_REQUEST_LOG")
            .args(&args)
            .output()
            .expect("run pcl");
        assert!(
            config_dir.exists(),
            "{command:?} did not create the config dir: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let mode = fs::metadata(&config_dir)
            .expect("config dir metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "{command:?} created the config dir {mode:o}");
    }
}

/// A legacy GET whose error body echoes its request target must not put the
/// device secret in the error output, human or --json.
#[test]
fn auth_poll_error_output_withholds_an_echoed_device_secret() {
    for json in [true, false] {
        let temp_dir = tempfile::tempdir().expect("create temp config dir");
        fs::write(
            temp_dir.path().join(PENDING_DEVICE_LOGIN_FILE),
            "test_secret",
        )
        .expect("write pending secret");
        let mut server = mockito::Server::new();
        let _post = server
            .mock("POST", "/api/v1/cli/auth/status")
            .with_status(405)
            .create();
        let _get = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::Any)
            .with_status(400)
            .with_body(
                "upstream failure at /api/v1/cli/auth/status?device_secret=test_secret&session_id=550e8400-e29b-41d4-a716-446655440000",
            )
            .create();
        let url = server.url();
        let mut args = vec![
            "--config-dir",
            temp_dir.path().to_str().expect("utf-8 temp path"),
        ];
        if json {
            args.push("--json");
        }
        args.extend([
            "auth",
            "--auth-url",
            &url,
            "poll",
            "--session-id",
            "550e8400-e29b-41d4-a716-446655440000",
        ]);
        let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
            .args(&args)
            .output()
            .expect("run pcl auth poll");
        assert!(!output.status.success());
        let rendered = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !rendered.contains("test_secret"),
            "poll error output (json: {json}) leaked the device secret: {rendered}"
        );
    }
}
