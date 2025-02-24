use crate::{
    config::CliConfig,
    error::{DaClientError, DaSubmitError},
};
use alloy_primitives::{hex, Bytes, B256};
use clap::Parser;
use pcl_common::{args::CliArgs, utils::bytecode};
use pcl_phoundry::build::BuildArgs;

use jsonrpsee::{
    core::client::ClientT,
    http_client::{HttpClient, HttpClientBuilder},
};

use serde::{Deserialize, Serialize};

#[derive(Parser)]
#[clap(
    name = "store",
    about = "Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer"
)]
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

        let out_dir = cli_args.out_dir();
        let result = build_args.run(cli_args)?;

        if !result.status.success() {
            eprintln!("Failed to build assertion contracts.");
            std::process::exit(1);
        }

        let bytecode: Bytes = hex::decode(bytecode(&self.assertion, out_dir))?.into();
        let result = DaClient::new(&self.url)?.submit_assertion(bytecode).await?;

        println!("Submitted assertion with id: {}", result.id);
        println!("Signature: {}", result.signature);
        Ok(())
    }
}

// TODO(Greg): Move this to a crate in the assertion-da repo, leverage here and in the
// assertion-executor
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
        Ok(self
            .client
            .request::<DaSubmissionResponse, &[String]>("da_submit_assertion", &[code.to_string()])
            .await?)
    }
}
