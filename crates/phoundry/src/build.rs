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
        let args = vec!["build", "--force", "-C", cli_args.assertions_dir().as_os_str().to_str().unwrap(), "--json"].iter().map(|s| s.to_string()).collect();
                    let phoundry = Phoundry { args };
        let output = phoundry.run(cli_args, false)?;
        if !output.stdout.is_empty() {
            let json_output: serde_json::Value = serde_json::from_slice(&output.stdout)
                .map_err(|_| PhoundryError::InvalidForgeOutput("invalid json output"))?;

            let contracts = json_output.get("contracts")
                .and_then(|c| c.as_object())
                .ok_or(PhoundryError::InvalidForgeOutput("invalid contracts field"))?;

            let mut assertion_builds = Vec::new();

            for (path, contract_data) in contracts {
                let contracts_obj = contract_data.as_object()
                    .ok_or(PhoundryError::InvalidForgeOutput("invalid contract data format"))?;

                for (contract_name, implementations) in contracts_obj {
                    if !self.assertions.contains(&contract_name) {
                        continue;
                    }

                    let implementations = implementations.as_array()
                        .ok_or(PhoundryError::InvalidForgeOutput("invalid implementations format"))?;

                    for impl_data in implementations {

                        let compiler_metadata = impl_data.get("contract").and_then(|c| c.get("metadata")).and_then(|m| m.as_str()).ok_or(PhoundryError::InvalidForgeOutput("missing or invalid metadata"))?.to_string();
                        // Extract bytecode using chained get() calls
                        let bytecode = impl_data
                            .get("contract")
                            .and_then(|c| c.get("evm"))
                            .and_then(|e| e.get("bytecode"))
                            .and_then(|b| b.get("object"))
                            .and_then(|o| o.as_str())
                            .ok_or(PhoundryError::InvalidForgeOutput("missing or invalid bytecode"))?.to_string();

                        // Get flattened source
                        let flatten_args = vec!["flatten".to_string(), path.to_string()];
                        let phoundry = Phoundry { args: flatten_args };
                        let flatten_output = phoundry.run(CliArgs::default(), false)?;
                        let source = String::from_utf8_lossy(&flatten_output.stdout).to_string();

                        assertion_builds.push(AssertionBuildOutput {
                            contract_name: contract_name.clone(),
                            bytecode,
                            source,
                            compiler_metadata
                        });
                    }
                }
            }
        }
        if !output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&output.stderr)); 
        }
         Ok(())
    }
}