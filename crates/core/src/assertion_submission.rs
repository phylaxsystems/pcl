use crate::{config::CliConfig, error::DappSubmitError};
use inquire::Select;
use pcl_common::args::CliArgs;
use serde::Deserialize;

#[derive(Deserialize)]
struct Project {
    _project_id: String,
    project_name: String,
    _project_description: Option<String>,
    _profile_image_url: Option<String>,
    _project_networks: Vec<String>,
    _project_manager: String,
    _assertion_adopters: Vec<String>,
    _created_at: String,
    _updated_at: String,
}

#[derive(clap::Parser)]
pub struct DappSubmitArgs {
    #[clap(
        short,
        long,
        default_value = "https://credible-layer-dapp.pages.dev/api/v1"
    )]
    dapp_url: String,
}

impl DappSubmitArgs {
    pub async fn run(
        &self,
        cli_args: CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DappSubmitError> {
        let client = reqwest::Client::new();
        let projects: Vec<Project> = client
            .get(format!(
                "{}/projects?user={}",
                self.dapp_url, "0x702352bc4fc5a3C1e7ef8D96C6d51d5352998c2B"
            ))
            .send()
            .await?
            .json()
            .await?;

        // Create selection options
        let project_options: Vec<String> =
            projects.iter().map(|p| p.project_name.clone()).collect();

        // Show interactive selection
        let selection = Select::new(
            "Select a project to submit the assertion to:",
            project_options,
        )
        .prompt()
        .map_err(|_| DappSubmitError::ProjectSelectionCancelled)?;

        let project = projects
            .iter()
            .find(|p| p.project_name == selection)
            .unwrap();

        Ok(())
    }
}
