use std::path::PathBuf;

use clap::Parser;
use pcl_common::args::CliArgs;

use crate::{error::PhoundryError, phorge::Phorge};

#[derive(Parser)]
pub struct BuildArgs {
    pub assertions: Vec<String>,
}

impl BuildArgs {
    pub fn run(&self, cli_args: &CliArgs) -> Result<std::process::Output, PhoundryError> {
        let args = vec!["build".to_owned()];
        Phorge { args }.run(cli_args, false)
    }

    pub fn get_flattened_source(&self, path: &PathBuf, cli_args: &CliArgs) -> Result<String, PhoundryError> {
        let flatten_args = vec!["flatten".to_string(), path.to_string_lossy().to_string()];
        let phoundry = Phorge { args: flatten_args };
        let flatten_output = phoundry.run(cli_args, false)?;
        Ok(String::from_utf8_lossy(&flatten_output.stdout).to_string())
    }
}
