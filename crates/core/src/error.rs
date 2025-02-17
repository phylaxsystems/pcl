use pcl_phoundry::error::PhoundryError;
use reqwest::Error as ReqwestError;
use thiserror::Error;

/// Errors that can occur during assertion submission to the Data Availability (DA) layer
#[derive(Error, Debug)]
pub enum DaSubmitError {
    /// Error when HTTP request to the DA layer fails
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] ReqwestError),

    /// Error when the submission is rejected by the DA layer
    #[error("Submission failed: {0}")]
    SubmissionFailed(String),

    /// Error during the build process of the assertion
    #[error("Build failed: {0}")]
    BuildError(#[from] PhoundryError),
}

/// Errors that can occur during assertion submission to the Credible Layer dApp
#[derive(Error, Debug)]
pub enum DappSubmitError {
    /// Error when no authentication token is found in the config
    #[error("No auth token found")]
    NoAuthToken,

    /// Error when user cancels the project selection process
    #[error("Project selection cancelled")]
    ProjectSelectionCancelled,

    /// Error when connection to the dApp API fails
    #[error("Failed to connect to the dApp API")]
    ApiConnectionError(#[from] ReqwestError),

    /// Error when the submission is rejected by the dApp
    #[error("Submission failed: {0}")]
    SubmissionFailed(String),
}

/// Errors that can occur during configuration operations
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Error when reading the config file from ~/.pcl/config.toml fails
    #[error("Failed to read config file: {0}")]
    ReadError(std::io::Error),

    /// Error when writing to the config file at ~/.pcl/config.toml fails
    #[error("Failed to write config file: {0}")]
    WriteError(std::io::Error),

    /// Error when attempting an operation that requires authentication
    /// but no authentication token is present in the config
    #[error("No Authentication Token Found")]
    NotAuthenticated,
}

/// Errors that can occur during authentication operations
#[derive(Error, Debug)]
pub enum AuthError {
    /// Error when HTTP request to the auth service fails
    #[error(
        "Authentication request failed. Please check your connection and try again.\nError: {0}"
    )]
    RequestFailed(#[from] reqwest::Error),

    /// Error when authentication times out
    #[error("Authentication timed out after {0} attempts. Please try again and approve the wallet connection promptly.")]
    Timeout(u32),

    /// Error when authentication verification fails
    #[error("Authentication failed: {0}")]
    InvalidAuthData(String),

    /// Error when config operations fail during auth
    #[error("Config error: {0}")]
    ConfigError(#[from] ConfigError),

    /// Error when an invalid Ethereum address is received
    #[error(
        "Invalid Ethereum address received. Please ensure you're connecting with a valid wallet."
    )]
    InvalidAddress,

    /// Error when an invalid timestamp format is received
    #[error("Invalid timestamp received from server. Please try again.")]
    InvalidTimestamp,
}
