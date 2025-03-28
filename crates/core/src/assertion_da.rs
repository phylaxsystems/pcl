use clap::Parser;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use pcl_common::{args::CliArgs, utils::get_build_info, Assertion};
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
    #[clap(value_parser = parse_assertion)]
    assertion: Assertion,
}

fn parse_assertion(s: &str) -> Result<Assertion, String> {
    let parts = s.split(':').collect::<Vec<&str>>();

    if parts.len() == 1 {
        return Ok(Assertion::new(None, parts[0].to_string()));
    }

    if parts.len() == 2 {
        return Ok(Assertion::new(
            Some(parts[0].to_string()),
            parts[1].to_string(),
        ));
    }

    Err(
        "Assertion must be in the format <file_name>:<contract_name> or <contract_name>"
            .to_string(),
    )
}

impl DASubmitArgs {
    pub async fn run(
        &self,
        cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DaSubmitError> {
        let build_args = BuildArgs {
            assertions: vec![self.assertion.contract_name().clone()],
        };

        let _result = build_args.run(cli_args)?;

        let out_dir = cli_args.out_dir();
        let build_info = get_build_info(&self.assertion, &out_dir);
        let mut full_path = cli_args.root_dir();
        full_path.push(build_info.compilation_target);

        let flatten_contract = build_args.get_flattened_source(&full_path, cli_args)?;
        let compiler_version = build_info
            .compiler_version
            .split('+')
            .next()
            .unwrap_or_default()
            .to_string();

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
        let result = DaClient::new(&self.url)?
            .submit_assertion(
                self.assertion.contract_name().to_string(),
                flatten_contract,
                compiler_version,
            )
            .await?;

        config.add_assertion_for_submission(
            self.assertion.contract_name().to_string(),
            result.id.to_string(),
            result.signature.to_string(),
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
        println!(
            "  {} submit -a {} -p <project_name>",
            "pcl".cyan().bold(),
            self.assertion.contract_name().cyan()
        );
        println!("Visit the Credible Layer DApp to link the assertion on-chain and enforce it:");
        println!("  {}", "https://dapp.credible.layer".cyan().bold());
        Ok(())
    }
}
