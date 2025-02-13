use reqwest::Error as ReqwestError;
use pcl_phoundry::PhoundryError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SubmitError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] ReqwestError),
    #[error("Submission failed: {0}")]
    SubmissionFailed(String),
    #[error("Build failed: {0}")]
    BuildError(#[from] PhoundryError),
}