#[cfg(feature = "credible")]
use std::{
    fs,
    path::Path,
    process::Command,
};

#[cfg(feature = "credible")]
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create fixture destination");
    for entry in fs::read_dir(from).expect("read fixture directory") {
        let entry = entry.expect("read fixture entry");
        let source = entry.path();
        let destination = to.join(entry.file_name());
        if source.is_dir() {
            copy_dir(&source, &destination);
        } else {
            fs::copy(&source, &destination).expect("copy fixture file");
        }
    }
}

#[cfg(feature = "credible")]
fn fixture_project() -> tempfile::TempDir {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/verify-project");
    let temp_dir = tempfile::tempdir().expect("create temp project");
    copy_dir(&fixture, temp_dir.path());
    temp_dir
}

#[cfg(feature = "credible")]
fn assert_verify_success(output: std::process::Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    let summary = &envelope["data"];
    assert_eq!(summary["outcome"], "success");
    assert_eq!(summary["total"], 1);
    assert_eq!(summary["passed"], 1);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["assertions"][0]["name"], "NoArgsAssertion");
    assert_eq!(summary["assertions"][0]["status"], "success");
    assert_eq!(
        summary["assertions"][0]["triggers"]["0x0f04ec21"],
        "allCall"
    );
    let next_action = envelope["next_actions"][0]
        .as_str()
        .expect("next action string");
    assert!(next_action.contains("pcl apply --root "), "{next_action}");
    assert!(
        next_action.contains("--config assertions/credible.toml"),
        "{next_action}"
    );
}

#[cfg(feature = "credible")]
fn assert_command_success(output: &std::process::Output, command: &str) {
    assert!(
        output.status.success(),
        "{command} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(feature = "credible")]
#[test]
fn build_cli_succeeds_for_fixture_project() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "build",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
        ])
        .output()
        .expect("run pcl build");

    assert_command_success(&output, "pcl build");
}

#[cfg(feature = "credible")]
#[test]
fn test_cli_succeeds_for_fixture_project() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "test",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
        ])
        .output()
        .expect("run pcl test");

    assert_command_success(&output, "pcl test");
}

#[cfg(feature = "credible")]
#[test]
fn apply_dry_run_builds_and_verifies_fixture_payload_without_api() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--json",
            "apply",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "--dry-run",
        ])
        .output()
        .expect("run pcl apply dry-run");

    assert_command_success(&output, "pcl apply --dry-run");
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let envelope: serde_json::Value = serde_json::from_str(&stdout).expect("json envelope");
    assert_eq!(envelope["status"], "ok");
    assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
    let summary = &envelope["data"];
    assert_eq!(summary["outcome"], "dry_run");
    assert_eq!(
        summary["project_id"],
        "550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(summary["applied"], false);
    assert_eq!(summary["preview"], serde_json::Value::Null);
    assert_eq!(summary["release"], serde_json::Value::Null);
    assert_eq!(summary["verification"]["status"], "success");
    assert_eq!(summary["verification"]["passed"], 1);
    assert_eq!(
        summary["payload"]["contracts"]["mock"]["assertions"][0]["contractName"],
        "NoArgsAssertion"
    );
    assert_eq!(
        summary["payload"]["contracts"]["mock"]["assertions"][0]["compilerVersion"],
        "v0.8.28+commit.7893614a"
    );
    assert_eq!(
        summary["payload"]["contracts"]["mock"]["assertions"][0]["evmVersion"],
        "cancun"
    );
    assert!(
        summary["payload"]["contracts"]["mock"]["assertions"][0]["bytecode"]
            .as_str()
            .is_some_and(|bytecode| bytecode.starts_with("0x"))
    );
    let root = fs::canonicalize(project.path()).expect("canonical fixture root");
    assert_eq!(
        envelope["next_actions"][0],
        format!(
            "pcl apply --root {} --config assertions/credible.toml --yes --json",
            root.display()
        )
    );
}

#[cfg(feature = "credible")]
#[test]
fn apply_dry_run_json_preserves_failed_assertion_summary() {
    let project = fixture_project();
    fs::write(
        project.path().join("assertions/src/NoArgsAssertion.a.sol"),
        r"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

abstract contract Assertion {
    function triggers() external view virtual;
}

contract NoArgsAssertion is Assertion {
    function triggers() external view override {}

    function assertionCheckBool() external pure returns (bool) {
        return true;
    }
}
",
    )
    .expect("write failing assertion fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "--json",
            "apply",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "--dry-run",
        ])
        .output()
        .expect("run pcl apply dry-run");

    assert!(
        !output.status.success(),
        "pcl apply unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("json error envelope");
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "apply.assertions_failed");
    assert_eq!(envelope["data"]["status"], "failure");
    assert_eq!(envelope["data"]["failed"], 1);
    assert_eq!(envelope["data"]["assertions"][0]["name"], "NoArgsAssertion");
    assert_eq!(envelope["data"]["assertions"][0]["status"], "no_triggers");
    assert_eq!(
        envelope["next_actions"][0],
        "Inspect data.assertions for failing assertions"
    );
}

