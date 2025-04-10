use std::{fmt::Debug, path::{Path, PathBuf}};
use thiserror::Error;
use color_eyre::Report;

#[derive(Error, Debug)]
pub enum PhoundryError {
    #[error("forge is not installed or not available in PATH")]
    ForgeNotInstalled,
    #[error("forge command failed")]
    ForgeCommandFailed(#[from] color_eyre::Report),
    #[error("invalid forge output: {0}")]
    InvalidForgeOutput(&'static str),
    #[error("invalid forge command: {0}")]
    InvalidForgeCommand(String),
    #[error("Phoundry profile {0} was not found in config {1}")]
    InvalidFoundryProfile(String, PathBuf),
    #[error("Phoundry failed to extract the config: {0}")]
    FoundryConfigError(#[from] foundry_config::error::ExtractConfigError),
}
