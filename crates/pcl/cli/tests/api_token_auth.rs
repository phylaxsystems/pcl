use serde_json::{
    Value,
    json,
};
use std::{
    fs,
    process::Command,
};

#[test]
fn ensure_uses_injected_token_without_creating_a_session() {
    let mut server = mockito::Server::new();
    let token = format!("cl_api_{}", "a".repeat(43));
    let check = server.mock("GET", "/api/v1/auth/api-tokens/current")
        .match_header("authorization", format!("Bearer {token}").as_str())
        .with_status(200).with_header("content-type", "application/json")
        .with_body(r#"{"authenticated":true,"project_id":"project","scope":"invalidations:read","expires_at":"2099-01-01"}"#).create();
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("credential.json");
    fs::write(
        &file,
        serde_json::to_vec(&json!({"platform_url":server.url(),"token":token})).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["auth", "ensure", "--json"])
        .env("PCL_API_TOKEN_FILE", &file)
        .env("PCL_API_URL", server.url())
        .env("PCL_AUTH_URL", server.url())
        .arg("--config-dir")
        .arg(tmp.path().join("config"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["data"]["authenticated"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(&token));
    let config = fs::read_to_string(tmp.path().join("config/config.toml")).unwrap_or_default();
    assert!(!config.contains(&token));
    assert!(!config.contains("[auth]"));
    check.assert();
}

#[test]
fn bad_token_file_never_starts_device_login() {
    let mut server = mockito::Server::new();
    let login = server.mock("GET", mockito::Matcher::Any).expect(0).create();
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["auth", "ensure", "--json"])
        .env("PCL_API_TOKEN_FILE", tmp.path().join("missing"))
        .env("PCL_API_URL", server.url())
        .env("PCL_AUTH_URL", server.url())
        .arg("--config-dir")
        .arg(tmp.path().join("config"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "auth.api_token_invalid");
    login.assert();
}

#[test]
fn rejected_or_redirected_token_never_falls_back_to_login() {
    for status in [401, 302] {
        let mut server = mockito::Server::new();
        let token = format!("cl_api_{}", "b".repeat(43));
        let check = server
            .mock("GET", "/api/v1/auth/api-tokens/current")
            .match_header("authorization", format!("Bearer {token}").as_str())
            .with_status(status)
            .with_header("location", &format!("{}/unexpected", server.url()))
            .expect(1)
            .create();
        let fallback = server.mock("GET", "/unexpected").expect(0).create();
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("credential.json");
        fs::write(
            &file,
            serde_json::to_vec(&json!({"platform_url":server.url(),"token":token})).unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
            .args(["auth", "ensure", "--json", "--config-dir"])
            .arg(tmp.path().join("config"))
            .env("PCL_API_TOKEN_FILE", &file)
            .env("PCL_API_URL", server.url())
            .env("PCL_AUTH_URL", server.url())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(envelope["error"]["code"], "auth.api_token_invalid");
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&token));
        check.assert();
        fallback.assert();
    }
}

#[test]
fn incidents_use_injected_token_and_do_not_refresh_after_rejection() {
    for status in [200, 401] {
        let mut server = mockito::Server::new();
        let token = format!("cl_api_{}", "c".repeat(43));
        let project = "11111111-1111-4111-8111-111111111111";
        let request = server
            .mock(
                "GET",
                format!("/api/v1/views/projects/{project}/invalidations").as_str(),
            )
            .match_query(mockito::Matcher::Any)
            .match_header("authorization", format!("Bearer {token}").as_str())
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
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("credential.json");
        fs::write(
            &file,
            serde_json::to_vec(&json!({"platform_url":server.url(),"token":token})).unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
            .args([
                "incidents",
                "--project-id",
                project,
                "--json",
                "--config-dir",
            ])
            .arg(tmp.path().join("config"))
            .env("PCL_API_TOKEN_FILE", &file)
            .env("PCL_API_URL", server.url())
            .output()
            .unwrap();
        assert_eq!(
            output.status.success(),
            status == 200,
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            envelope["status"],
            if status == 200 { "ok" } else { "error" }
        );
        assert!(!String::from_utf8_lossy(&output.stdout).contains(&token));
        request.assert();
        refresh.assert();
    }
}
