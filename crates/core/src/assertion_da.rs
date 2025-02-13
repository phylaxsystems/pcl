use alloy_primitives::keccak256;
use pcl_common::{args::CliArgs, utils::bytecode};
use pcl_phoundry::build::BuildArgs;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::error::SubmitError;

#[derive(Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    result: SubmissionResponse,
    id: u64,
}

#[derive(Deserialize)]
struct SubmissionResponse {
    status: String,
    id: String,
}

#[derive(Serialize)]
struct JsonRpcRequest {
    json_rpc: String,
    method: String,
    params: Vec<String>,
    id: u64,
}

#[derive(clap::Parser)]
pub struct DASubmitArgs {
    // FIXME(Odysseas): Replace localhost with the actual DA URL from our infrastructure
    /// URL of the assertion-DA
    #[clap(long, env = "PCL_DA_URL", default_value = "http://localhost:3000")]
    url: String,
    /// Name of the assertion contract to submit
    assertion: String,
}

impl DASubmitArgs {
    pub async fn run(&self, cli_args: CliArgs) -> Result<(), SubmitError> {
        let build_args = BuildArgs {
            assertions: vec![self.assertion.clone()],
        };

        build_args.run(cli_args)?;
        let bytecode = self.get_bytecode(&self.assertion)?;
        let id = self.calculate_id(&bytecode)?;
        let request = self.create_jsonrpc_request(&id, &bytecode)?;
        self.submit_request(&request).await
    }

    fn get_bytecode(&self, assertion: &str) -> Result<String, SubmitError> {
        let artifact_path = format!("{}.sol:{}", assertion, assertion);
        Ok(bytecode(&artifact_path))
    }

    fn calculate_id(&self, bytecode: &str) -> Result<String, SubmitError> {
        // TODO: Need to align with the correct calculation of the id
        let id = keccak256(bytecode.as_bytes());
        Ok(id.to_string())
    }

    fn create_jsonrpc_request(
        &self,
        id: &str,
        bytecode: &str,
    ) -> Result<JsonRpcRequest, SubmitError> {
        Ok(JsonRpcRequest {
            json_rpc: "2.0".to_string(),
            method: "da_submit_assertion".to_string(),
            params: vec![
                format!("0x{}", id),       // keccak256 hash as id
                format!("0x{}", bytecode), // code
            ],
            id: 1,
        })
    }

    async fn submit_request(&self, request: &JsonRpcRequest) -> Result<(), SubmitError> {
        let client = Client::new();
        let response = client.post(&self.url).json(request).send().await?;

        if !response.status().is_success() {
            return Err(SubmitError::SubmissionFailed(response.status().to_string()));
        }

        let result: JsonRpcResponse = response.json().await?;
        println!(
            "Submitted assertion '{}': ID {}: Status {}",
            self.assertion, result.result.id, result.result.status
        );

        Ok(())
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;

    #[test]
    fn test_calculate_id() {
        let args = DASubmitArgs {
            url: "http://test".to_string(),
            assertion: "TestAssertion.sol".to_string(),
        };
        let result = args.calculate_id("sample_bytecode");
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn test_create_jsonrpc_request() {
        let args = DASubmitArgs {
            url: "http://test".to_string(),
            assertion: "TestAssertion.sol".to_string(),
        };
        let request = args
            .create_jsonrpc_request("test_id", "test_bytecode")
            .unwrap();
        assert_eq!(request.json_rpc, "2.0");
        assert_eq!(request.method, "da_submit_assertion");
        assert_eq!(request.params.len(), 2);
        assert_eq!(request.params[0], "0xtest_id");
        assert_eq!(request.params[1], "0xtest_bytecode");
    }

    #[tokio::test]
    async fn test_submit_request() {
        let mut server = Server::new();
        let mock = server
            .mock("POST", "/")
            .match_body(r#"{"jsonrpc":"2.0","method":"da_submit_assertion","params":["0xtest_id","0xtest_bytecode"],"id":1}"#)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","result":{"id":"0xtest_id","status":"success"},"id":1}"#)
            .create();

        let args = DASubmitArgs {
            url: server.url(),
            assertion: "TestAssertion.sol".to_string(),
        };

        let request = args
            .create_jsonrpc_request("test_id", "test_bytecode")
            .unwrap();
        let result = args.submit_request(&request).await;
        assert!(result.is_ok());
        mock.assert();
    }

    #[tokio::test]
    async fn test_submit_request_failure() {
        let mut server = Server::new();
        let mock = server
            .mock("POST", "/")
            .with_status(400)
            .with_body(
                r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":1}"#,
            )
            .create();

        let args = DASubmitArgs {
            url: server.url(),
            assertion: "TestAssertion.sol".to_string(),
        };

        let request = args
            .create_jsonrpc_request("test_id", "test_bytecode")
            .unwrap();
        let result = args.submit_request(&request).await;
        assert!(matches!(result, Err(SubmitError::SubmissionFailed(_))));
        mock.assert();
    }
}
