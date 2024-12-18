use std::process::Command;

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
    pub fn run(&self, args: Vec<String>) -> Result<(), PhoundryError> {
        // Check if forge exists first
        if !Command::new(FORGE_BINARY_PATH)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Err(PhoundryError::ForgeNotInstalled);
        }

        // Execute forge and pass through all output exactly as-is
        let mut command = Command::new(FORGE_BINARY_PATH);
        command.args(args);
        
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