#[cfg(feature = "credible")]
#[test]
fn verify_cli_succeeds_for_explicit_fixture_assertion() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "verify",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "assertions/src/NoArgsAssertion.a.sol:NoArgsAssertion",
            "--json",
        ])
        .output()
        .expect("run pcl verify");

    assert_verify_success(output);
}

#[cfg(feature = "credible")]
#[test]
fn verify_cli_succeeds_for_credible_toml_fixture() {
    let project = fixture_project();

    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args([
            "verify",
            "--root",
            project.path().to_str().expect("utf-8 temp path"),
            "--json",
        ])
        .output()
        .expect("run pcl verify");

    assert_verify_success(output);
}

/// Runs `pcl --json <args> --root <project>` and returns success plus the envelope.
#[cfg(feature = "credible")]
fn pcl_json(project: &Path, args: &[&str]) -> (bool, serde_json::Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .arg("--json")
        .args(args)
        .args(["--root", project.to_str().expect("utf-8 temp path")])
        .output()
        .expect("run pcl");
    let stream = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let envelope = serde_json::from_slice(stream).unwrap_or_else(|_| {
        panic!(
            "json envelope\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    (output.status.success(), envelope)
}

#[cfg(feature = "credible")]
fn set_assertion_file(project: &Path, file: &str) {
    let config = project.join("assertions/credible.toml");
    let toml: Vec<String> = fs::read_to_string(&config)
        .expect("read credible.toml")
        .lines()
        .map(|line| {
            if line.starts_with("file = ") {
                format!("file = \"{file}\"")
            } else {
                line.to_string()
            }
        })
        .collect();
    fs::write(config, toml.join("\n")).expect("write credible.toml");
}

#[cfg(feature = "credible")]
#[test]
fn release_and_verify_select_the_configured_file_over_a_same_named_contract() {
    let project = fixture_project();
    // Sorts first and declares the same contract name without triggers.
    fs::write(
        project.path().join("assertions/src/AAA.sol"),
        "pragma solidity ^0.8.28;\ncontract NoArgsAssertion { function x() external {} }\n",
    )
    .expect("write duplicate");

    for file in [
        "assertions/src/NoArgsAssertion.a.sol:NoArgsAssertion",
        "assertions/src/NoArgsAssertion.a.sol",
    ] {
        set_assertion_file(project.path(), file);
        let (ok, envelope) = pcl_json(project.path(), &["apply", "--dry-run"]);
        assert!(ok, "{envelope}");
        let item = &envelope["data"]["payload"]["contracts"]["mock"]["assertions"][0];
        assert_eq!(
            item["compilerSourcePath"],
            "assertions/src/NoArgsAssertion.a.sol"
        );
        let input: serde_json::Value =
            serde_json::from_str(item["compilerInput"].as_str().expect("compiler input"))
                .expect("compiler input json");
        let sources: Vec<_> = input["sources"]
            .as_object()
            .expect("sources")
            .keys()
            .collect();
        assert_eq!(sources, ["assertions/src/NoArgsAssertion.a.sol"]);
        let (ok, envelope) = pcl_json(project.path(), &["verify"]);
        assert!(ok, "{file}: {envelope}");
    }
    let (ok, envelope) = pcl_json(
        project.path(),
        &[
            "verify",
            "assertions/src/NoArgsAssertion.a.sol:NoArgsAssertion",
        ],
    );
    assert!(ok, "{envelope}");
}

#[cfg(feature = "credible")]
#[test]
fn release_accepts_bare_toml_file_names_and_names_missing_files() {
    let project = fixture_project();
    set_assertion_file(project.path(), "NoArgsAssertion.a.sol");
    let (ok, envelope) = pcl_json(project.path(), &["apply", "--dry-run"]);
    assert!(ok, "{envelope}");
    assert_eq!(
        envelope["data"]["payload"]["contracts"]["mock"]["assertions"][0]["compilerSourcePath"],
        "assertions/src/NoArgsAssertion.a.sol"
    );

    set_assertion_file(project.path(), "assertions/src/Typo.a.sol");
    let (ok, envelope) = pcl_json(project.path(), &["apply", "--dry-run"]);
    assert!(!ok);
    assert_eq!(envelope["error"]["code"], "build.failed");
    let message = envelope["error"]["message"].as_str().expect("message");
    assert!(message.contains("assertions/src/Typo.a.sol"), "{message}");
}

#[cfg(feature = "credible")]
#[test]
fn release_fails_on_unrelated_invalid_project_sources() {
    let project = fixture_project();
    fs::write(
        project.path().join("assertions/src/Invalid.sol"),
        "pragma solidity ^0.8.28; invalid syntax;",
    )
    .expect("write invalid source");
    for command in [["apply", "--dry-run"], ["deploy", "--dry-run"]] {
        let (ok, envelope) = pcl_json(project.path(), &command);
        assert!(!ok, "{command:?} succeeded: {envelope}");
        assert_eq!(envelope["error"]["code"], "build.failed", "{envelope}");
    }
}

#[cfg(feature = "credible")]
fn release_item(envelope: &serde_json::Value) -> &serde_json::Value {
    &envelope["data"]["payload"]["contracts"]["mock"]["assertions"][0]
}

#[cfg(feature = "credible")]
#[test]
fn release_accepts_crlf_sources_cold_and_cached() {
    let project = fixture_project();
    let source = project.path().join("assertions/src/NoArgsAssertion.a.sol");
    let content = fs::read_to_string(&source).expect("read assertion");
    fs::write(&source, content.replace('\n', "\r\n")).expect("write CRLF assertion");
    for state in ["cold", "cached"] {
        for command in [["apply", "--dry-run"], ["deploy", "--dry-run"]] {
            let (ok, envelope) = pcl_json(project.path(), &command);
            assert!(ok, "{state} {command:?}: {envelope}");
        }
    }
}

#[cfg(feature = "credible")]
#[test]
fn release_prefers_a_root_relative_bare_file_over_the_default_directory() {
    let project = fixture_project();
    let default = project.path().join("assertions/src/NoArgsAssertion.a.sol");
    let content = fs::read_to_string(&default).expect("read assertion");
    fs::write(
        project.path().join("NoArgsAssertion.a.sol"),
        format!("{content}\n// ROOT VARIANT\n"),
    )
    .expect("write root variant");
    fs::write(
        project.path().join("assertions/src/UsesRoot.sol"),
        "pragma solidity ^0.8.28;\nimport {NoArgsAssertion as RootVariant} from \"NoArgsAssertion.a.sol\";\ncontract UsesRoot {}\n",
    )
    .expect("write importer");
    set_assertion_file(project.path(), "NoArgsAssertion.a.sol:NoArgsAssertion");
    let (ok, envelope) = pcl_json(project.path(), &["apply", "--dry-run"]);
    assert!(ok, "{envelope}");
    let item = release_item(&envelope);
    assert_eq!(item["compilerSourcePath"], "NoArgsAssertion.a.sol");
    assert!(
        item["flattenedSource"]
            .as_str()
            .is_some_and(|source| source.contains("ROOT VARIANT"))
    );

    fs::remove_file(project.path().join("assertions/src/UsesRoot.sol")).expect("remove importer");
    fs::remove_file(project.path().join("NoArgsAssertion.a.sol")).expect("remove root variant");
    let (ok, envelope) = pcl_json(project.path(), &["apply", "--dry-run"]);
    assert!(ok, "{envelope}");
    assert_eq!(
        release_item(&envelope)["compilerSourcePath"],
        "assertions/src/NoArgsAssertion.a.sol"
    );
}

#[cfg(feature = "credible")]
#[test]
fn release_sends_the_original_job_settings() {
    let project = fixture_project();
    let config = project.path().join("foundry.toml");
    let toml = fs::read_to_string(&config).expect("read foundry.toml");
    fs::write(
        &config,
        format!("{toml}\nvia_ir = false\n[profile.default.model_checker]\nengine = \"none\"\n"),
    )
    .expect("write foundry.toml");
    let (ok, envelope) = pcl_json(project.path(), &["apply", "--dry-run"]);
    assert!(ok, "{envelope}");
    let input: serde_json::Value = serde_json::from_str(
        release_item(&envelope)["compilerInput"]
            .as_str()
            .expect("compiler input"),
    )
    .expect("compiler input json");
    let job = fs::read_dir(project.path().join("out/build-info"))
        .expect("build info dir")
        .map(|entry| {
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(entry.expect("entry").path()).expect("read build info"),
            )
            .expect("build info json")
        })
        .find_map(|info| info.get("input").cloned())
        .expect("full build info");
    assert_eq!(input["settings"], job["settings"]);
    assert_eq!(input["settings"]["viaIR"], false);
    assert_eq!(input["settings"]["modelChecker"]["engine"], "none");
    assert!(input.get("basePath").is_none() && input.get("includePaths").is_none());
}
