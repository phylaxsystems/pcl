use clap::Parser;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use pcl_common::{
    args::CliArgs,
    utils::{compilation_target, compiler_version},
};
use pcl_phoundry::build::BuildArgs;
use tokio::time::Duration;

use assertion_da_client::DaClient;

use crate::{config::CliConfig, error::DaSubmitError};

#[derive(Parser)]
#[clap(
    name = "store",
    about = "Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer"
)]
pub struct DASubmitArgs {
    // FIXME (Odysseas): Replace localhost with the actual DA URL from our infrastructure
    /// URL of the assertion-DA
    #[clap(long, env = "PCL_DA_URL", default_value = "http://localhost:5001")]
    url: String,
    /// Name of the assertion contract to submit
    assertion: String,
}

impl DASubmitArgs {
    pub async fn run(
        &self,
        cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DaSubmitError> {
        let build_args = BuildArgs {
            assertions: vec![self.assertion.clone()],
        };

        let out_dir = cli_args.out_dir();
        let relative_path = compilation_target(&self.assertion, &out_dir);
        let mut full_path = cli_args.root_dir();
        full_path.push(relative_path);

        let _result = build_args.run(cli_args)?;

        let flatten_contract = build_args.get_flattened_source(&full_path, cli_args)?;
        let compiler_version = compiler_version(&self.assertion, &out_dir)
            .split('+')
            .next()
            .unwrap_or_default()
            .to_string();

        // Create a spinner to show progress while submitting
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
        let result = DaClient::new(&self.url)?
            .submit_assertion(self.assertion.clone(), flatten_contract, compiler_version)
            .await?;

        config.add_assertion_for_submission(
            self.assertion.clone(),
            result.id.to_string(),
            result.signature.to_string()
        );
        // Finish spinner with success message
        spinner.finish_with_message("✅ Assertion successfully submitted!");

        // Display formatted assertion information
        println!("\n\n{}", "Assertion Information".bold().green());
        println!("{}", "===================".green());
        println!("{}", config.assertions_for_submission.last().unwrap());
        
        // Display next steps with highlighted command
        println!("\n{}", "Next Steps:".bold());
        println!("Submit this assertion to a project with:");
        println!("  {} submit -a {} -p <project_name>", "pcl".cyan().bold(), self.assertion.cyan());
        println!("Visit the Credible Layer DApp to link the assertion on-chain and enforce it:");
        println!("  {}", "https://dapp.credible.layer".cyan().bold());
        Ok(())
    }
}
