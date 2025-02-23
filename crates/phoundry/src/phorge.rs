use pcl_common::args::CliArgs;
use std::{
    env,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

use crate::error::PhoundryError;

const FORGE_BINARY_NAME: &str = "phorge";

// Remove the const and add a function to get the forge binary path
fn get_forge_binary_path() -> PathBuf {
    let exe_path = env::current_exe().expect("Failed to get current executable path");
    exe_path
        .parent()
        .expect("Failed to get executable directory")
        .join(FORGE_BINARY_NAME)
}

#[derive(clap::Parser)]
pub struct Phorge {
    pub args: Vec<String>,
}

impl Phorge {
    /// Run the forge command with the given arguments.
    /// Phoundry should be installed as part of the pcl workspace, meaning that we
    /// can assume that forge is available in the PATH.
    /// We do this so that we don't have to re-write the forge command in the CLI, as
    /// a lot of the functionality is implemented as part of the forge binary, which we can't import
    /// as a crate.
    pub fn run(&self, cli_args: CliArgs, print_output: bool) -> Result<Output, PhoundryError> {
        self.run_args(get_forge_binary_path(), cli_args, print_output)
    }

    fn run_args(
        &self,
        forge_bin_path: PathBuf,
        cli_args: CliArgs,
        print_output: bool,
    ) -> Result<Output, PhoundryError> {
        let mut args = self.args.clone();

        if let Some(ref root_dir) = cli_args.root_dir {
            args.push("--root".to_string());
            args.push(root_dir.to_str().unwrap().to_string());
        }

        let mut command = Command::new(forge_bin_path);

        command.args(args);

        if print_output {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
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

        Ok(command.output()?)
    }

    /// Check if forge is installed and available in the PATH.
    pub fn forge_must_be_installed() -> Result<(), PhoundryError> {
        if Command::new(get_forge_binary_path())
            .arg("--version")
            .output()
            .is_err()
        {
            return Err(PhoundryError::ForgeNotInstalled);
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::PathBuf;

    fn set_current_dir(path: &str) {
        std::env::set_current_dir(path).unwrap();
    }

    fn run_build_test(cli_args: CliArgs, phorge_bin_path: &str) {
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
            CliArgs {
                root_dir: Some(PathBuf::from("mock-protocol")),
                ..CliArgs::default()
            },
            "../target/release/phorge",
        );

        set_current_dir("mock-protocol");

        run_build_test(
            CliArgs {
                ..CliArgs::default()
            },
            "../../target/release/phorge",
        );
    }
}
