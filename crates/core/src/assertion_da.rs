use clap::{Parser, ValueHint};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use pcl_common::args::CliArgs;
use pcl_phoundry::phorge::BuildAndFlattenArgs;
use tokio::time::Duration;

use assertion_da_client::{DaClient, DaClientError};
use jsonrpsee_core::client::Error as ClientError;
use jsonrpsee_http_client::transport::Error as TransportError;

use crate::{
    config::{AssertionForSubmission, CliConfig},
    error::DaSubmitError,
};

/// Command-line arguments for storing assertions in the Data Availability layer
#[derive(Parser)]
#[clap(
    name = "store",
    about = "Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer"
)]
pub struct DaStoreArgs {
    /// URL of the assertion-DA
    #[clap(long, short = 'u', env = "PCL_DA_URL", value_hint = ValueHint::Url, default_value = "http://localhost:5001")]
    url: String,
    #[clap(flatten)]
    args: BuildAndFlattenArgs,
}

impl DaStoreArgs {
    /// Creates and configures a progress spinner
    fn create_spinner() -> ProgressBar {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner} {msg}")
                .expect("Failed to set spinner style"),
        );
        spinner.enable_steady_tick(Duration::from_millis(80));
        spinner
    }

    /// Handles HTTP error responses from the DA layer
    #[allow(clippy::result_large_err)]
    fn handle_http_error(status_code: u16, spinner: &ProgressBar) -> Result<(), DaSubmitError> {
        match status_code {
            401 => {
                spinner.finish_with_message(
                    "❌ Assertion submission failed! Unauthorized. Please run `pcl auth login`",
                );
                Ok(())
            }
            _ => {
                spinner.finish_with_message(format!(
                    "❌ Assertion submission failed! Status code: {}",
                    status_code
                ));
                Ok(())
            }
        }
    }

    /// Displays the assertion information and next steps
    fn display_success_info(&self, assertion: &AssertionForSubmission) {
        println!("\n\n{}", "Assertion Information".bold().green());
        println!("{}", "===================".green());
        println!("{}", assertion);

        println!("\n{}", "Next Steps:".bold());
        println!("Submit this assertion to a project with:");
        println!(
            "  {} submit -a {} -p <project_name>",
            "pcl".cyan().bold(),
            self.args.assertion_contract
        );
        println!("Visit the Credible Layer DApp to link the assertion on-chain and enforce it:");
        println!("  {}", "https://dapp.credible.layer".cyan().bold());
    }

    /// Executes the assertion storage process
    pub async fn run(
        &self,
        _cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DaSubmitError> {
        let build_flatten_output = self.args.run()?;
        let spinner = Self::create_spinner();
        spinner.set_message("Submitting assertion to DA...");

        let client = match &config.auth {
            Some(auth) => DaClient::new_with_auth(&self.url, &auth.access_token)?,
            None => DaClient::new(&self.url)?,
        };

        // Submit the assertion
        let result = match client
            .submit_assertion(
                self.args.assertion_contract.clone(),
                build_flatten_output.flattened_source,
                build_flatten_output.compiler_version,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => match err {
                DaClientError::ClientError(ClientError::Transport(ref boxed_err)) => {
                    if let Some(TransportError::Rejected { status_code }) = boxed_err.downcast_ref()
                    {
                        return Self::handle_http_error(*status_code, &spinner);
                    }
                    return Err(err.into());
                }
                _ => return Err(err.into()),
            },
        };

        let assertion_for_submission = AssertionForSubmission {
            assertion_contract: self.args.assertion_contract.to_string(),
            assertion_id: result.id.to_string(),
            signature: result.signature.to_string(),
        };

        config.add_assertion_for_submission(assertion_for_submission.clone());
        spinner.finish_with_message("✅ Assertion successfully submitted!");

        self.display_success_info(&assertion_for_submission);
        Ok(())
    }
}
