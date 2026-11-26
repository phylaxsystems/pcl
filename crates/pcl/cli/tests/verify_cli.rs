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

/// Installed solc 0.8.28 from svm, if any.
#[cfg(feature = "credible")]
fn installed_solc() -> Option<std::path::PathBuf> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    [
        home.join(".svm"),
        home.join("Library/Application Support/svm"),
        home.join(".local/share/svm"),
    ]
    .into_iter()
    .map(|svm| svm.join("0.8.28/solc-0.8.28"))
    .find(|solc| solc.is_file())
}

#[cfg(feature = "credible")]
#[test]
fn release_runs_a_local_solc_given_relative_to_the_current_directory() {
    let Some(solc) = installed_solc() else {
        return;
    };
    let project = fixture_project();
    fs::copy(solc, project.path().join("solc")).expect("copy solc");
    let config = project.path().join("foundry.toml");
    let toml = fs::read_to_string(&config).expect("read foundry.toml");
    fs::write(
        &config,
        toml.replace("solc = \"0.8.28\"", "solc = \"./solc\""),
    )
    .expect("write");
    for env in [None, Some("./solc")] {
        if env.is_some() {
            fs::write(&config, toml.replace("solc = \"0.8.28\"\n", "")).expect("write");
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_pcl"));
        command
            .current_dir(project.path())
            .args(["--json", "apply", "--root", ".", "--dry-run"]);
        if let Some(solc) = env {
            command.env("FOUNDRY_SOLC", solc);
        }
        let output = command.output().expect("run pcl");
        assert_command_success(&output, "pcl apply --dry-run with a relative solc");
    }
}

/// Adds a second assertion to the fixture project.
#[cfg(feature = "credible")]
fn add_second_assertion(project: &Path) {
    let src = project.join("assertions/src");
    let first = fs::read_to_string(src.join("NoArgsAssertion.a.sol")).expect("read assertion");
    fs::write(
        src.join("Second.a.sol"),
        first.replace("NoArgsAssertion", "Second"),
    )
    .expect("write second assertion");
    let config = project.join("assertions/credible.toml");
    let toml = fs::read_to_string(&config).expect("read credible.toml");
    fs::write(
        &config,
        format!(
            "{toml}\n[[contracts.mock.assertions]]\nfile = \"assertions/src/Second.a.sol:Second\"\n"
        ),
    )
    .expect("write credible.toml");
}

/// A regular build, which keeps build info without the compiler input.
#[cfg(feature = "credible")]
fn regular_build(project: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_pcl"))
        .args(["build", "--root", project.to_str().expect("utf-8")])
        .output()
        .expect("run pcl build");
    assert_command_success(&output, "pcl build");
}

/// Contents and permissions of every file under the project's `out` and cache directories.
#[cfg(feature = "credible")]
fn project_outputs(project: &Path) -> Vec<(std::path::PathBuf, Vec<u8>, bool)> {
    let mut files = vec![];
    let mut pending = vec![project.join("out"), project.join("assertions/cache")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let readonly = path.metadata().expect("metadata").permissions().readonly();
                files.push((path.clone(), fs::read(&path).expect("read"), readonly));
            }
        }
    }
    files.sort();
    files
}

#[cfg(feature = "credible")]
fn release_payload(project: &Path, command: &str) -> serde_json::Value {
    let (ok, envelope) = pcl_json(project, &[command, "--dry-run"]);
    assert!(ok, "{command}: {envelope}");
    envelope["data"]["payload"]["contracts"].clone()
}

#[cfg(feature = "credible")]
#[test]
fn release_on_a_cache_without_full_build_info_sends_a_full_build_and_keeps_the_cache() {
    let cold = fixture_project();
    add_second_assertion(cold.path());
    let expected = release_payload(cold.path(), "apply");

    let project = fixture_project();
    add_second_assertion(project.path());
    regular_build(project.path());
    let before = project_outputs(project.path());
    // Apply and deploy send what a full build of the same sources sends, on every run, and
    // leave the project's artifacts, cache and build info as they were.
    for command in ["apply", "deploy", "apply"] {
        assert_eq!(
            release_payload(project.path(), command),
            expected,
            "{command}"
        );
    }
    assert_eq!(project_outputs(project.path()), before);
}

#[cfg(all(feature = "credible", unix))]
#[test]
fn release_works_with_read_only_build_outputs() {
    use std::os::unix::fs::PermissionsExt;
    let set_mode = |path: &Path, writable: bool| {
        let mode = fs::metadata(path).expect("metadata").permissions().mode();
        let mode = if writable {
            mode | 0o200
        } else {
            mode & !0o222
        };
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
    };
    let cold = fixture_project();
    add_second_assertion(cold.path());
    let expected = release_payload(cold.path(), "apply");

    for protect in ["artifacts", "cache directory"] {
        let project = fixture_project();
        add_second_assertion(project.path());
        regular_build(project.path());
        let protected: Vec<std::path::PathBuf> = if protect == "artifacts" {
            let mut paths = vec![];
            let mut pending = vec![project.path().join("out")];
            while let Some(path) = pending.pop() {
                if path.is_dir() {
                    pending.extend(
                        fs::read_dir(&path)
                            .expect("dir")
                            .map(|e| e.expect("entry").path()),
                    );
                }
                paths.push(path);
            }
            paths
        } else {
            // The index stays writable, but the directory refuses new or removed entries.
            vec![project.path().join("assertions/cache")]
        };
        for path in &protected {
            set_mode(path, false);
        }
        let before = project_outputs(project.path());
        for command in ["apply", "deploy"] {
            assert_eq!(
                release_payload(project.path(), command),
                expected,
                "{protect} {command}"
            );
        }
        assert_eq!(project_outputs(project.path()), before, "{protect}");
        for path in &protected {
            set_mode(path, true);
        }
    }
}
