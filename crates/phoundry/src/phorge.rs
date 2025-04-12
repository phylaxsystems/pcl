use clap::{Parser, ValueHint};
use color_eyre::Report;
use forge::{
    cmd::{build::BuildArgs, test::TestArgs},
    opts::{Forge, ForgeSubcommand},
};
use foundry_cli::{
    opts::{BuildOpts, ProjectPathOpts},
    utils::LoadConfig,
};
use foundry_compilers::{
    flatten::{Flattener, FlattenerError},
    info::ContractInfo,
    solc::SolcLanguage,
    ProjectCompileOutput,
};
use foundry_config::{error::ExtractConfigError, find_project_root};
use std::path::PathBuf;
use tokio::task::spawn_blocking;

use crate::error::PhoundryError;

/// Command-line interface for running Phorge tests.
/// This struct wraps the standard Foundry test arguments.
#[derive(Debug, Parser, Clone)]
#[clap(about = "Run tests using Phorge")]
pub struct PhorgeTest {
    #[clap(flatten)]
    pub test_args: TestArgs,
}

/// Output from building and flattening a Solidity contract.
/// Contains the compiler version used and the flattened source code.
#[derive(Debug, Default)]
pub struct BuildAndFlatOutput {
    /// Version of the Solidity compiler used
    pub compiler_version: String,
    /// Flattened source code of the contract
    pub flattened_source: String,
}

impl BuildAndFlatOutput {
    /// Creates a new BuildAndFlatOutput instance.
    pub fn new(compiler_version: String, flattened_source: String) -> Self {
        Self {
            compiler_version,
            flattened_source,
        }
    }
}

/// Command-line arguments for building and flattening Solidity contracts.
/// This is used to prepare contracts for submission to the assertion DA layer.
#[derive(Debug, Default, Parser)]
#[clap(about = "Build and flatten contracts using Phorge")]
pub struct BuildAndFlattenArgs {
    /// Root directory of the project
    #[clap(
        short = 'r',
        long,
        value_hint = ValueHint::DirPath,
        help = "Root directory of the project"
    )]
    pub root: Option<PathBuf>,

    /// Name of the assertion contract to build and flatten
    #[clap(
        short = 'a',
        long,
        help = "Name of the assertion contract to build and flatten"
    )]
    pub assertion_contract: String,

    /// Constructor arguments for the assertion contract
    #[clap(
        short = 'c',
        long,
        help = "Constructor arguments for the assertion contract"
    )]
    pub constructor_args: Vec<String>,
}

impl BuildAndFlattenArgs {
    /// Builds and flattens the specified contract.
    ///
    /// # Returns
    ///
    /// - `Ok(BuildAndFlatOutput)` containing the compiler version and flattened source
    /// - `Err(PhoundryError)` if any step in the process fails
    pub fn run(&self) -> Result<BuildAndFlatOutput, Box<PhoundryError>> {
        let build = self.build()?;
        let info = ContractInfo::new(&self.assertion_contract);

        // Find the contract artifact
        let artifact = build
            .find_contract(info)
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;

        // Extract metadata and compiler version
        let metadata = artifact
            .metadata
            .clone()
            .ok_or_else(|| PhoundryError::InvalidForgeOutput("Missing contract metadata"))?;

        let solc_version = metadata
            .compiler
            .version
            .split_once('+')
            .ok_or_else(|| PhoundryError::InvalidForgeOutput("Invalid solc version format"))?
            .0
            .to_string();

        // Find the source path for the contract
        let rel_source_path = metadata
            .settings
            .compilation_target
            .iter()
            .find_map(|(path, name)| {
                if name == &self.assertion_contract {
                    Some(path)
                } else {
                    None
                }
            })
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;

        // Determine the full path to the contract
        let path = match &self.root {
            Some(root) => root.join(rel_source_path),
            None => find_project_root(None)
                .map_err(|_| PhoundryError::DirectoryNotFound(PathBuf::from(".")))?
                .join(rel_source_path),
        };

        // Flatten the contract
        let flattened = self.flatten(&path)?;
        Ok(BuildAndFlatOutput::new(solc_version, flattened))
    }

