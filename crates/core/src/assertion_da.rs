//! Module for handling assertion submission to the Data Availability (DA) layer.
//!
//! This module provides functionality to submit assertions to a Data Availability layer,
//! which is a crucial component of the Credible Layer system. It handles the process
//! of building, flattening, and submitting assertions along with their source code
//! to be stored in the DA layer.

use clap::{Parser, ValueHint};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use pcl_common::args::CliArgs;
use pcl_phoundry::phorge::{BuildAndFlatOutput, BuildAndFlattenArgs};
use tokio::time::Duration;

use assertion_da_client::{DaClient, DaClientError, DaSubmissionResponse};
use jsonrpsee_core::client::Error as ClientError;
use jsonrpsee_http_client::transport::Error as TransportError;

use crate::{
    config::{AssertionForSubmission, CliConfig},
    error::DaSubmitError,
};

/// Command-line arguments for storing assertions in the Data Availability layer.
///
/// This struct handles the configuration needed to submit assertions to the DA layer,
/// including the DA server URL and build arguments for the assertion.
#[derive(Parser)]
#[clap(
    name = "store",
    about = "Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer"
)]
pub struct DaStoreArgs {
    /// URL of the assertion-DA server
    #[clap(
        long,
        short = 'u',
        env = "PCL_DA_URL",
        value_hint = ValueHint::Url,
        default_value = "http://localhost:5001"
    )]
    url: String,

    /// Build and flatten arguments for the assertion
    #[clap(flatten)]
    args: BuildAndFlattenArgs,
}

impl DaStoreArgs {
    /// Creates and configures a progress spinner for displaying operation status.
    ///
    /// Returns a configured `ProgressBar` instance with a custom spinner style.
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

    /// Handles HTTP error responses from the DA layer.
    ///
    /// # Arguments
    /// * `status_code` - The HTTP status code received from the DA layer
    /// * `spinner` - The progress spinner to update with error messages
    ///
    /// # Returns
    /// * `Result<(), Box<DaSubmitError>>` - Ok if the error was handled, Err otherwise
    fn handle_http_error(
        status_code: u16,
        spinner: &ProgressBar,
    ) -> Result<(), Box<DaSubmitError>> {
        match status_code {
            401 => {
                spinner.finish_with_message(
                    "❌ Assertion submission failed! Unauthorized. Please run `pcl auth login`.",
                );
                Ok(())
            }
            _ => Err(Box::new(DaSubmitError::HttpError(status_code))),
        }
    }

    /// Displays the assertion information and next steps after successful submission.
    ///
    /// # Arguments
    /// * `assertion` - The assertion that was successfully submitted
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

    /// Builds and flattens the assertion source code.
    ///
    /// # Returns
    /// * `Result<BuildAndFlatOutput, DaSubmitError>` - The build output or error
    async fn build_and_flatten_assertion(&self) -> Result<BuildAndFlatOutput, DaSubmitError> {
        self.args
            .run()
            .map_err(|e| DaSubmitError::PhoundryError(*e))
    }

    /// Creates a DA client with appropriate authentication.
    ///
    /// # Arguments
    /// * `config` - Configuration containing authentication details
    ///
    /// # Returns
    /// * `Result<DaClient, DaClientError>` - The configured client or error
    fn create_da_client(&self, config: &CliConfig) -> Result<DaClient, DaClientError> {
        match &config.auth {
            Some(auth) => DaClient::new_with_auth(&self.url, &auth.access_token),
            None => DaClient::new(&self.url),
        }
    }

