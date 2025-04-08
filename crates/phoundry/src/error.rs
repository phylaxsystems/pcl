use std::fmt::Debug;
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
}
