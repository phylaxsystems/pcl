use clap::Parser;
use pcl_common::args::CliArgs;

use crate::{error::PhoundryError, phorge::Phorge};

#[derive(Parser)]
pub struct BuildArgs {
    pub assertions: Vec<String>,
}

impl BuildArgs {
    pub fn run(&self, cli_args: CliArgs) -> Result<std::process::Output, PhoundryError> {
        let args = [
            "build",
            "-C",
            cli_args
                .assertions_dir()
                .as_path()
                .to_str()
                .unwrap(),
            "--root",
            cli_args.root_dir().as_path().to_str().unwrap(),
        ].iter().map(|s| s.to_string()).collect();

        // args.push("--out");
        // args.push(cli_args.out_dir_joined().as_path().to_str().unwrap());

        println!("Running phorge with args: {:?}", args);

        Phorge { args }.run(cli_args.clone(), true)
    }

    pub fn get_flattened_source(&self, path: &str) -> Result<String, PhoundryError> {
        let flatten_args = vec!["flatten".to_string(), path.to_string()];
        let phoundry = Phorge { args: flatten_args };
        let flatten_output = phoundry.run(CliArgs::default(), false)?;
        Ok(String::from_utf8_lossy(&flatten_output.stdout).to_string())
    }
}