    /// Submits the assertion to the DA layer.
    ///
    /// # Arguments
    /// * `client` - The DA client to use for submission
    /// * `build_output` - The build output containing flattened source
    /// * `spinner` - The progress spinner to update
    ///
    /// # Returns
    /// * `Result<(), DaSubmitError>` - Success or error
    async fn submit_to_da(
        &self,
        client: &DaClient,
        build_output: &BuildAndFlatOutput,
        spinner: &ProgressBar,
    ) -> Result<DaSubmissionResponse, DaSubmitError> {
        match client
            .submit_assertion(
                self.args.assertion_contract.clone(),
                build_output.flattened_source.clone(),
                build_output.compiler_version.clone(),
            )
            .await
        {
            Ok(res) => Ok(res),
            Err(err) => match err {
                DaClientError::ClientError(ClientError::Transport(ref boxed_err)) => {
                    if let Some(TransportError::Rejected { status_code }) = boxed_err.downcast_ref()
                    {
                        Self::handle_http_error(*status_code, spinner)?;
                        Err(err.into())
                    } else {
                        Err(err.into())
                    }
                }
                _ => Err(err.into()),
            },
        }
    }

    /// Updates the configuration with the submission result.
    ///
    /// # Arguments
    /// * `config` - The configuration to update
    /// * `spinner` - The progress spinner to update
    fn update_config<A: ToString, S: ToString>(
        &self,
        config: &mut CliConfig,
        assertion_id: A,
        signature: S,
        spinner: &ProgressBar,
    ) {
        let assertion_for_submission = AssertionForSubmission {
            assertion_contract: self.args.assertion_contract.to_string(),
            assertion_id: assertion_id.to_string(),
            signature: signature.to_string(),
        };

        config.add_assertion_for_submission(assertion_for_submission.clone());
        spinner.finish_with_message("✅ Assertion successfully submitted!");

        self.display_success_info(&assertion_for_submission);
    }

