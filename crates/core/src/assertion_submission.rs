use pcl_common::args::CliArgs;
use serde::{Deserialize, Serialize};
use inquire::Select;
use crate::{config::CliConfig, error::DappSubmitError};

#[derive(Deserialize)]
struct Project {
    id: String,
    name: String,
}

#[derive(clap::Parser)]
pub struct AssertionSubmitArgs {
    #[clap(short, long, default_value = "https://credible-layer-dapp.pages.dev/api/v1")]
    dapp_url: String,
}


impl AssertionSubmitArgs {
    pub async fn run(&self, cli_args: CliArgs, config: CliConfig) -> Result<(), DappSubmitError> {
        let client = reqwest::Client::new();
        let projects: Vec<Project> = client
            .get(format!("{}/projects?user={}", self.dapp_url, config.auth.unwrap().user_address))
            .send()
            .await?
            .json()
            .await?;

        // Create selection options
        let project_options: Vec<String> = projects
            .iter()
            .map(|p| format!("{} ({})", p.name, p.id))
            .collect();

        // Show interactive selection
        let selection = Select::new(
            "Select a project to submit the assertion to:",
            project_options,
        )
        .prompt()
        .map_err(|_| DappSubmitError::ProjectSelectionCancelled)?;
        Ok(())
    }
}
