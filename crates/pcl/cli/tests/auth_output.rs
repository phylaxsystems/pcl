use std::{
    fs,
    process::Command,
};

#[test]
fn auth_login_json_with_existing_auth_outputs_json_envelope() {
    let temp_dir = tempfile::tempdir().expect("create temp config dir");
    fs::write(
        temp_dir.path().join("config.toml"),
        r#"[auth]
access_token = "test-token"
refresh_token = "refresh-token"
expires_at = 4102444800
email = "agent@example.com"
"#,
    )
    .expect("write test config");

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
