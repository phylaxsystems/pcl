use std::fmt::Debug;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PhoundryError {
    #[error("forge is not installed or not available in PATH")]
    ForgeNotInstalled,
    #[error("forge command failed")]
    ForgeCommandFailed(#[from] std::io::Error),
    #[error("invalid forge output: {0}")]
    InvalidForgeOutput(&'static str),
}
