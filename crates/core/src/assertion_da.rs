use crate::{
    config::CliConfig,
    error::{DaClientError, DaSubmitError},
};
use alloy_primitives::{keccak256, Bytes, B256, hex};
use pcl_common::{args::CliArgs, utils::bytecode};
use pcl_phoundry::build::BuildArgs;

use jsonrpsee::{http_client::{HttpClient, HttpClientBuilder}, core::client::ClientT};

use serde::{Deserialize, Serialize};



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
    pub async fn run(
        &self,
        cli_args: CliArgs,
        _config: &mut CliConfig,
    ) -> Result<(), DaSubmitError> {
        let build_args = BuildArgs {
            assertions: vec![self.assertion.clone()],
        };

        let root_dir = cli_args.root_dir();
        build_args.run(cli_args)?;

        let bytecode : Bytes = hex::decode(bytecode(&self.assertion, root_dir))?.into();
        let da_client = DaClient::new(&self.url)?;
        let result = da_client.submit_assertion(bytecode).await?;

        println!("Submitted assertion with id: {}", result.id);
        println!("Signature: {}", result.signature);
        Ok(())

    }

}

#[derive(Debug)]
pub struct DaClient {
    client: HttpClient,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DaSubmissionResponse {
    id: B256,
    signature: Bytes,

}

#[derive(Debug, Deserialize, Serialize)]
pub struct DaFetchResponse {
    bytecode: Bytes,
    signature: Bytes,
}

impl DaClient {
    /// Create a new DA client
    pub fn new(da_url: &str) -> Result<Self, DaClientError> {
        let client = HttpClientBuilder::default().build(da_url)?;

        Ok(Self { client })
    }

    /// Fetch the bytecode for the given assertion id from the DA layer
    pub async fn fetch_assertion_bytecode(
        &self,
        assertion_id: B256,
    ) -> Result<Bytes, DaClientError> {
        let response = self
            .client
            .request::<DaFetchResponse, &[String]>("da_get_assertion", &[assertion_id.to_string()])
            .await?;

        Ok(response.bytecode)
    }

    /// Submit the assertion bytecode to the DA layer
    pub async fn submit_assertion(
        &self,
        code: Bytes,
    ) -> Result<DaSubmissionResponse, DaClientError> {
       Ok(self.client
            .request::<DaSubmissionResponse, &[String]>(
                "da_submit_assertion",
                &[code.to_string()],
            )
            .await?)
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
        let mut server = Server::new_async().await;
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
        let mut server = Server::new_async().await;
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
        assert!(matches!(result, Err(DaSubmitError::SubmissionFailed(_))));
        mock.assert();
    }
}
