use serde_json::{
    Value,
    json,
};
use std::{
    fs,
    process::Command,
};

const TOKEN: &str = "cl_api_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CURRENT: &str = "/api/v1/auth/api-tokens/current";

// Every case gets an isolated credential/config directory and checks that the
// credential appears in neither output nor persistent interactive auth state.
fn run(server: &mockito::Server, args: &[&str], token: Option<&str>) -> Value {
    run_with_session(server, args, token, None)
}

fn run_with_session(
    server: &mockito::Server,
    args: &[&str],
    token: Option<&str>,
    expires_at: Option<i64>,
) -> Value {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("credential.json");
    if let Some(token) = token {
        fs::write(
            &file,
            serde_json::to_vec(&json!({
                "platform_url": server.url(), "token": token
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let config_dir = tmp.path().join("config");
    fs::create_dir_all(&config_dir).unwrap();
    let saved = expires_at.map(|expiry| {
        format!(
            r#"platform_url = "{url}"
[auth]
access_token = "saved-broad-session"
refresh_token = "saved-refresh"
expires_at = {expiry}
issuer_platform_url = "{url}"
"#,
            url = server.url()
        )
    });
    if let Some(saved) = &saved {
        fs::write(config_dir.join("config.toml"), saved).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(args)
        .args(["--json", "--config-dir"])
        .arg(tmp.path().join("config"))
        .env("PCL_API_TOKEN_FILE", &file)
        .env("PCL_API_URL", server.url())
        .env("PCL_AUTH_URL", server.url())
        .env("PCL_AUTH_NO_BROWSER", "1")
        .output()
        .unwrap();
    let envelope: Value = serde_json::from_slice(if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    })
    .unwrap();
    assert_eq!(output.status.success(), envelope["status"] == "ok");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
    let config = fs::read_to_string(tmp.path().join("config/config.toml")).unwrap_or_default();
    assert!(!config.contains(TOKEN));
    if let Some(saved) = saved {
        assert_eq!(config, saved);
    } else {
        assert!(!config.contains("[auth]"));
    }
    envelope
}

#[test]
fn auth_commands_validate_injected_token_without_a_saved_session() {
    for command in ["ensure", "status", "login", "refresh"] {
        let mut server = mockito::Server::new();
        let check = server.mock("GET", CURRENT)
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .with_status(200).with_header("content-type", "application/json")
            .with_body(r#"{"authenticated":true,"project_id":"project","scope":"invalidations:read","expires_at":"2099-01-01"}"#).create();
        let envelope = run(&server, &["auth", command], Some(TOKEN));
        assert_eq!(envelope["status"], "ok");
        assert_eq!(envelope["data"]["authenticated"], true);
        check.assert();
    }
}

#[test]
fn bad_token_file_never_starts_device_login() {
    let mut server = mockito::Server::new();
    let login = server.mock("GET", mockito::Matcher::Any).expect(0).create();
    for token in [None, Some("malformed")] {
        assert_eq!(
            run(&server, &["auth", "ensure"], token)["error"]["code"],
            "auth.api_token_invalid"
        );
    }
    login.assert();
}

#[test]
fn rejected_or_redirected_token_never_falls_back_to_login() {
    let project = "11111111-1111-4111-8111-111111111111";
    for (args, path, code) in [
        (
            vec!["auth", "ensure"],
            CURRENT.to_string(),
            "auth.api_token_invalid",
        ),
        (
            vec!["download", "--project-id", project],
            format!("/api/v1/projects/{project}"),
            "download.api_failed",
        ),
    ] {
        for status in [401, 302] {
            let mut server = mockito::Server::new();
            let check = server
                .mock("GET", path.as_str())
                .match_query(mockito::Matcher::Any)
                .match_header("authorization", format!("Bearer {TOKEN}").as_str())
                .with_status(status)
                .with_header("location", &format!("{}/unexpected", server.url()))
                .expect(1)
                .create();
            let fallback = server.mock("GET", "/unexpected").expect(0).create();
            assert_eq!(run(&server, &args, Some(TOKEN))["error"]["code"], code);
            check.assert();
            fallback.assert();
        }
    }
}

#[test]
fn incidents_use_injected_token_and_do_not_refresh_after_rejection() {
    for status in [200, 401] {
        let mut server = mockito::Server::new();
        let project = "11111111-1111-4111-8111-111111111111";
        let request = server
            .mock(
                "GET",
                format!("/api/v1/views/projects/{project}/invalidations").as_str(),
            )
            .match_query(mockito::Matcher::Any)
            .match_header("authorization", format!("Bearer {TOKEN}").as_str())
            .with_status(status)
            .with_header("content-type", "application/json")
            .with_body(if status == 200 {
                r#"{"data":{"items":[]}}"#
            } else {
                r#"{"error":"Unauthorized"}"#
            })
            .expect(1)
            .create();
        let refresh = server
            .mock("POST", "/api/v1/auth/refresh")
            .expect(0)
            .create();
        let envelope = run(
            &server,
            &["incidents", "--project-id", project],
            Some(TOKEN),
        );
        assert_eq!(
            envelope["status"],
            if status == 200 { "ok" } else { "error" }
        );
        request.assert();
        refresh.assert();
    }
}

#[test]
fn injected_credentials_never_use_or_refresh_a_broader_saved_session() {
    let project = "11111111-1111-4111-8111-111111111111";
    for expiry in [1, 4_102_444_800] {
        for token in [None, Some("malformed"), Some(TOKEN)] {
            for (args, path) in [
                (vec!["auth", "ensure"], CURRENT.to_string()),
                (
                    vec!["incidents", "--project-id", project],
                    format!("/api/v1/views/projects/{project}/invalidations"),
                ),
                (
                    vec!["download", "--project-id", project],
                    format!("/api/v1/projects/{project}"),
                ),
            ] {
                let mut server = mockito::Server::new();
                let injected = server
                    .mock("GET", path.as_str())
                    .match_query(mockito::Matcher::Any)
                    .match_header("authorization", format!("Bearer {TOKEN}").as_str())
                    .with_status(401)
                    .expect(usize::from(token == Some(TOKEN)))
                    .create();
                let broad = server
                    .mock("GET", mockito::Matcher::Any)
                    .match_header("authorization", "Bearer saved-broad-session")
                    .expect(0)
                    .create();
                let login_or_refresh = server
                    .mock("POST", mockito::Matcher::Any)
                    .expect(0)
                    .create();
                assert_eq!(
                    run_with_session(&server, &args, token, Some(expiry))["status"],
                    "error"
                );
                injected.assert();
                broad.assert();
                login_or_refresh.assert();
            }
        }
    }
}
