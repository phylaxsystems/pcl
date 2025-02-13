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
    #[clap(short, long)]
    project_name: Option<String>,
    #[clap(short, long)]
    assertion_name: Option<String>,
}

impl DappSubmitArgs {
    pub async fn run(
        &self,
        _cli_args: CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DappSubmitError> {
        let projects = self.get_projects(config).await?;
        let _project = self.select_project(projects)?;

        Ok(())
    }

    fn _select_assertion(&self) {
        todo!()
    }

    fn _submit_assertion(&self) {}

    fn select_project(&self, projects: Vec<Project>) -> Result<String, DappSubmitError> {
        match &self.project_name {
            None => {
                let project_options: Vec<String> =
                    projects.iter().map(|p| p.project_name.clone()).collect();

                Select::new(
                    "Select a project to submit the assertion to:",
                    project_options,
                )
                .prompt()
                .map_err(|_| DappSubmitError::ProjectSelectionCancelled)
            }
            Some(name) => {
                let exists = projects
                    .iter()
                    .any(|p| p.project_name.to_lowercase() == name.to_lowercase());

                if !exists {
                    println!(
                        "The project {} does not exist. Please create it or choose from the list of existing projects.",
                        name
                    );
                }
                Ok(name.clone())
            }
        }
    }
    async fn get_projects(&self, config: &mut CliConfig) -> Result<Vec<Project>, DappSubmitError> {
        let client = reqwest::Client::new();
        let projects: Vec<Project> = client
            .get(format!(
                "{}/projects?user={}",
                self.dapp_url,
                config.auth.as_ref().unwrap().user_address
            ))
            .send()
            .await?
            .json()
            .await?;
        Ok(projects)
    }
}
