use std::{env, path::Path, process::Command};

use pcl_common::args::CliArgs;
use thiserror::Error;

const FORGE_BINARY_PATH: &str = "forge";

#[derive(clap::Parser)]
pub struct Phoundry {
    pub args: Vec<String>,
}

impl Phoundry {
    /// Run the forge command with the given arguments.
    /// Phoundry should be installed as part of the pcl workspace, meaning that we 
    /// can assume that forge is available in the PATH.
    /// We do this so that we don't have to re-write the forge command in the CLI, as 
    /// a lot of the functionality is implemented as part of the forge binary, which we can't import
    /// as a crate.
    pub fn run(&self, cli_args: CliArgs, phoundry_args: Vec<String>) -> Result<(), PhoundryError> {

        // Execute forge and pass through all output exactly as-is
        let mut command = Command::new(FORGE_BINARY_PATH);

        command.args(phoundry_args);

        // Only valid for the context of this binary execution
        env::set_var("FOUNDRY_SRC", cli_args.assertions_src().as_os_str().to_str().unwrap());
        env::set_var("FOUNDRY_TEST", cli_args.assertions_test().as_os_str().to_str().unwrap());
        
        let output = command.output()?;


        
        // Pass through stdout/stderr exactly as forge produced them
        if !output.stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&output.stderr)); 
        }
        Ok(())
    }

    /// Check if forge is installed and available in the PATH.
    pub fn forge_must_be_installed() -> Result<(), PhoundryError> {
        if !Command::new(FORGE_BINARY_PATH)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Err(PhoundryError::ForgeNotInstalled);
        }
        Ok(())
    }

    pub fn build_assertions(&self, assertions_path: &Path) -> Result<(), PhoundryError> {
        let mut command = Command::new(FORGE_BINARY_PATH);
        command.args(["--force", "-C", assertions_path.as_os_str().to_str().unwrap()]);
        let output = command.output()?;
        
        // Pass through stdout/stderr exactly as forge produced them
        if !output.stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(&output.stderr)); 
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
}


#[cfg(test)]
mod tests {
    use super::*;

    const build_output: &str = include_str!("../../../testdata/forge-build-output.json");

    #[test]
    fn test_forge_must_be_installed() {
        assert!(Phoundry::forge_must_be_installed().is_ok());
    }
}