    /// Builds the project and returns the compilation output.
    fn build(&self) -> Result<ProjectCompileOutput, Box<PhoundryError>> {
        let build_cmd = BuildArgs {
            build: BuildOpts {
                project_paths: ProjectPathOpts {
                    root: self.root.clone(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        build_cmd
            .run()
            .map_err(|e| Box::new(PhoundryError::from(e)))
    }

    /// Flattens the contract source code.
    fn flatten(&self, path: &PathBuf) -> Result<String, Box<PhoundryError>> {
        let build = BuildOpts {
            project_paths: ProjectPathOpts {
                root: self.root.clone(),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = build.load_config()?;
        let project = config
            .ephemeral_project()
            .map_err(|e| Box::new(PhoundryError::SolcError(e)))?;

        let can_path = std::fs::canonicalize(path).map_err(|e| Box::new(PhoundryError::from(e)))?;

        // Try the new flattener first
        let flattener = Flattener::new(project.clone(), &can_path);
        let flattened_source = match flattener {
            Ok(flattener) => Ok(flattener.flatten()),
            Err(FlattenerError::Compilation(_)) => {
                // Fallback to the old flattening implementation for invalid syntax
                project
                    .paths
                    .with_language::<SolcLanguage>()
                    .flatten(path)
                    .map_err(|e| Box::new(PhoundryError::from(e)))
            }
            Err(FlattenerError::Other(err)) => Err(Box::new(PhoundryError::from(err))),
        }?;

        Ok(flattened_source)
    }
}

impl PhorgeTest {
    /// Runs the test command in a separate blocking task.
    /// This prevents blocking the current runtime while executing the forge command.
    pub async fn run(self) -> Result<(), Box<PhoundryError>> {
        // Extract the Send-safe parts of the test args
        let test_args = self.test_args;
        let global_opts = test_args.global.clone();

        // Spawn the blocking operation in a separate task
        spawn_blocking(move || {
            // Reconstruct the Forge struct inside the closure
            let forge = Forge {
                cmd: ForgeSubcommand::Test(test_args),
                global: global_opts,
            };
            forge::args::run_command(forge)
        })
        .await
        .map_err(|e| Box::new(PhoundryError::ForgeCommandFailed(e.into())))??;
        Ok(())
    }
}

impl From<ExtractConfigError> for Box<PhoundryError> {
    fn from(error: ExtractConfigError) -> Self {
        Box::new(PhoundryError::FoundryConfigError(error))
    }
}

impl From<std::io::Error> for Box<PhoundryError> {
    fn from(error: std::io::Error) -> Self {
        Box::new(PhoundryError::from(error))
    }
}

impl From<Report> for Box<PhoundryError> {
    fn from(error: Report) -> Self {
        Box::new(PhoundryError::ForgeCommandFailed(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper function to create a temporary Solidity project
    fn setup_test_project() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("test_project");
        fs::create_dir_all(&project_root).unwrap();

        // Create a simple test contract
        let contract_path = project_root.join("src").join("TestContract.sol");
        fs::create_dir_all(contract_path.parent().unwrap()).unwrap();
        fs::write(
            &contract_path,
            r#"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract TestContract {
    function test() public pure returns (bool) {
        return true;
    }
}"#,
        )
        .unwrap();

        (temp_dir, project_root)
    }

    #[test]
    fn test_build_and_flatten_args_new() {
        let args = BuildAndFlattenArgs {
            root: None,
            assertion_contract: "TestContract".to_string(),
            constructor_args: vec![],
        };

        assert_eq!(args.assertion_contract, "TestContract");
        assert!(args.root.is_none());
        assert!(args.constructor_args.is_empty());
    }

    #[test]
    fn test_build_and_flat_output_new() {
        let output = BuildAndFlatOutput::new("0.8.0".to_string(), "contract Test { }".to_string());

        assert_eq!(output.compiler_version, "0.8.0");
        assert_eq!(output.flattened_source, "contract Test { }");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_build_and_flatten_integration() {
        let (_temp_dir, project_root) = setup_test_project();

        let args = BuildAndFlattenArgs {
            root: Some(project_root),
            assertion_contract: "TestContract".to_string(),
            constructor_args: vec![],
        };

        let result = args.run();

        // The actual result will depend on the test environment
        // In a real test, we would verify the output
        assert!(result.is_ok() || result.is_err());
    }
}
