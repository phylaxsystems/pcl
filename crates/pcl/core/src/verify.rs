use crate::{
    abi,
    credible_config::{
        CredibleConfigError,
        CredibleToml,
        assertion_contract_name,
    },
    error::VerifyError,
    output::{
        OutputStream,
        ok_envelope,
        print_envelope,
    },
};
use alloy_json_abi::JsonAbi;
use alloy_primitives::{
    Bytes,
    hex,
};
use assertion_executor::ExecutorConfig;
use assertion_verification::{
    VerificationResult,
    VerificationStatus,
    verify_assertion,
};
use clap::ValueHint;
use pcl_common::args::{
    CliArgs,
    OutputMode,
};
use pcl_phoundry::{
    DEFAULT_ASSERTION_CONTRACTS_DIR,
    build_and_flatten::BuildAndFlattenArgs,
};
use serde::Serialize;
use serde_json::json;
use std::path::{
    Path,
    PathBuf,
};

#[derive(clap::Parser, Debug)]
#[command(name = "verify", about = "Verify assertions locally before deployment")]
pub struct VerifyArgs {
    /// Assertion to verify (contract name or `file:contract`).
    /// Verifies all assertions from `credible.toml` when omitted.
    #[arg()]
    pub assertion: Option<String>,

    #[arg(
        long,
        value_hint = ValueHint::DirPath,
        default_value = ".",
        help = "Project root directory"
    )]
    pub root: PathBuf,

    #[arg(
        short = 'c',
        long = "config",
        value_hint = ValueHint::FilePath,
        default_value = "assertions/credible.toml",
        help = "Path to credible.toml, relative to root or absolute"
    )]
    pub config: PathBuf,

    #[arg(long, num_args = 1.., help = "Constructor arguments for the assertion")]
    pub args: Vec<String>,

    #[arg(long, hide = true, help = "Deprecated; use global --json")]
    pub json: bool,
}

struct VerifyInput {
    display_name: String,
    bytecode: Bytes,
}

#[derive(Debug, Serialize)]
pub struct VerifyJsonAssertion {
    name: String,
    #[serde(flatten)]
    result: VerificationResult,
}

impl VerifyArgs {
    pub fn run(&self, cli_args: &CliArgs) -> Result<(), VerifyError> {
        let output_mode = if self.json {
            OutputMode::Json
        } else {
            cli_args.output_mode()
        };
        let root = std::fs::canonicalize(&self.root).map_err(|e| {
            VerifyError::Io {
                message: format!("Project root not found: {}", self.root.display()),
                source: e,
            }
        })?;

        if self.assertion.is_none() && !self.args.is_empty() {
            return Err(VerifyError::Config(CredibleConfigError::Invalid(
                "--args can only be used when verifying a specific assertion".to_string(),
            )));
        }

        let inputs = match &self.assertion {
            Some(assertion) => self.build_single(assertion, &root)?,
            None => Self::build_from_toml(&root, &self.config)?,
        };

        let bytecodes: Vec<(&str, Bytes)> = inputs
            .iter()
            .map(|input| (input.display_name.as_str(), input.bytecode.clone()))
            .collect();

        let summary = run_verification(&bytecodes);

        if output_mode == OutputMode::Human {
            println!("pcl verify \u{2014} Assertion Verification\n");
            print_verification_summary(&summary);
            if summary.failed == 0 {
                println!(
                    "All {} assertion{} verified successfully.",
                    summary.total,
                    if summary.total == 1 { "" } else { "s" }
                );
            } else {
                println!(
                    "{} of {} assertion{} failed verification.",
                    summary.failed,
                    summary.total,
                    if summary.total == 1 { "" } else { "s" }
                );
            }
        } else if summary.failed == 0 {
            let envelope = ok_envelope(
                json!({
                    "outcome": "success",
                    "total": summary.total,
                    "passed": summary.passed,
                    "failed": summary.failed,
                    "assertions": &summary.assertions,
                }),
                vec!["pcl apply --dry-run".to_string()],
            );
            print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
        }

        if summary.failed > 0 {
            return Err(VerifyError::AssertionsFailed(Box::new(summary)));
        }

        Ok(())
    }

    fn build_single(&self, assertion: &str, root: &Path) -> Result<Vec<VerifyInput>, VerifyError> {
        let contract_name = parse_assertion_name(assertion);
        let output = BuildAndFlattenArgs {
            root: Some(root.to_path_buf()),
            assertion_contract: contract_name.clone(),
            contracts: assertion_contracts_dir(assertion),
        }
        .run()
        .map_err(VerifyError::BuildFailed)?;

        let bytecode = build_deployment_bytecode(&output.bytecode, &output.abi, &self.args)?;
        let display_name = format_display_name(&contract_name, &self.args);

        Ok(vec![VerifyInput {
            display_name,
            bytecode,
        }])
    }

    fn build_from_toml(root: &Path, config: &Path) -> Result<Vec<VerifyInput>, VerifyError> {
        let config_path = root.join(config);
        let credible = CredibleToml::from_path(&config_path)?;

        let mut inputs = Vec::new();
        for contract in credible.contracts.values() {
            for assertion in &contract.assertions {
                let contract_name = assertion_contract_name(&assertion.file)?;
                let output = BuildAndFlattenArgs {
                    root: Some(root.to_path_buf()),
                    assertion_contract: contract_name.clone(),
                    contracts: assertion_contracts_dir(&assertion.file),
                }
                .run()
                .map_err(VerifyError::BuildFailed)?;

                let bytecode =
                    build_deployment_bytecode(&output.bytecode, &output.abi, &assertion.args)?;
                let display_name = format_display_name(&contract_name, &assertion.args);

                inputs.push(VerifyInput {
                    display_name,
                    bytecode,
                });
            }
        }

        if inputs.is_empty() {
            return Err(VerifyError::Config(CredibleConfigError::Invalid(
                "No assertions found in credible.toml".to_string(),
            )));
        }

        Ok(inputs)
    }
}

