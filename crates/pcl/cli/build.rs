use std::{
    process::Command,
    time::{
        SystemTime,
        UNIX_EPOCH,
    },
};

pub fn main() {
    println!("cargo:rerun-if-changed=../../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=VERGEN_GIT_SHA");
    println!("cargo:rerun-if-env-changed=VERGEN_BUILD_TIMESTAMP");

    println!(
        "cargo:rustc-env=VERGEN_GIT_SHA={}",
        std::env::var("VERGEN_GIT_SHA")
            .ok()
            .or_else(git_sha)
            .unwrap_or_else(|| "unknown".to_string())
    );
    println!(
        "cargo:rustc-env=VERGEN_BUILD_TIMESTAMP={}",
        std::env::var("VERGEN_BUILD_TIMESTAMP")
            .ok()
            .unwrap_or_else(build_timestamp)
    );
}

fn git_sha() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8(output.stdout).ok()?;
    let sha = sha.trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha.to_string())
    }
}

fn build_timestamp() -> String {
    if let Some(timestamp) = date_timestamp() {
        return timestamp;
    }

    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| "unknown".to_string(),
        |duration| duration.as_secs().to_string(),
    )
}

fn date_timestamp() -> Option<String> {
    let output = Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let timestamp = String::from_utf8(output.stdout).ok()?;
    let timestamp = timestamp.trim();
    if timestamp.is_empty() {
        None
    } else {
        Some(timestamp.to_string())
    }
}
