use crate::{
    config::{AssertionsForSubmission, CliConfig, Assertion},
    error::DappSubmitError,
};
use inquire::{MultiSelect, Select};
use pcl_common::args::CliArgs;
use serde_json::Value as JsonValue;
use alloy_primitives::{Bytes, B256};

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
        short,
        long,
        default_value = "https://credible-layer-dapp.pages.dev/api/v1"
    )]
    dapp_url: String,

    /// Optional project name to skip interactive selection
    #[clap(short, long)]
    project_name: Option<String>,

    /// Optional list of assertion contract names to skip interactive selection
    #[clap(short, long)]
    contract_names: Option<Vec<String>>,
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

        if projects.is_empty() {
            return Err(DappSubmitError::NoProjectsFound);
        }

        let project_names = projects
            .iter()
            .map(|p| {
                let project_name = p
                    .get("project_name")
                    .expect("Project object did not contain 'project_name'");
                project_name.as_str().expect("Project name not a string").to_owned()
            })
            .collect();

        let project_name = self.provide_or_select(
            self.project_name.clone(),
            project_names,
            "Select a project to submit the assertion to:".to_string(),
        )?;

        if config.assertions_for_submission.is_empty() {
            return Err(DappSubmitError::NoAssertionsInConfig);
        }

        let selected_assertion_names = self.provide_or_multi_select(
            self.contract_names.clone(),
            config.assertions_for_submission.names(),
            "Select an assertion contract to submit:".to_string(),
        )?;

        let selected_assertions = selected_assertion_names.iter().fold(
            AssertionsForSubmission::default(),
            |mut ass, n| {
                let assertion = config
                    .assertions_for_submission
                    .assertions
                    .iter()
                    .find(|a| a.contract_name == *n)
                    .expect(&format!("Assertion Contract: {n} not found"));
                ass.assertions.push(assertion.clone());
                ass
            },
        );

        let project = projects
            .iter()
            .find(|p| {
                *p.get("project_name")
                    .expect("Project object did not contain 'project_name'")
                    == *project_name
            })
            .expect("Selected project name not found.");

        // Submit selected assertions to dapp
        self.submit_assertion(project, selected_assertions.clone())
            .await?;

        // Remove assertions by retaining assertions not in the selected list
        config.assertions_for_submission.assertions.retain(|a| {
            selected_assertions
                .assertions
                .iter()
                .find(|b| a.contract_name == b.contract_name)
                .is_none()
        });

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
        project: &JsonValue,
        assertions: AssertionsForSubmission,
    ) -> Result<(), DappSubmitError> {
        let client = reqwest::Client::new();

        let project_id = project
            .get("project_id")
            .expect("Project object did not contain 'project_id'");

        let response = client
            .post(format!(
                "{}/{}/submitted_assertions",
                project_id, self.dapp_url
            ))
            .json(&assertions)
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
            None => Select::new(message.as_str(), values).prompt().map_err(|e| {
                println!("{:?}", e);

                DappSubmitError::ProjectSelectionCancelled
            }),
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
                        .map_err(|e| {
                            println!("{:?}", e);
                            DappSubmitError::ProjectSelectionCancelled
                        })?;
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
                .map_err(|e| {
                    println!("{:?}", e);

                    DappSubmitError::ProjectSelectionCancelled
                }),
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
    async fn get_projects(
        &self,
        config: &mut CliConfig,
    ) -> Result<Vec<JsonValue>, DappSubmitError> {
        let client = reqwest::Client::new();

        let projects = client
            .get(format!(
                "{}/projects?user={}",
                self.dapp_url,
                config.auth.as_ref().expect("Not currently authorized with dApp.").user_address
            ))
            .send()
            .await?
            .json::<JsonValue>()
            .await?;

        Ok(projects
            .as_array()
            .ok_or(DappSubmitError::NoProjectsFound)?
            .to_vec())
    }
}

/// TODO(ODYSSEAS): Add tests for the DappSubmitArgs struct
#[cfg(test)]
mod tests {
    use crate::{assertion_submission::DappSubmitArgs, config::UserAuth};
    use super::*;
    use tempfile::TempDir;
    use alloy_primitives::fixed_bytes;


    #[tokio::test]
    async fn test_file_store() {
        let temp_dir = TempDir::new().unwrap().into_path();

        let mut config = CliConfig {
            assertions_for_submission: AssertionsForSubmission {
                assertions: vec![
                    Assertion {
                        contract_name: "Assertion1".to_string(),
                        assertion_id: B256::ZERO,
                        signature: Bytes::from(vec![1, 2, 3]),
                    },
                    Assertion {
                        contract_name: "Assertion2".to_string(),
                        assertion_id: alloy_primitives::FixedBytes::from_slice(&[1; 32]).into(),
                        signature: Bytes::from(vec![4, 5, 6]),
                    },
                ],
            },
            auth: Some(UserAuth::default()),
        };

        let args = DappSubmitArgs {
            dapp_url: "".to_string(),
            project_name: Some("Project1".to_string()),
            contract_names: None,
        };

        args.run(CliArgs::default(), &mut config).await.unwrap();

    }

    #[test]
    fn test_provide_or_select_with_valid_input() {
        let args = DappSubmitArgs {
            dapp_url: "".to_string(),
            project_name: Some("Project1".to_string()),
            contract_names: None,
        };

        let values = vec!["Project1".to_string(), "Project2".to_string()];
        let result =
            args.provide_or_select(Some("Project1".to_string()), values, "Select:".to_string());

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Project1");
    }

}
