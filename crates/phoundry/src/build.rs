use std::path::PathBuf;
use clap::Parser;
use pcl_common::args::CliArgs;

use crate::{Phoundry, PhoundryError};

#[derive(Parser)]
pub struct BuildArgs {
    pub assertions: Vec<String>
}

#[derive(Debug, Default)]
struct AssertionBuildOutput {
    pub contract_name: String,
    pub bytecode: String,
    pub source: String,
    pub compiler_metadata: String
}

impl BuildArgs {
    pub fn run(&self, cli_args: CliArgs) -> Result<(), PhoundryError> {
        let build_output = self.execute_forge_build(&cli_args)?;
        
        if !build_output.stdout.is_empty() {
            let json_output = self.parse_forge_output(&build_output.stdout)?;
            let contracts = self.extract_contracts(&json_output)?;
            let assertion_builds = self.process_contracts(&contracts)?;
        }

        if !build_output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&build_output.stderr));
        }
        Ok(())
    }

    fn execute_forge_build(&self, cli_args: &CliArgs) -> Result<std::process::Output, PhoundryError> {
        let args = vec!["build", "--force", "-C", cli_args.assertions_dir().as_os_str().to_str().unwrap(), "--json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let phoundry = Phoundry { args };
        phoundry.run(cli_args.clone(), false)
    }

    fn parse_forge_output(&self, stdout: &[u8]) -> Result<serde_json::Value, PhoundryError> {
        serde_json::from_slice(stdout)
            .map_err(|_| PhoundryError::InvalidForgeOutput("invalid json output"))
    }

    fn extract_contracts<'a>(&'a self, json_output: &'a serde_json::Value) -> Result<&serde_json::Map<String, serde_json::Value>, PhoundryError> {
        json_output.get("contracts")
            .and_then(|c| c.as_object())
            .ok_or(PhoundryError::InvalidForgeOutput("invalid contracts field"))
    }

    fn process_contracts(
        &self,
        contracts: &serde_json::Map<String, serde_json::Value>
    ) -> Result<Vec<AssertionBuildOutput>, PhoundryError> {
        let mut assertion_builds = Vec::new();

        for (path, contract_data) in contracts {
            let contracts_obj = contract_data.as_object()
                .ok_or(PhoundryError::InvalidForgeOutput("invalid contract data format"))?;

            for (contract_name, implementations) in contracts_obj {
                if !self.assertions.contains(contract_name) {
                    continue;
                }
                
                let builds = self.process_implementations(implementations, path, contract_name)?;
                assertion_builds.extend(builds);
            }
        }

        Ok(assertion_builds)
    }

    fn process_implementations(
        &self,
        implementations: &serde_json::Value,
        path: &str,
        contract_name: &str,
    ) -> Result<Vec<AssertionBuildOutput>, PhoundryError> {
        let mut builds = Vec::new();
        let implementations = implementations.as_array()
            .ok_or(PhoundryError::InvalidForgeOutput("invalid implementations format"))?;

        for impl_data in implementations {
            let compiler_metadata = self.extract_metadata(impl_data)?;
            let bytecode = self.extract_bytecode(impl_data)?;
            let source = self.get_flattened_source(path)?;

            builds.push(AssertionBuildOutput {
                contract_name: contract_name.to_string(),
                bytecode,
                source,
                compiler_metadata,
            });
        }

        Ok(builds)
    }

    fn extract_metadata(&self, impl_data: &serde_json::Value) -> Result<String, PhoundryError> {
        impl_data
            .get("contract")
            .and_then(|c| c.get("metadata"))
            .and_then(|m| m.as_str())
            .map(String::from)
            .ok_or(PhoundryError::InvalidForgeOutput("missing or invalid metadata"))
    }

    fn extract_bytecode(&self, impl_data: &serde_json::Value) -> Result<String, PhoundryError> {
        impl_data
            .get("contract")
            .and_then(|c| c.get("evm"))
            .and_then(|e| e.get("bytecode"))
            .and_then(|b| b.get("object"))
            .and_then(|o| o.as_str())
            .map(String::from)
            .ok_or(PhoundryError::InvalidForgeOutput("missing or invalid bytecode"))
    }

    fn get_flattened_source(&self, path: &str) -> Result<String, PhoundryError> {
        let flatten_args = vec!["flatten".to_string(), path.to_string()];
        let phoundry = Phoundry { args: flatten_args };
        let flatten_output = phoundry.run(CliArgs::default(), false)?;
        Ok(String::from_utf8_lossy(&flatten_output.stdout).to_string())
    }
}