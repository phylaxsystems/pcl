use reqwest::Error as ReqwestError;
use pcl_phoundry::error::PhoundryError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaSubmitError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] ReqwestError),
    #[error("Submission failed: {0}")]
    SubmissionFailed(String),
    #[error("Build failed: {0}")]
    BuildError(#[from] PhoundryError),
}

#[derive(Error, Debug)]
pub enum DappSubmitError {
    #[error("No auth token found")]
    NoAuthToken,
    #[error("Project selection cancelled")]
    ProjectSelectionCancelled,
    #[error("Failed to connect to the dApp API")]
    ApiConnectionError(#[from] ReqwestError),
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    ReadError(std::io::Error),
    #[error("Failed to write config file: {0}")]
    WriteError(std::io::Error),
}
