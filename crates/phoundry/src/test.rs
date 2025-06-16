use clap::{
    Parser,
    ValueHint,
};
use color_eyre::Report;
use forge::{
    cmd::{
        build::BuildArgs,
        test::TestArgs,
    },
    opts::{
        Forge,
        ForgeSubcommand,
    },
};
use foundry_cli::{
    opts::{
        BuildOpts,
        ProjectPathOpts,
    },
    utils::LoadConfig,
};
use foundry_common::compile::ProjectCompiler;
use foundry_compilers::{
    flatten::{
        Flattener,
        FlattenerError,
    },
    info::ContractInfo,
    solc::SolcLanguage,
    ProjectCompileOutput,
};

use alloy_json_abi::JsonAbi;

use foundry_config::{
    error::ExtractConfigError,
    find_project_root,
};
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

impl PhorgeTest {
    /// Runs the test command in a separate blocking task.
    /// This prevents blocking the current runtime while executing the forge command.
    pub async fn run(self) -> Result<(), Box<PhoundryError>> {
        // Extract the Send-safe parts of the test args
        let test_args = self.test_args;
        let global_opts = test_args.global.clone();
        global_opts.init()?;
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