    /// Executes the assertion storage process.
    ///
    /// This method:
    /// 1. Builds and flattens the assertion
    /// 2. Creates a DA client with appropriate authentication
    /// 3. Submits the assertion to the DA layer
    /// 4. Updates the configuration with the submission result
    ///
    /// # Arguments
    /// * `_cli_args` - General CLI arguments (unused)
    /// * `config` - Configuration containing assertions and auth details
    ///
    /// # Returns
    /// * `Result<(), DaSubmitError>` - Success or specific error
    ///
    /// # Errors
    /// * Returns `DaSubmitError` if the build process fails
    /// * Returns `DaSubmitError` if the submission to DA layer fails
    /// * Returns `DaSubmitError` if there are authentication issues
    pub async fn run(
        &self,
        _cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DaSubmitError> {
        let spinner = Self::create_spinner();
        spinner.set_message("Submitting assertion to DA...");

        let build_output = self.build_and_flatten_assertion().await?;
        let client = self
            .create_da_client(config)
            .map_err(DaSubmitError::DaClientError)?;
        let submission_response = self.submit_to_da(&client, &build_output, &spinner).await?;
        self.update_config(
            config,
            submission_response.id,
            &submission_response.signature,
            &spinner,
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UserAuth;
    use alloy_primitives::Address;
    use chrono::DateTime;
    use mockito::Server;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Creates a test configuration with authentication
    fn create_test_config() -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: "test_token".to_string(),
                refresh_token: "test_refresh".to_string(),
                user_address: Address::from_slice(&[0; 20]),
                expires_at: DateTime::from_timestamp(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs() as i64,
                    0,
                )
                .unwrap(),
            }),
            ..Default::default()
        }
    }

    /// Creates test build and flatten arguments
    fn create_test_build_args() -> BuildAndFlattenArgs {
        BuildAndFlattenArgs {
            assertion_contract: "test_assertion".to_string(),
            // Add other required fields
            ..BuildAndFlattenArgs::default()
        }
    }

    #[tokio::test]
    async fn test_handle_http_error_unauthorized() {
        let spinner = DaStoreArgs::create_spinner();
        let result = DaStoreArgs::handle_http_error(401, &spinner);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_http_error_other() {
        let spinner = DaStoreArgs::create_spinner();
        let result = DaStoreArgs::handle_http_error(500, &spinner);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_display_success_info() {
        let args = DaStoreArgs {
            url: "http://localhost:5001".to_string(),
            args: create_test_build_args(),
        };

        let assertion = AssertionForSubmission {
            assertion_contract: "test_assertion".to_string(),
            assertion_id: "test_id".to_string(),
            signature: "test_signature".to_string(),
        };

        // This test just ensures the function doesn't panic
        args.display_success_info(&assertion);
    }

    #[tokio::test]
    async fn test_run_with_auth() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/submit_assertion")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id": "test_id", "signature": "test_signature"}"#)
            .create();

        let mut config = create_test_config();
        let args = DaStoreArgs {
            url: server.url(),
            args: create_test_build_args(),
        };

        let cli_args = CliArgs::default();
        let result = args.run(&cli_args, &mut config).await;
        assert!(result.is_ok());
        mock.assert();
    }

    #[tokio::test]
    async fn test_run_unauthorized() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/submit_assertion")
            .with_status(401)
            .create();

        let mut config = create_test_config();
        let args = DaStoreArgs {
            url: server.url(),
            args: create_test_build_args(),
        };

        let cli_args = CliArgs::default();
        let result = args.run(&cli_args, &mut config).await;
        assert!(result.is_err());
        mock.assert();
    }

    #[tokio::test]
    async fn test_run_server_error() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/submit_assertion")
            .with_status(500)
            .create();

        let mut config = create_test_config();
        let args = DaStoreArgs {
            url: server.url(),
            args: create_test_build_args(),
        };

        let cli_args = CliArgs::default();
        let result = args.run(&cli_args, &mut config).await;
        assert!(result.is_err());
        mock.assert();
    }

    #[tokio::test]
    async fn test_create_spinner() {
        let spinner = DaStoreArgs::create_spinner();
        assert_eq!(spinner.message(), "");
        spinner.set_message("test");
        assert_eq!(spinner.message(), "test");
    }

    #[tokio::test]
    async fn test_create_da_client_with_auth() {
        let args = DaStoreArgs {
            url: "http://localhost:5001".to_string(),
            args: BuildAndFlattenArgs::default(),
        };

        let config = CliConfig {
            auth: Some(UserAuth {
                access_token: "test_token".to_string(),
                refresh_token: "test_refresh".to_string(),
                user_address: Address::from_slice(&[0; 20]),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
            }),
            ..Default::default()
        };

        let client = args.create_da_client(&config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_create_da_client_without_auth() {
        let args = DaStoreArgs {
            url: "http://localhost:5001".to_string(),
            args: BuildAndFlattenArgs::default(),
        };

        let config = CliConfig::default();
        let client = args.create_da_client(&config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let args = DaStoreArgs {
            url: "http://localhost:5001".to_string(),
            args: BuildAndFlattenArgs {
                assertion_contract: "test_assertion".to_string(),
                ..BuildAndFlattenArgs::default()
            },
        };

        let mut config = CliConfig::default();
        let spinner = DaStoreArgs::create_spinner();

        args.update_config(&mut config, "test_id", "test_signature", &spinner);

        assert_eq!(config.assertions_for_submission.len(), 1);
        let assertion = config
            .assertions_for_submission
            .get("test_assertion")
            .unwrap();
        assert_eq!(assertion.assertion_contract, "test_assertion");
        assert_eq!(assertion.assertion_id, "test_id");
        assert_eq!(assertion.signature, "test_signature");
    }

    #[tokio::test]
    async fn test_run_with_invalid_url() {
        let args = DaStoreArgs {
            url: "invalid-url".to_string(),
            args: BuildAndFlattenArgs::default(),
        };

        let mut config = CliConfig::default();
        let cli_args = CliArgs::default();

        let result = args.run(&cli_args, &mut config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_with_expired_auth() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/submit_assertion")
            .with_status(401)
            .create();

        let args = DaStoreArgs {
            url: server.url(),
            args: BuildAndFlattenArgs::default(),
        };

        let cli_args = CliArgs::default();

        let mut config = CliConfig {
            auth: Some(UserAuth {
                access_token: "expired_token".to_string(),
                refresh_token: "expired_refresh".to_string(),
                user_address: Address::from_slice(&[0; 20]),
                expires_at: DateTime::from_timestamp(0, 0).unwrap(), // Expired token
            }),
            ..Default::default()
        };

        let result = args.run(&cli_args, &mut config).await;
        assert!(result.is_err());
        mock.assert();
    }
}
