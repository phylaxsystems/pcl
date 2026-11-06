//! CLI process tests against fixtures, not proof of deployed dapp authorization.
use mockito::{
    Matcher,
    Server,
};
use serde_json::{
    Value,
    json,
};
use std::{
    fs,
    path::Path,
    process::Command,
};

const PROJECT: &str = "00000000-0000-4000-8000-000000000001";
const SESSION: &str = "550e8400-e29b-41d4-a716-446655440000";

fn run(config: &Path, platform: &str, args: &[&str], success: bool) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .env("PCL_API_URL", platform)
        .env_remove("PCL_AUTH_URL")
        .env("PCL_AUTH_NO_BROWSER", "1")
        .arg("--config-dir")
        .arg(config)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert_eq!(output.status.success(), success, "{args:?}: {output:?}");
    let bytes = if success {
        &output.stdout
    } else {
        &output.stderr
    };
    serde_json::from_slice(bytes).expect("single JSON envelope")
}

fn stored(config: &Path) -> String {
    fs::read_to_string(config.join("config.toml")).unwrap_or_default()
}

fn seed(config: &Path, platform: &str, expiry: i64) {
    fs::write(
        config.join("config.toml"),
        format!(
            r#"platform_url = "{platform}"
[auth]
access_token = "old-access"
refresh_token = "old-refresh"
issuer_platform_url = "{platform}"
expires_at = {expiry}
"#
        ),
    )
    .unwrap();
}

fn approve_session(config: &Path, server: &mut Server, url: &str) {
    let code = server
        .mock("GET", "/api/v1/cli/auth/code")
        .with_header("content-type", "application/json")
        .with_body(
            json!({"code":"123456", "sessionId":SESSION,
            "deviceSecret":"device-secret", "expiresAt":"2099-01-01T00:00:00Z"})
            .to_string(),
        )
        .expect(1)
        .create();
    let challenge = run(config, url, &["auth", "ensure"], true);
    assert_eq!(challenge["status"], "action_required");
    assert_eq!(challenge["data"]["requires_user"], true);
    assert!(!stored(config).contains("[auth]"));
    code.assert();

    // Simulate approval; the opaque access token's fallback expiry makes it due
    // for renewal without modifying credentials between CLI invocations.
    let approved = server
        .mock("GET", "/api/v1/cli/auth/status")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("session_id".into(), SESSION.into()),
            Matcher::UrlEncoded("device_secret".into(), "device-secret".into()),
        ]))
        .with_header("content-type", "application/json")
        .with_body(
            json!({"verified":true, "user_id":SESSION,
            "token":"old-access", "refresh_token":"old-refresh"})
            .to_string(),
        )
        .expect(1)
        .create();
    let expiry = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc3339();
    let login = run(
        config,
        url,
        &[
            "auth",
            "poll",
            "--session-id",
            SESSION,
            "--device-secret",
            "device-secret",
            "--expires-at",
            &expiry,
        ],
        true,
    );
    assert_eq!(login["data"]["authenticated"], true);
    approved.assert();
}

