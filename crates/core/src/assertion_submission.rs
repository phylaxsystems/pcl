use crate::{
    config::{AssertionForSubmission, CliConfig},
    error::DappSubmitError,
};
use inquire::{MultiSelect, Select};
use pcl_common::args::CliArgs;
use serde::Deserialize;
use serde_json::json;

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

/// Arguments for submitting assertions to the Credible Layer dApp
///
/// This struct handles CLI arguments for the assertion submission process,
/// including the dApp URL, project name, and assertion names.
#[derive(clap::Parser)]
#[clap(about = "Submit assertions to the Credible Layer dApp")]
pub struct DappSubmitArgs {
    /// Base URL for the Credible Layer dApp API
    #[clap(
        short,
        long,
        default_value = "https://credible-layer-dapp.pages.dev/api/v1"
    )]
    dapp_url: String,

    /// Optional project name to skip interactive selection
    #[clap(short, long)]
    project_name: Option<String>,

    /// Optional list of assertion names to skip interactive selection
    #[clap(short, long)]
    assertion_name: Option<Vec<String>>,
}

impl DappSubmitArgs {
    /// Executes the assertion submission workflow
    ///
    /// # Arguments
    /// * `_cli_args` - General CLI arguments
    /// * `config` - Configuration containing assertions and auth details
    ///
    /// # Returns
    /// * `Result<(), DappSubmitError>` - Success or specific error
    pub async fn run(
        &self,
        _cli_args: CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DappSubmitError> {
        let projects = self.get_projects(config).await?;
        let assertions_for_submission = config
            .assertions_for_submission
            .iter()
            .map(|a| a.assertion_contract.clone())
            .collect();

        let project_name = self.provide_or_select(
            self.project_name.clone(),
            projects.iter().map(|p| p.project_name.clone()).collect(),
            "Select a project to submit the assertion to:".to_string(),
        )?;
        let project = projects
            .iter()
            .find(|p| p.project_name == project_name)
            .unwrap();

        let assertion_names = self.provide_or_multi_select(
            self.assertion_name.clone(),
            assertions_for_submission,
            "Select an assertion to submit:".to_string(),
        )?;

        let assertions: Vec<&AssertionForSubmission> = assertion_names
            .iter()
            .map(|n| {
                config
                    .assertions_for_submission
                    .iter()
                    .find(|a| a.assertion_contract == *n)
                    .unwrap()
            })
            .collect();

        self.submit_assertion(project, &assertions).await?;
        // TOOD: remove assertion from config

        Ok(())
    }

    /// Submits selected assertions to the specified project
    ///
    /// # Arguments
    /// * `project` - Target project for submission
    /// * `assertions` - List of assertions to submit
    ///
    /// # Returns
    /// * `Result<(), DappSubmitError>` - Success or API error
    async fn submit_assertion(
        &self,
        project: &Project,
        assertions: &[&AssertionForSubmission],
    ) -> Result<(), DappSubmitError> {
        let client = reqwest::Client::new();
        // TODO: Update payload structure once API spec is finalized
        let body = json!({
            "project_id": project._project_id,
            "assertions": assertions.iter().map(|a| &a.assertion_contract).collect::<Vec<_>>()
        });

        let response = client
            .post(format!("{}/assertions", self.dapp_url))
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(DappSubmitError::SubmissionFailed(response.text().await?))
        }
    }

    /// Handles interactive or direct selection of a single value
    ///
    /// # Arguments
    /// * `maybe_key` - Optional pre-selected value
    /// * `values` - Available options
    /// * `message` - Prompt message for interactive selection
    ///
    /// # Returns
    /// * `Result<String, DappSubmitError>` - Selected value or error
    fn provide_or_select(
        &self,
        maybe_key: Option<String>,
        values: Vec<String>,
        message: String,
    ) -> Result<String, DappSubmitError> {
        match maybe_key {
            None => Select::new(message.as_str(), values)
                .prompt()
                .map_err(|_| DappSubmitError::ProjectSelectionCancelled),
            Some(key) => {
                let exists = values
                    .iter()
                    .any(|p| key.to_lowercase() == p.to_lowercase());
                if exists {
                    Ok(key.to_string())
                } else {
                    println!("{} does not exist", key);
                    let choice = Select::new(message.as_str(), values)
                        .prompt()
                        .map_err(|_| DappSubmitError::ProjectSelectionCancelled)?;
                    Ok(choice)
                }
            }
        }
    }

    /// Handles interactive or direct selection of multiple values
    ///
    /// # Arguments
    /// * `maybe_keys` - Optional pre-selected values
    /// * `values` - Available options
    /// * `message` - Prompt message for interactive selection
    ///
    /// # Returns
    /// * `Result<Vec<String>, DappSubmitError>` - Selected values or error
    fn provide_or_multi_select(
        &self,
        maybe_keys: Option<Vec<String>>,
        values: Vec<String>,
        message: String,
    ) -> Result<Vec<String>, DappSubmitError> {
        match maybe_keys {
            None => MultiSelect::new(message.as_str(), values)
                .prompt()
                .map_err(|_| DappSubmitError::ProjectSelectionCancelled),
            Some(key) => {
                let exists = key
                    .iter()
                    .all(|k| values.iter().any(|v| k.to_lowercase() == v.to_lowercase()));
                if exists {
                    Ok(values)
                } else {
                    println!("{} does not exist", key.join(", "));
                    let choice = MultiSelect::new(message.as_str(), values)
                        .prompt()
                        .map_err(|_| DappSubmitError::ProjectSelectionCancelled)?;
                    Ok(choice)
                }
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

/// TODO(ODYSSEAS): Add tests for the DappSubmitArgs struct
#[cfg(test)]
mod tests {
    use crate::assertion_submission::DappSubmitArgs;

    #[test]
    fn test_provide_or_select_with_valid_input() {
        let args = DappSubmitArgs {
            dapp_url: "".to_string(),
            project_name: Some("Project1".to_string()),
            assertion_name: None,
        };

        let values = vec!["Project1".to_string(), "Project2".to_string()];
        let result =
            args.provide_or_select(Some("Project1".to_string()), values, "Select:".to_string());

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Project1");
    }
}
