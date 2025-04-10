use std::path::PathBuf;

use clap::{Parser, ValueHint};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use pcl_common::{args::CliArgs, utils::get_build_info, Assertion};
use pcl_phoundry::phorge::{BuildAndFlattenArgs, PhorgeBuild};
use tokio::time::Duration;

use assertion_da_client::{DaClient, DaClientError};
use jsonrpsee_core::client::Error as ClientError;
use jsonrpsee_http_client::transport::Error as TransportError;

use crate::{
    config::{AssertionForSubmission, CliConfig},
    error::DaSubmitError,
};

#[derive(Parser)]
#[clap(
    name = "store",
    about = "Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer"
)]
pub struct DaStoreArgs {
    // FIXME (Odysseas): Replace localhost with the actual DA URL from our infrastructure
    /// URL of the assertion-DA
    #[clap(long, env = "PCL_DA_URL", default_value = "http://localhost:5001")]
    url: String,
    #[clap(flatten)]
    args: BuildAndFlattenArgs
}

impl DaStoreArgs{
    pub async fn run(
        &self,
        cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DaSubmitError> {
        let build_flatten_output= self.args.run()?;
        // Create a spinner to show progress w  hile submitting
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner} {msg}")
                .expect("Failed to set spinner style"),
        );
        spinner.enable_steady_tick(Duration::from_millis(80));
        spinner.set_message("Submitting assertion to DA...");

        // Submit the assertion
        let result = match DaClient::new(&self.url)?
            .submit_assertion(
                self.args.assertion_contract.clone(),
                build_flatten_output.flattened_source,
                build_flatten_output.compiler_version,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => {
                match err {
                    DaClientError::ClientError(ClientError::Transport(ref boxed_err)) => {
                        match boxed_err.downcast_ref::<TransportError>().unwrap() {
                            TransportError::Rejected { status_code } => {
                                match status_code {
                                    401 => {
                                        spinner.finish_with_message("❌ Assertion submission failed! Unauthorized. Please run pcl run.");
                                    }
                                    status_code => {
                                        spinner.finish_with_message(format!(
                                            "❌ Assertion submission failed! Status code: {}",
                                            status_code
                                        ));
                                    }
                                };

                                return Ok(());
                            }
                            _ => return Err(err.into()),
                        }
                    }
                    _ => return Err(err.into()),
                };
            }
        };

        let assertion_for_submission = AssertionForSubmission {
            assertion_contract: self.args.assertion_contract.to_string(),
            assertion_id: result.id.to_string(),
            signature: result.signature.to_string(),
        };
        config.add_assertion_for_submission(assertion_for_submission.clone());
        // Finish spinner with success message
        spinner.finish_with_message("✅ Assertion successfully submitted!");

        // Display formatted assertion information
        println!("\n\n{}", "Assertion Information".bold().green());
        println!("{}", "===================".green());
        println!("{}", assertion_for_submission);

        // Display next steps with highlighted command
        println!("\n{}", "Next Steps:".bold());
        println!("Submit this assertion to a project with:");
        println!(
            "  {} submit -a {} -p <project_name>",
            "pcl".cyan().bold(),
            self.args.assertion_contract
        );
        println!("Visit the Credible Layer DApp to link the assertion on-chain and enforce it:");
        println!("  {}", "https://dapp.credible.layer".cyan().bold());
        Ok(())
    }
}
