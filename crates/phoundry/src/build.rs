use std::path::Path;

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

    pub fn get_flattened_source(
        &self,
        path: &Path,
        cli_args: &CliArgs,
    ) -> Result<String, PhoundryError> {
        todo!()
    }
}
