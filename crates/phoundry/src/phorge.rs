use forge::cmd::{build::BuildArgs, flatten::FlattenArgs, test::TestArgs};
use pcl_common::args::CliArgs;
use std::{
    env,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use clap::Parser;

use crate::{error::PhoundryError};

#[derive(clap::Parser)]
pub enum ForgeCmd {
    Test,
    Build,
    Flatten
}

#[derive(clap::Parser)]
pub struct Phorge {
    #[command(subcommand)]
    pub cmd: ForgeCmd,
    pub args: Vec<String>,
}

impl Phorge {
    /// Run the forge command with the given arguments.
    /// Phoundry should be installed as part of the pcl workspace, meaning that we
    /// can assume that forge is available in the PATH.
    /// We do this so that we don't have to re-write the forge command in the CLI, as
    /// a lot of the functionality is implemented as part of the forge binary, which we can't import
    /// as a crate.
    pub fn run(&self, cli_args: &CliArgs, print_output: bool) -> Result<Output, PhoundryError> {
        todo!()
    }

    pub async fn run_test(&self, cli_args: &CliArgs, print_output: bool) -> Result<(), PhoundryError> {
        let args = vec!["test"];
        let test_args: TestArgs = TestArgs::parse_from(args);
        test_args.run().await?;
        Ok(())
    }

    pub async fn run_build(&self, cli_args: &CliArgs, print_output: bool) -> Result<(), PhoundryError> {
        let args = vec!["build"];
        let build_args: BuildArgs = BuildArgs::parse_from(args);
        build_args.run().unwrap();
        Ok(())
    }

    pub async fn run_flatten(&self, cli_args: &CliArgs, print_output: bool) -> Result<(), PhoundryError> {
        let args = vec!["flatten"];
        let flatten_args: FlattenArgs = FlattenArgs::parse_from(args);
        let res = flatten_args.run().unwrap();
        Ok(())
    }

    fn build_args(
        &self,
        cli_args: &CliArgs,
        print_output: bool,
    ) -> Result<Output, PhoundryError> {
        let mut args = self.args.clone();

        if let Some(ref root_dir) = cli_args.root_dir {
            args.push("--root".to_string());
            args.push(root_dir.to_str().unwrap().to_string());
        }

        // Only valid for the context of this binary execution
        env::set_var(
            "FOUNDRY_SRC",
            cli_args.assertions_src().as_os_str().to_str().unwrap(),
        );

        env::set_var(
            "FOUNDRY_TEST",
            cli_args.assertions_test().as_os_str().to_str().unwrap(),
        );
        todo!()
    }

}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    fn set_current_dir(path: &str) {
        std::env::set_current_dir(path).unwrap();
    }

    fn run_build_test(cli_args: &CliArgs, phorge_bin_path: &str) {
        let phorge = Phorge {
            args: vec!["build".to_owned(), "--force".to_owned()],
        };

        let res = phorge
            .run_args(phorge_bin_path.into(), cli_args, true)
            .unwrap();

        assert!(res.status.success());
    }

    #[test]
    fn test_build_args_with_root_dir() {
        set_current_dir("../../testdata");

        run_build_test(
            &CliArgs {
                root_dir: Some(PathBuf::from("mock-protocol")),
                ..CliArgs::default()
            },
            "../target/debug/phorge",
        );

        set_current_dir("mock-protocol");

        run_build_test(
            &CliArgs {
                ..CliArgs::default()
            },
            "../../target/debug/phorge",
        );
    }
}