/// Parses a CLI assertion argument into a contract name.
///
/// - `ContractName` -> `ContractName`
/// - `file.sol:ContractName` -> `ContractName`
fn parse_assertion_name(arg: &str) -> String {
    if let Some((_, contract_name)) = arg.rsplit_once(':') {
        contract_name.to_string()
    } else {
        arg.to_string()
    }
}

fn assertion_contracts_dir(file: &str) -> PathBuf {
    let source_path = file.split_once(':').map_or(file, |(path, _)| path);
    Path::new(source_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR),
            Path::to_path_buf,
        )
}

/// Result of verifying a set of assertions.
#[derive(Debug, Serialize)]
pub struct VerificationSummary {
    pub status: &'static str,
    pub assertions: Vec<VerifyJsonAssertion>,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
}

/// Runs verification on a set of assertions and returns results without printing.
///
/// Each entry is `(display_name, deployment_bytecode)`.
/// Callers are responsible for display (human or JSON).
pub fn run_verification(inputs: &[(&str, Bytes)]) -> VerificationSummary {
    let executor_config = ExecutorConfig::default();
    let assertions: Vec<VerifyJsonAssertion> = inputs
        .iter()
        .map(|(display_name, bytecode)| {
            let result = verify_assertion(bytecode, &executor_config);
            VerifyJsonAssertion {
                name: (*display_name).to_string(),
                result,
            }
        })
        .collect();

    let total = assertions.len();
    let failed = assertions
        .iter()
        .filter(|a| a.result.status != VerificationStatus::Success)
        .count();
    let passed = total - failed;

    VerificationSummary {
        status: if failed == 0 { "success" } else { "failure" },
        assertions,
        total,
        passed,
        failed,
    }
}

/// Prints verification results in human-readable format.
pub fn print_verification_summary(summary: &VerificationSummary) {
    for assertion in &summary.assertions {
        print_human_result(&assertion.name, &assertion.result);
    }
}

pub fn build_deployment_bytecode(
    bytecode_hex: &str,
    abi: &JsonAbi,
    args: &[String],
) -> Result<Bytes, VerifyError> {
    let mut bytecode =
        hex::decode(bytecode_hex).map_err(|e| VerifyError::BytecodeHex(e.to_string()))?;

    if !args.is_empty() {
        let encoded = abi::encode_args(abi, args)?;
        bytecode.extend_from_slice(&encoded);
    }

    Ok(Bytes::from(bytecode))
}

pub fn format_display_name(name: &str, args: &[String]) -> String {
    if args.is_empty() {
        name.to_string()
    } else {
        let args_display: Vec<_> = args.iter().map(|a| abbreviate_arg(a)).collect();
        format!("{}({})", name, args_display.join(", "))
    }
}

pub fn abbreviate_arg(arg: &str) -> String {
    if arg.len() > 10 && arg.starts_with("0x") {
        format!("{}...{}", &arg[..6], &arg[arg.len() - 4..])
    } else {
        arg.to_string()
    }
}

pub fn status_str(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Success => "success",
        VerificationStatus::DeploymentFailure => "deployment_failure",
        VerificationStatus::NoTriggers => "no_triggers",
        VerificationStatus::MissingAssertionSpec => "missing_assertion_spec",
        VerificationStatus::InvalidAssertionSpec => "invalid_assertion_spec",
    }
}

pub fn print_human_result(display_name: &str, result: &VerificationResult) {
    if result.status == VerificationStatus::Success {
        println!("  \u{2713} {display_name}");
        if let Some(triggers) = &result.triggers {
            println!("    triggers:");
            for (selector, trigger_types) in triggers {
                println!("      {selector} \u{2192} {trigger_types}");
            }
        }
    } else {
        println!("  \u{2717} {display_name}");
        println!("    status: {}", status_str(result.status));
        if let Some(error) = &result.error {
            println!("    error: {error}");
        }
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_assertion_name_bare() {
        assert_eq!(parse_assertion_name("MyContract"), "MyContract");
    }

    #[test]
    fn parse_assertion_name_qualified() {
        assert_eq!(
            parse_assertion_name("MyContract.a.sol:MyContract"),
            "MyContract"
        );
    }

    #[test]
    fn format_display_name_no_args() {
        assert_eq!(format_display_name("Foo", &[]), "Foo");
    }

    #[test]
    fn format_display_name_with_args() {
        let args = vec!["0x1234567890abcdef".to_string(), "42".to_string()];
        assert_eq!(format_display_name("Foo", &args), "Foo(0x1234...cdef, 42)");
    }

    #[test]
    fn abbreviate_short_arg() {
        assert_eq!(abbreviate_arg("42"), "42");
        assert_eq!(abbreviate_arg("0xshort"), "0xshort");
    }

    #[test]
    fn abbreviate_long_hex_arg() {
        let arg = "0x1234567890abcdef";
        assert_eq!(abbreviate_arg(arg), "0x1234...cdef");
    }

    #[test]
    fn build_deployment_bytecode_no_args() {
        let abi = JsonAbi::default();
        let result = build_deployment_bytecode("6001", &abi, &[]).unwrap();
        assert_eq!(result.as_ref(), &[0x60, 0x01]);
    }

    #[test]
    fn build_deployment_bytecode_with_0x_prefix() {
        let abi = JsonAbi::default();
        let result = build_deployment_bytecode("0x6001", &abi, &[]).unwrap();
        assert_eq!(result.as_ref(), &[0x60, 0x01]);
    }
}
