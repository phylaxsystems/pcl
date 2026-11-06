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
        .env("PCL_CONFIG_DIR", tmp.path().join("config"))
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
        .env("PCL_CONFIG_DIR", tmp.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "auth.api_token_invalid");
    login.assert();
}
