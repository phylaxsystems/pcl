use clap::Parser;
use pcl_common::args::CliArgs;

use crate::{phorge::Phorge, error::PhoundryError};

#[derive(Debug)]
pub struct AssertionBuildOutput {
    pub contract_name: String,
    pub bytecode: String,
    pub compiler_metadata: String,
}

impl AssertionBuildOutput {
    pub fn new(contract_name: String, bytecode: String, compiler_metadata: String) -> Self {
        Self {
            contract_name,
            bytecode,
            compiler_metadata,
        }
    }
}

#[derive(Parser)]
pub struct BuildArgs {
    pub assertions: Vec<String>,
}

impl BuildArgs {
    pub fn run(&self, cli_args: CliArgs) -> Result<(), PhoundryError> {
        let build_output = self.execute_forge_build(&cli_args)?;

        if !build_output.stdout.is_empty() {
            let json_output = self.parse_forge_output(&build_output.stdout)?;
            let contracts = self.extract_contracts(&json_output)?;
            let _assertion_builds = self.process_contracts(contracts)?;
        }

        if !build_output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&build_output.stderr));
        }
        Ok(())
    }

    fn execute_forge_build(
        &self,
        cli_args: &CliArgs,
    ) -> Result<std::process::Output, PhoundryError> {
        let args = [
            "build",
            "--force",
            "-C",
            cli_args.assertions_dir().as_os_str().to_str().unwrap(),
            "--json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let phoundry = Phorge { args };
        phoundry.run(cli_args.clone(), false)
    }

    fn parse_forge_output(&self, stdout: &[u8]) -> Result<serde_json::Value, PhoundryError> {
        serde_json::from_slice(stdout)
            .map_err(|_| PhoundryError::InvalidForgeOutput("invalid json output"))
    }

    fn extract_contracts<'a>(
        &'a self,
        json_output: &'a serde_json::Value,
    ) -> Result<&'a serde_json::Map<String, serde_json::Value>, PhoundryError> {
        json_output
            .get("contracts")
            .and_then(|c| c.as_object())
            .ok_or(PhoundryError::InvalidForgeOutput("invalid contracts field"))
    }

    fn process_contracts(
        &self,
        contracts: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Vec<AssertionBuildOutput>, PhoundryError> {
        let mut assertion_builds = Vec::new();

        for (_, contract_data) in contracts {
            let contracts_obj =
                contract_data
                    .as_object()
                    .ok_or(PhoundryError::InvalidForgeOutput(
                        "invalid contract data format",
                    ))?;

            for (contract_name, implementations) in contracts_obj {
                if !self.assertions.contains(contract_name) {
                    continue;
                }

                let builds = self.process_implementations(implementations, contract_name)?;
                assertion_builds.extend(builds);
            }
        }

        Ok(assertion_builds)
    }

    fn process_implementations(
        &self,
        implementations: &serde_json::Value,
        contract_name: &str,
    ) -> Result<Vec<AssertionBuildOutput>, PhoundryError> {
        let mut builds = Vec::new();
        let implementations =
            implementations
                .as_array()
                .ok_or(PhoundryError::InvalidForgeOutput(
                    "invalid implementations format",
                ))?;

        for impl_data in implementations {
            let compiler_metadata = self.extract_metadata(impl_data)?;
            let bytecode = self.extract_bytecode(impl_data)?;
            builds.push(AssertionBuildOutput::new(
                contract_name.to_string(),
                bytecode,
                compiler_metadata,
            ));
        }

        Ok(builds)
    }

    fn extract_metadata(&self, impl_data: &serde_json::Value) -> Result<String, PhoundryError> {
        impl_data
            .get("contract")
            .and_then(|c| c.get("metadata"))
            .and_then(|m| m.as_str())
            .map(String::from)
            .ok_or(PhoundryError::InvalidForgeOutput(
                "missing or invalid metadata",
            ))
    }

    fn extract_bytecode(&self, impl_data: &serde_json::Value) -> Result<String, PhoundryError> {
        impl_data
            .get("contract")
            .and_then(|c| c.get("evm"))
            .and_then(|e| e.get("bytecode"))
            .and_then(|b| b.get("object"))
            .and_then(|o| o.as_str())
            .map(String::from)
            .ok_or(PhoundryError::InvalidForgeOutput(
                "missing or invalid bytecode",
            ))
    }

    pub fn get_flattened_source(&self, path: &str) -> Result<String, PhoundryError> {
        let flatten_args = vec!["flatten".to_string(), path.to_string()];
        let phoundry = Phorge { args: flatten_args };
        let flatten_output = phoundry.run(CliArgs::default(), false)?;
        Ok(String::from_utf8_lossy(&flatten_output.stdout).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn get_sample_forge_output() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../testdata/forge-build-output.json"))
            .expect("Failed to parse test JSON")
    }

    #[test]
    fn test_parse_forge_output() {
        let args = BuildArgs { assertions: vec![] };
        let json_bytes = serde_json::to_vec(&get_sample_forge_output()).unwrap();

        let result = args.parse_forge_output(&json_bytes);
        assert!(result.is_ok());
    }

    #[test]
    fn test_extract_contracts() {
        let args = BuildArgs { assertions: vec![] };
        let json = get_sample_forge_output();

        let contracts = args.extract_contracts(&json);
        assert!(contracts.is_ok());

        let contracts = contracts.unwrap();
        assert!(contracts.contains_key(
            "/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/assertions/src/Assertion.sol"
        ));
    }

    #[test]
    fn test_process_contracts() {
        let args = BuildArgs {
            assertions: vec!["TestIncrement".to_string()],
        };
        let json = get_sample_forge_output();
        let contracts = args.extract_contracts(&json).unwrap();

        let builds = args.process_contracts(contracts);
        assert!(builds.is_ok());

        let builds = builds.unwrap();
        assert!(!builds.is_empty());
        assert_eq!(builds[0].contract_name, "TestIncrement");
    }

    #[test]
    fn test_extract_metadata() {
        let args = BuildArgs { assertions: vec![] };
        let impl_data = json!({
            "contract": {
                "metadata": "{\"compiler\":{\"version\":\"0.8.28+commit.7893614a\"}}"
            }
        });

        let metadata = args.extract_metadata(&impl_data);
        assert!(metadata.is_ok());
        assert!(metadata.unwrap().contains("0.8.28"));
    }

    #[test]
    fn test_extract_bytecode() {
        let args = BuildArgs { assertions: vec![] };
        let impl_data = json!({
            "contract": {
                "evm": {
                    "bytecode": {
                        "object": "6080604052"
                    }
                }
            }
        });

        let bytecode = args.extract_bytecode(&impl_data);
        assert!(bytecode.is_ok());
        assert_eq!(bytecode.unwrap(), "6080604052");
    }

    #[test]
    fn test_process_contracts_with_multiple_assertions() {
        let args = BuildArgs {
            assertions: vec!["TestIncrement".to_string(), "Assertion".to_string()],
        };
        let json = get_sample_forge_output();
        let contracts = args.extract_contracts(&json).unwrap();

        let builds = args.process_contracts(contracts).unwrap();
        assert_eq!(builds.len(), 3); // Should find both TestIncrement implementations and the Assertion
    }

    #[test]
    fn test_process_contracts_with_nonexistent_assertion() {
        let args = BuildArgs {
            assertions: vec!["NonexistentContract".to_string()],
        };
        let json = get_sample_forge_output();
        let contracts = args.extract_contracts(&json).unwrap();

        let builds = args.process_contracts(contracts).unwrap();
        assert!(builds.is_empty());
    }

    #[test]
    fn test_extract_real_metadata() {
        let args = BuildArgs { assertions: vec![] };
        let json = get_sample_forge_output();
        let contracts = args.extract_contracts(&json).unwrap();

        // Get TestIncrement implementation data
        let test_increment_contract = contracts
            .get("/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/assertions/src/Assertion.sol")
            .unwrap()
            .get("TestIncrement")
            .unwrap()
            .as_array()
            .unwrap()
            .first()
            .unwrap();

        let metadata = args.extract_metadata(test_increment_contract).unwrap();
        assert!(metadata.contains("0.8.28+commit.7893614a"));
        assert!(metadata.contains("TestIncrement"));
        assert!(metadata.contains("assertions/src/Assertion.sol"));
    }

    #[test]
    fn test_extract_real_bytecode() {
        let args = BuildArgs { assertions: vec![] };
        let json = get_sample_forge_output();
        let contracts = args.extract_contracts(&json).unwrap();

        // Get TestIncrement implementation data
        let test_increment_contract = contracts
            .get("/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/assertions/src/Assertion.sol")
            .unwrap()
            .get("TestIncrement")
            .unwrap()
            .as_array()
            .unwrap()
            .first()
            .unwrap();

        let bytecode = args.extract_bytecode(test_increment_contract).unwrap();
        assert_eq!(bytecode, "6080604052348015600e575f5ffd5b50606280601a5f395ff3fe6080604052348015600e575f5ffd5b50600436106026575f3560e01c8063220ba53014602a575b5f5ffd5b00fea264697066735822122018124cd9024a76b0f76b2f8dadfc710b78dbc511767938c9833fddf657c8553c64736f6c634300081c0033");
    }

    #[test]
    fn test_all_contract_paths() {
        let args = BuildArgs { assertions: vec![] };
        let json = get_sample_forge_output();
        let contracts = args.extract_contracts(&json).unwrap();

        let expected_paths = vec![
            "/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/assertions/src/Assertion.sol",
            "/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/assertions/test/Increment.t.sol",
            "/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/lib/credible-std/src/Assertion.sol",
            "/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/lib/credible-std/src/Credible.sol",
            "/Users/odysseas/code/phylax/pcl/mocks/mock-protocol/lib/credible-std/src/PhEvm.sol",
        ];

        for path in expected_paths {
            assert!(contracts.contains_key(path));
        }
    }
}
