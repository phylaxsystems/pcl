use std::{fmt::Debug, path::{Path, PathBuf}};
use thiserror::Error;
use color_eyre::Report;
use foundry_compilers::error::SolcError;

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
    #[error("Contract {0} was not found in the build output")]
    ContractNotFound(String),
    #[error("Invalid path: {0:?}")]
    InvalidPath(PathBuf),
    #[error("Directory not found: {0:?}")]
    DirectoryNotFound(PathBuf),
    #[error("File not found: {0:?}")]
    FileNotFound(PathBuf),
    #[error("Solc error: {0}")]
    SolcError(#[from] SolcError),
}
