use crate::{
    config::CliConfig,
    error::DaSubmitError,
};
use alloy_primitives::{hex, Bytes};
use clap::Parser;
use pcl_common::{args::CliArgs, utils::bytecode};
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
