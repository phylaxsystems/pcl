use clap::{Parser, ValueHint};
use forge::cmd::{build::BuildArgs, test::TestArgs};
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
use foundry_config::find_project_root;
use std::path::PathBuf;

use crate::error::PhoundryError;

/// Command-line arguments for running Phorge tests
#[derive(Debug, Parser)]
#[clap(about = "Run tests using Phorge")]
pub struct PhorgeTest {
    #[clap(flatten)]
    pub args: TestArgs,
}

/// Output from building and flattening a contract
#[derive(Debug, Default)]
pub struct BuildAndFlatOutput {
    /// Version of the compiler used
    pub compiler_version: String,
    /// Flattened source code
    pub flattened_source: String,
}

impl BuildAndFlatOutput {
    pub fn new(compiler_version: String, flattened_source: String) -> Self {
        Self {
            compiler_version,
            flattened_source,
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
    pub root: Option<PathBuf>,
    /// Name of the assertion contract
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
    pub constructor_args: Vec<String>
}

impl BuildAndFlattenArgs {
    /// Run the build and flatten process
    pub fn run(&self) -> Result<BuildAndFlatOutput, PhoundryError> {
        let build = self.build()?;
        let info = ContractInfo::new(&self.assertion_contract);
        let artifact = build
            .find_contract(info)
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;
        let metadata = artifact.metadata.clone().unwrap();
        let solc_version = metadata
            .compiler
            .version
            .split_once('+')
            .expect("Failed to split solc version")
            .0
            .to_string();
        let contract_name = &self.assertion_contract;
        let rel_source_path = metadata
            .settings
            .compilation_target
            .iter()
            .find_map(|(path, name)| {
                if name == contract_name {
                    Some(path)
                } else {
                    None
                }
            })
            .ok_or_else(|| PhoundryError::ContractNotFound(contract_name.to_string()))?;
        let path = match &self.root {
            Some(root) => root.join(rel_source_path),
            None => find_project_root(None).unwrap().join(rel_source_path),
        };
        let flattened = self.flatten(&path)?;
        dbg!(&flattened);
        dbg!(&solc_version);
        dbg!(&path);
        Ok(BuildAndFlatOutput::new(solc_version, flattened))
    }

    /// Build the project and return the compilation output
    fn build(&self) -> Result<ProjectCompileOutput, PhoundryError> {
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
        build_cmd.run().map_err(PhoundryError::from)
    }

    /// Flatten the contract source code
    fn flatten(&self, path: &PathBuf) -> Result<String, PhoundryError> {
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
            .map_err(PhoundryError::SolcError)?;
        let can_path = std::fs::canonicalize(path).map_err(PhoundryError::from)?;
        let flattener = Flattener::new(project.clone(), &can_path);
        let flattened_source = match flattener {
            Ok(flattener) => Ok(flattener.flatten()),
            Err(FlattenerError::Compilation(_)) => {
                // Fallback to the old flattening implementation for invalid syntax
                project
                    .paths
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