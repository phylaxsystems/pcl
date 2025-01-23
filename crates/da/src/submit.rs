use alloy_primitives::keccak256;
use pcl_common::{args::CliArgs, utils::bytecode};
use pcl_phoundry::build::BuildArgs;
use pcl_phoundry::PhoundryError;
use reqwest::{blocking::Client, Error as ReqwestError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Deserialize)]
struct SubmissionResponse {
    status: String,
    id: String,
}

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: Vec<String>,
    id: u64,
}

#[derive(clap::Parser)]
pub struct DASubmitArgs {
    /// Name of the assertion contract to submit
    assertion: String,
}

impl DASubmitArgs {
    pub fn run(&self, cli_args: CliArgs) -> Result<(), SubmitError> {
        let build_args = BuildArgs {
            assertions: vec![self.assertion.clone()],
        };

        build_args.run(cli_args)?;

        let client = Client::new();
        let endpoint = "https://da.credible.xyz"; // Fixed endpoint

        let artifact_path = format!("{}.sol:{}", self.assertion, self.assertion);
        let bytecode = bytecode(&artifact_path);

        // Calculate keccak256 hash of bytecode
        let id = keccak256(bytecode.as_bytes());

        // Create JSON-RPC request
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "da_submit_assertion".to_string(),
            params: vec![
                format!("0x{}", id.to_string()),       // keccak256 hash as id
                format!("0x{}", bytecode.to_string()), // code
            ],
            id: 1,
        };

        // Submit to assertion-DA
        let response = client.post(endpoint).json(&request).send()?;

        if !response.status().is_success() {
            return Err(SubmitError::SubmissionFailed(response.status().to_string()));
        }

        let result: SubmissionResponse = response.json()?;
        println!(
            "Submitted assertion '{}': ID {}: Status {}",
            self.assertion, result.id, result.status
        );

        Ok(())
    }
}

#[derive(Error, Debug)]
pub enum SubmitError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] ReqwestError),
    #[error("Submission failed: {0}")]
    SubmissionFailed(String),
    #[error("Build failed: {0}")]
    BuildError(#[from] PhoundryError),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use mockito::Server;

    #[test]
    fn test_submit_assertion() {
        let mut server = Server::new();

        // Print current directory before and after change
        println!("Current dir before: {:?}", std::env::current_dir().unwrap());
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir("testdata/mock-protocol").unwrap();
        println!("Current dir after: {:?}", std::env::current_dir().unwrap());
        println!("Assertions dir: {:?}", PathBuf::from("assertions"));

        let mock = server
            .mock("POST", "/")
            .match_body(r#"{"jsonrpc":"2.0","method":"da_submit_assertion","params":["0x1234","0x5678"],"id":1}"#)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","result":{"id":"0x1234","status":"success"},"id":1}"#)
            .create();

        let args = DASubmitArgs {
            assertion: "OwnableAssertion.sol".to_string(),
        };

        let result = args.run(CliArgs {
            assertions_dir: Some(PathBuf::from("assertions")),
        });

        std::env::set_current_dir(original_dir).unwrap();
        println!("Result: {:?}", result);
        assert!(result.is_ok());
        mock.assert();
    }

    #[test]
    fn test_submit_assertion_failure() {
        let mut server = Server::new();

        let mock = server
            .mock("POST", "/")
            .with_status(400)
            .with_body(
                r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":1}"#,
            )
            .create();

        let args = DASubmitArgs {
            assertion: "TestAssertion".to_string(),
        };

        let result = args.run(CliArgs::default());
        assert!(matches!(result, Err(SubmitError::SubmissionFailed(_))));
        mock.assert();
    }
}