#[test]
fn approved_session_refreshes_reads_private_trace_and_logs_out() {
    let config = tempfile::tempdir().unwrap();
    let mut server = Server::new();
    let url = server.url();
    approve_session(config.path(), &mut server, &url);

    let refresh = server
        .mock("POST", "/api/v1/auth/refresh")
        .match_header("authorization", Matcher::Missing)
        .match_body(Matcher::Json(json!({"refresh_token":"old-refresh"})))
        .with_header("content-type", "application/json")
        .with_body(
            json!({"token":"new-access", "refresh_token":"new-refresh",
            "expires_at":"2099-01-01T00:00:00Z", "refresh_expires_at":"2099-02-01T00:00:00Z"})
            .to_string(),
        )
        .expect(1)
        .create();
    let renewed = run(config.path(), &url, &["auth", "refresh"], true);
    assert_eq!(renewed["data"]["refreshed"], true);
    assert!(stored(config.path()).contains("new-refresh"));
    assert!(!stored(config.path()).contains("old-refresh"));
    refresh.assert();

    let detail = server
        .mock("GET", "/api/v1/views/invalidations/incident")
        .match_header("authorization", "Bearer new-access")
        .match_query(Matcher::UrlEncoded("project_id".into(), PROJECT.into()))
        .with_header("content-type", "application/json")
        .with_body(json!({"data":{"invalidating_transactions":[{"id":"tx"}]}}).to_string())
        .expect(1)
        .create();
    let trace_body = json!({"data":{"debug_trace":{"structured_trace":{
        "decoded":{"name":"LimitExceeded", "value":"9007199254740993"}},
        "payload_budget":{"truncated":false}}}, "_meta":{"request_id":"trace-meta"}});
    let trace = server
        .mock(
            "GET",
            "/api/v1/views/invalidations/incident/transactions/tx/trace",
        )
        .match_header("authorization", "Bearer new-access")
        .match_query(Matcher::UrlEncoded("project_id".into(), PROJECT.into()))
        .with_header("content-type", "application/json")
        .with_body(trace_body.to_string())
        .expect(1)
        .create();
    let context = run(
        config.path(),
        &url,
        &[
            "incidents",
            "--project",
            PROJECT,
            "--incident-id",
            "incident",
            "--include-traces",
        ],
        true,
    );
    assert_eq!(context["trace_context"]["complete"], true);
    assert_eq!(
        context["trace_context"]["transactions"][0]["result"]["data"],
        trace_body
    );
    detail.assert();
    trace.assert();

    let logout = server
        .mock("POST", "/api/v1/web/auth/logout")
        .match_header("authorization", "Bearer new-access")
        .match_body(Matcher::Json(json!({})))
        .with_header("content-type", "application/json")
        .with_body(r#"{"success":true}"#)
        .expect(1)
        .create();
    let logged_out = run(config.path(), &url, &["auth", "logout"], true);
    assert_eq!(logged_out["data"]["remote_logout"]["success"], true);
    assert!(!stored(config.path()).contains("[auth]"));
    let denied = run(
        config.path(),
        &url,
        &[
            "incidents",
            "--project",
            PROJECT,
            "--incident-id",
            "incident",
            "--include-traces",
        ],
        false,
    );
    assert_eq!(denied["status"], "error");
    logout.assert();
    detail.assert(); // No second protected request after logout.
    trace.assert();
}

#[test]
fn denied_incident_never_fetches_traces_or_falls_back_to_public_access() {
    for (status, expiry, detail_requests, refresh_requests) in [
        (403, 4_102_444_800, 1, 0), // Membership denied: no refresh/retry.
        (401, 4_102_444_800, 1, 1), // Revoked access, then rejected refresh.
        (401, 1, 0, 1),             // Expired access: reject refresh before protected GET.
    ] {
        let config = tempfile::tempdir().unwrap();
        let mut server = Server::new();
        let url = server.url();
        seed(config.path(), &url, expiry);
        let before = stored(config.path());
        let detail = server
            .mock("GET", "/api/v1/views/invalidations/incident")
            .match_query(Matcher::UrlEncoded("project_id".into(), PROJECT.into()))
            .match_header("authorization", "Bearer old-access")
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "denied-detail")
            .with_body(r#"{"error":"access denied"}"#)
            .expect(detail_requests)
            .create();
        let refresh = server
            .mock("POST", "/api/v1/auth/refresh")
            .match_body(Matcher::Json(json!({"refresh_token":"old-refresh"})))
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "denied-refresh")
            .with_body(r#"{"error":"session revoked","code":"INVALID_REFRESH_TOKEN"}"#)
            .expect(refresh_requests)
            .create();
        let other = server
            .mock(
                "GET",
                Matcher::Regex(
                    "/api/v1/views/(public/.*|invalidations/incident/transactions/.*)".into(),
                ),
            )
            .expect(0)
            .create();
        let artifact = config.path().join("context.json");
        fs::write(&artifact, "previous evidence").unwrap();
        let denied = run(
            config.path(),
            &url,
            &[
                "incidents",
                "--project",
                PROJECT,
                "--incident-id",
                "incident",
                "--include-traces",
                "--output",
                artifact.to_str().unwrap(),
            ],
            false,
        );
        assert_eq!(denied["status"], "error");
        if status == 403 {
            assert_eq!(denied["error"]["http"]["status"], status);
            assert_eq!(denied["error"]["request_id"], "denied-detail");
        } else {
            assert_eq!(denied["error"]["code"], "auth.refresh_failed");
        }
        assert_eq!(fs::read_to_string(artifact).unwrap(), "previous evidence");
        if status == 403 {
            assert_eq!(stored(config.path()), before);
        } else {
            assert!(!stored(config.path()).contains("[auth]"));
        }
        detail.assert();
        refresh.assert();
        other.assert();
    }
}

#[test]
fn failed_logout_retains_credentials_until_confirmed_or_explicit_local_cleanup() {
    for (status, body) in [
        (
            401,
            r#"{"error":"not revoked","code":"APP_SESSION_NOT_REVOKED"}"#,
        ),
        (500, r#"{"error":"revocation write failed"}"#),
        (200, r#"{"success":false}"#),
        (200, "not JSON"),
    ] {
        let config = tempfile::tempdir().unwrap();
        let mut server = Server::new();
        let url = server.url();
        seed(config.path(), &url, 4_102_444_800);
        let before = stored(config.path());
        let logout = server
            .mock("POST", "/api/v1/web/auth/logout")
            .match_header("authorization", "Bearer old-access")
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "logout-failed")
            .with_body(body)
            .expect(1)
            .create();
        let failed = run(config.path(), &url, &["auth", "logout"], false);
        assert_eq!(failed["error"]["code"], "auth.logout_failed");
        assert_eq!(failed["error"]["local_credentials_removed"], false);
        if status == 401 {
            assert_eq!(
                failed["error"]["remote_logout"]["error_code"],
                "APP_SESSION_NOT_REVOKED"
            );
            assert!(
                failed["next_actions"][0]
                    .as_str()
                    .unwrap()
                    .contains("auth refresh --force")
            );
        }
        assert_eq!(stored(config.path()), before);
        logout.assert();
        logout.remove();
        let success = server
            .mock("POST", "/api/v1/web/auth/logout")
            .match_header("authorization", "Bearer old-access")
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true}"#)
            .expect(1)
            .create();
        let retried = run(config.path(), &url, &["auth", "logout"], true);
        assert_eq!(retried["data"]["local_credentials_removed"], true);
        assert!(!stored(config.path()).contains("[auth]"));
        success.assert();
        seed(config.path(), &url, 4_102_444_800);
        let local = run(config.path(), &url, &["auth", "logout", "--local"], true);
        assert_eq!(local["data"]["remote_logout"]["attempted"], false);
        assert!(!stored(config.path()).contains("[auth]"));
        success.assert(); // Explicit local cleanup sends no further revocation.
    }
}

#[test]
fn logout_rejects_platform_mismatch_without_sending_credentials() {
    let config = tempfile::tempdir().unwrap();
    let mut server = Server::new();
    seed(config.path(), "https://other.example", 4_102_444_800);
    let before = stored(config.path());
    let remote = server
        .mock("POST", "/api/v1/web/auth/logout")
        .expect(0)
        .create();
    let denied = run(config.path(), &server.url(), &["auth", "logout"], false);
    assert_eq!(denied["error"]["code"], "auth.platform_mismatch");
    assert_eq!(stored(config.path()), before);
    let local = run(
        config.path(),
        &server.url(),
        &["auth", "logout", "--local"],
        true,
    );
    assert_eq!(local["data"]["remote_logout"]["attempted"], false);
    assert!(!stored(config.path()).contains("[auth]"));
    remote.assert();
}
