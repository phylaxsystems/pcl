use std::{path::PathBuf, str::FromStr};

use crate::{config::CliConfig, error::DaSubmitError};
use alloy_primitives::{hex, Bytes};
use clap::Parser;
use pcl_common::{args::CliArgs, utils::{bytecode, compilation_target, compiler_version}};
use pcl_phoundry::build::BuildArgs;

use assertion_da_client::DaClient;

#[derive(Parser)]
#[clap(
    name = "store",
    about = "Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer"
)]
pub struct DASubmitArgs {
    // FIXME(Odysseas): Replace localhost with the actual DA URL from our infrastructure
    /// URL of the assertion-DA
    #[clap(long, env = "PCL_DA_URL", default_value = "http://localhost:5001")]
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
        let relative_path= compilation_target(&self.assertion, &out_dir);
        let mut full_path= cli_args.root_dir();
        full_path.push(relative_path);

        let _result = build_args.run(cli_args)?;

        let flatten_contract = build_args.get_flattened_source(&full_path)?;
        let compiler_version = compiler_version(&self.assertion, &out_dir)
            .split('+')
            .next()
            .unwrap_or_default()
            .to_string();

        let result = DaClient::new(&self.url)?.submit_assertion(self.assertion.clone(), flatten_contract, compiler_version).await?;

        println!("Submitted assertion with id: {}", result.id);
        println!("Signature: {}", result.signature);
        Ok(())
    }


}
