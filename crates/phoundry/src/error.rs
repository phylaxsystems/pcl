use thiserror::Error;
use std::fmt::Debug;

#[derive(Error, Debug)]
pub enum PhoundryError {
    #[error("forge is not installed or not available in PATH")]
    ForgeNotInstalled,
    #[error("forge command failed")]
    ForgeCommandFailed(#[from] std::io::Error),
    #[error("invalid forge output")]
    InvalidForgeOutput(&'static str),
}
