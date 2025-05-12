use crate::{
    config::{AssertionForSubmission, AssertionKey, CliConfig},
    error::DappSubmitError,
};
use clap::ValueHint;
use inquire::{MultiSelect, Select};
use pcl_common::args::CliArgs;
use serde::Deserialize;
use serde_json::json;

// TODO(Odysseas) Add tests for the Dapp submission + Rust bindings from the Dapp API

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct Project {
    project_id: String,
    project_name: String,
    project_description: Option<String>,
    profile_image_url: Option<String>,
    project_networks: Vec<String>,
    project_manager: String,
    created_at: String,
    updated_at: String,
}

/// Arguments for submitting assertions to the Credible Layer dApp
///
/// This struct handles CLI arguments for the assertion submission process,
/// including the dApp URL, project name, and assertion names.
#[derive(clap::Parser)]
#[clap(
    name = "submit",
    about = "Submit assertions to the Credible Layer dApp"
)]
pub struct DappSubmitArgs {
    /// Base URL for the Credible Layer dApp API
    #[clap(
        short = 'u',
        long,
        value_hint = ValueHint::Url,
        value_name = "API Endpoint",
        default_value = "https://dapp.phylax.systems/api/v1"
    )]
    dapp_url: String,

    /// Optional project name to skip interactive selection
    #[clap(
        short = 'p',
        long,
        value_name = "PROJECT",
        value_hint = ValueHint::Other,

    )]
    project_name: Option<String>,

    /// Optional list of assertion name and constructor args to skip interactive selection
    /// Format: assertion_name OR 'assertion_name(constructor_arg0,constructor_arg1)'
    #[clap(
        long,
        short = 'a',
        value_name = "ASSERTION_KEYS",
        value_hint = ValueHint::Other,
    )]
    assertion_keys: Option<Vec<AssertionKey>>,
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
        _cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DappSubmitError> {
        let projects = self.get_projects(config).await?;

        let assertion_keys_for_submission = config
            .assertions_for_submission
            .keys()
            .map(|k| k.to_string())
            .collect();

        let project_name = self.provide_or_select(
            self.project_name.clone(),
            projects.iter().map(|p| p.project_name.clone()).collect(),
            "Select a project to submit the assertion to:".to_string(),
        )?;
        let project = projects
            .iter()
            .find(|p| p.project_name == project_name)
            .unwrap(); // Safe to unwrap since it should be selected from the list

        let assertion_keys = self.provide_or_multi_select(
            self.assertion_keys
                .clone()
                .map(|keys| keys.iter().map(|k| k.to_string()).collect()),
            assertion_keys_for_submission,
            "Select an assertion to submit:".to_string(),
        )?;
        let mut assertions = vec![];
        for key in assertion_keys {
            let assertion = config
                .assertions_for_submission
                .remove(&key.clone().into())
                .ok_or(DappSubmitError::CouldNotFindStoredAssertion(key.clone()))?;

            assertions.push(assertion);
        }

        self.submit_assertion(project, &assertions, config).await?;

        println!(
            "Successfully submitted {} assertion{} to project {}",
            assertions.len(),
            if assertions.len() > 1 { "s" } else { "" },
            project.project_name
        );

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
        assertions: &[AssertionForSubmission],
        config: &CliConfig,
    ) -> Result<(), DappSubmitError> {
        let client = reqwest::Client::new();
        let body = json!({
            "assertions": assertions.iter().map(|a| json!({
                "contract_name": &a.assertion_contract,
                "assertion_id": &a.assertion_id,
                "signature": &a.signature
            })).collect::<Vec<_>>()
        });

        let response = client
            .post(format!(
                "{}/projects/{}/submitted-assertions",
                self.dapp_url, project.project_id
            ))
            .header(
                "Authorization",
                format!("Bearer {}", config.auth.as_ref().unwrap().access_token),
            )
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if response.status().is_success() {
            Ok(())
        } else {
            // If the response is unauthorized, return a specific error
            if response.status().as_u16() == 401 {
                return Err(DappSubmitError::NoAuthToken);
            }
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
                    println!("{key} does not exist");
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
                config
                    .auth
                    .as_ref()
                    .ok_or(DappSubmitError::NoAuthToken)?
                    .user_address
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
            assertion_keys: None,
        };

        let values = vec!["Project1".to_string(), "Project2".to_string()];
        let result =
            args.provide_or_select(Some("Project1".to_string()), values, "Select:".to_string());

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Project1");
    }
}
