use forge::cmd::{build::BuildArgs, flatten::FlattenArgs, test::TestArgs};
use foundry_cli::{opts::{BuildOpts, ProjectPathOpts}, utils::LoadConfig};
use foundry_compilers::{
    artifacts::{ConfigurableContractArtifact, Metadata}, flatten::{Flattener, FlattenerError}, info::ContractInfo, solc::SolcLanguage, ProjectCompileOutput
};
use pcl_common::args::CliArgs;
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio}, str::FromStr,
};
use clap::{Parser, ValueHint};

use crate::error::PhoundryError;

/// Command-line arguments for running Phorge tests
#[derive(Debug, Parser)]
#[clap(about = "Run tests using Phorge")]
pub struct PhorgeTest {
    #[clap(flatten)]
    pub args: TestArgs,
}

/// Configuration for building contracts with Phorge
#[derive(Debug)]
pub struct PhorgeBuild {
    /// Root directory of the project
    pub root: PathBuf,
    /// Path to the assertion file
    pub assertion_file: PathBuf,
}

/// Output from building and flattening a contract
#[derive(Debug, Default)]
pub struct BuildAndFlatOutput {
    /// Version of the compiler used
    pub compiler_version: String,
    /// Flattened source code
    pub flattened_source: String,
    // TODO(Odysseas): Add constructor args and check they are correct
}

impl BuildAndFlatOutput {
    pub fn new(compiler_version: String, flattened_source: String) -> Self {
        Self {
            compiler_version, flattened_source,
        }
    }
}

/// Command-line arguments for building and flattening contracts
#[derive(Debug, Parser)]
#[clap(about = "Build and flatten contracts using Phorge")]
pub struct BuildAndFlattenArgs {
    /// Root directory of the project
    #[clap(
        short = 'r',
        long,
        value_hint = ValueHint::DirPath,
        help = "Root directory of the project"
    )]
    pub root: PathBuf,
    /// Name of the assertion contract
    #[clap(
        short = 'a',
        long,
        help = "Name of the assertion contract to build and flatten"
    )]
    pub assertion_contract: String,
}

impl BuildAndFlattenArgs {
    /// Run the build and flatten process
    pub fn run(&self) -> Result<BuildAndFlatOutput, PhoundryError> {
        let build = self.build()?;
        let info = ContractInfo::new(&self.assertion_contract);
        let artifact = build.find_contract(info)
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;
        let metadata = artifact.metadata.clone().unwrap();
        let solc_version = metadata.compiler.version.split_once('+').expect("Failed to split solc version").0.to_string();
        let contract_name = &self.assertion_contract;
        let rel_source_path= metadata.settings.compilation_target.iter()
            .find_map(|(path, name)| if name == contract_name { Some(path) } else { None })
            .ok_or_else(|| PhoundryError::ContractNotFound(contract_name.to_string()))?;
        let path = self.root.join(rel_source_path);
        let flattened = self.flatten(&path)?;
        Ok(BuildAndFlatOutput::new(solc_version, flattened))
    }

    /// Build the project and return the compilation output
    fn build(&self) -> Result<ProjectCompileOutput, PhoundryError> {
        let build_cmd = BuildArgs {
            build: BuildOpts {
                project_paths: ProjectPathOpts {
                    root: Some(self.root.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        build_cmd.run().map_err(PhoundryError::from)
    }

    /// Flatten the contract source code
    fn flatten(&self, path: &PathBuf) -> Result<String, PhoundryError> {
        let build = BuildOpts {
            project_paths: ProjectPathOpts {
                root: Some(self.root.clone()),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = build.load_config()?;
        let project = config.ephemeral_project()
            .map_err(|e| PhoundryError::SolcError(e))?;

        let flattener = Flattener::new(project.clone(), path);
        let flattened_source = match flattener {
            Ok(flattener) => Ok(flattener.flatten()),
            Err(FlattenerError::Compilation(_)) => {
                // Fallback to the old flattening implementation for invalid syntax
                project.paths
                    .with_language::<SolcLanguage>()
                    .flatten(path)
                    .map_err(PhoundryError::from)
            }
            Err(FlattenerError::Other(err)) => Err(PhoundryError::from(err)),
        }?;
        Ok(flattened_source)
    }
}

impl PhorgeTest {
    /// Run the test command
    pub async fn run(self) -> Result<(), PhoundryError> {
        self.args.run().await?;
        Ok(())
    }
}

// Helper functions
impl PhorgeBuild {
    /// Create a new PhorgeBuild instance
    pub fn new(root: impl Into<PathBuf>, assertion_file: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            assertion_file: assertion_file.into(),
        }
    }

    /// Validate the build configuration
    pub fn validate(&self) -> Result<(), PhoundryError> {
        if !self.root.exists() {
            return Err(PhoundryError::DirectoryNotFound(self.root.clone()));
        }
        if !self.assertion_file.exists() {
            return Err(PhoundryError::FileNotFound(self.assertion_file.clone()));
        }
        Ok(())
    }
}