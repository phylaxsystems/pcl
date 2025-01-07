use std::{
    env,
    path::PathBuf,
    process::{Command, Output},
};

use pcl_common::args::CliArgs;
use thiserror::Error;

pub mod build;

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
        // Execute forge and pass through all output exactly as-is
        let mut command = Command::new(get_forge_binary_path());

        command.args(self.args.clone());

        // Only valid for the context of this binary execution
        env::set_var(
            "FOUNDRY_SRC",
            cli_args.assertions_src().as_os_str().to_str().unwrap(),
        );
        env::set_var(
            "FOUNDRY_TEST",
            cli_args.assertions_test().as_os_str().to_str().unwrap(),
        );

        let output = command.output()?;

        // Pass through stdout/stderr exactly as forge produced them
        if print_output && !output.stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        }
        Ok(output)
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

#[derive(Error, Debug)]
pub enum PhoundryError {
    #[error("forge is not installed or not available in PATH")]
    ForgeNotInstalled,
    #[error("forge command failed")]
    ForgeCommandFailed(#[from] std::io::Error),
    #[error("invalid forge output")]
    InvalidForgeOutput(&'static str),
}
