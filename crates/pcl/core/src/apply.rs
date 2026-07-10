#[cfg(feature = "credible")]
use crate::verify::{
    VerificationSummary,
    build_deployment_bytecode,
    format_display_name,
    print_verification_summary,
    run_verification,
};
use crate::{
    DEFAULT_PLATFORM_URL,
    abi,
    api::generated_operation_path,
    client::{
        ClientBuildError,
        authenticated_client,
        ensure_fresh_auth,
    },
    config::CliConfig,
    credible_config::{
        CredibleToml,
        assertion_contract_name,
    },
    diff::PreviewResponse,
    error::ApplyError,
    output::{
        OutputStream,
        ok_envelope,
        print_envelope,
        shell_word,
    },
};
use alloy_primitives::Bytes;
use clap::ValueHint;
use dapp_api_client::generated::client::{
    Client as GeneratedClient,
    Error as GeneratedError,
    types::{
        GetProjectsResponseItem,
        PostProjectsProjectIdReleasesBody,
        PostProjectsProjectIdReleasesBodyContractsValue,
        PostProjectsProjectIdReleasesBodyContractsValueAssertionsItem,
        PostProjectsProjectIdReleasesPreviewBody,
        PostProjectsProjectIdReleasesResponse,
    },
};
use inquire::Select;
use pcl_common::args::{
    CliArgs,
    OutputMode,
};
use pcl_phoundry::{
    DEFAULT_ASSERTION_CONTRACTS_DIR,
    build_and_flatten::BuildAndFlattenArgs,
};
use serde_json::{
    Value,
    json,
};
use std::{
    collections::HashMap,
    io::{
        Write,
        stderr,
        stdin,
    },
    path::{
        Path,
        PathBuf,
    },
};
use url::Url;
use uuid::Uuid;

#[derive(clap::Parser, Debug)]
#[command(
    name = "apply",
    about = "Preview and apply declarative deployment changes from credible.toml"
)]
pub struct ApplyArgs {
    #[arg(
        long,
        value_hint = ValueHint::DirPath,
        default_value = ".",
        help = "Project root directory"
    )]
    pub root: PathBuf,

    #[arg(
        short = 'c',
        long = "config",
        value_hint = ValueHint::FilePath,
        default_value = "assertions/credible.toml",
        help = "Path to credible.toml, relative to root or absolute"
    )]
    pub config: PathBuf,

    #[arg(
        long = "yes",
        visible_alias = "auto-approve",
        help = "Apply without interactive confirmation"
    )]
    pub yes: bool,

    #[arg(
        long,
        help = "Build and verify the release payload without calling the API"
    )]
    pub dry_run: bool,

    #[arg(
        short = 'u',
        long = "api-url",
        env = "PCL_API_URL",
        value_hint = ValueHint::Url,
        help = "Base URL for the platform API. Defaults to the current login URL, then production"
    )]
    pub api_url: Option<url::Url>,
}

impl ApplyArgs {
    #[allow(clippy::too_many_lines)]
    pub async fn run(&self, cli_args: &CliArgs, config: &mut CliConfig) -> Result<(), ApplyError> {
        let output_mode = cli_args.output_mode();
        let api_url = config.resolve_platform_url(self.api_url.as_ref());
        let root = canonicalize_root(&self.root)?;
        let config_path = root.join(&self.config);
        let credible = CredibleToml::from_path(&config_path)?;
        let project_id = match credible.project_id {
            Some(project_id) => project_id,
            None if output_mode != OutputMode::Human => {
                return Err(ApplyError::InvalidConfig(
                    "`project_id` is required in credible.toml when using machine output"
                        .to_string(),
                ));
            }
            None => {
                Self::ensure_fresh_auth(config, cli_args, &api_url).await?;
                self.select_project(config, &api_url).await?
            }
        };
        let (payload, verification_inputs) = Self::build_payload(&credible, &root)?;
        #[cfg(feature = "credible")]
        let verification = Self::verify_all_assertions(&verification_inputs, output_mode)?;
        #[cfg(not(feature = "credible"))]
        let _ = verification_inputs;

        if self.dry_run {
            #[cfg(feature = "credible")]
            self.print_dry_run_output(output_mode, &root, project_id, &payload, &verification)?;
            #[cfg(not(feature = "credible"))]
            self.print_dry_run_output(output_mode, &root, project_id, &payload)?;
            return Ok(());
        }

        Self::ensure_fresh_auth(config, cli_args, &api_url).await?;
        let client = Self::build_client(config, &api_url)?;
        let preview = Self::call_preview(&client, &project_id, &payload).await?;

        if !preview.has_changes() {
            if output_mode == OutputMode::Human {
                println!("{}", crate::diff::NO_CHANGES_MESSAGE);
            } else {
                let envelope = ok_envelope(
                    apply_data(
                        "no_changes",
                        project_id,
                        #[cfg(feature = "credible")]
                        Some(&verification),
                        None,
                        Some(&preview),
                        false,
                        None,
                    ),
                    vec!["pcl projects mine".to_string()],
                );
                print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
            }
            return Ok(());
        }

        if output_mode == OutputMode::Human {
            print!("{}", preview.render_plan());
        }

        if !self.yes {
            if output_mode != OutputMode::Human {
                return Err(ApplyError::JsonConfirmationRequiresYes);
            }
            if !confirm_apply()? {
                return Err(ApplyError::ApplyCancelled);
            }
        }

        let release = Self::call_create_release(&client, &project_id, &payload).await?;

        if output_mode != OutputMode::Human {
            let envelope = ok_envelope(
                apply_data(
                    "applied",
                    project_id,
                    #[cfg(feature = "credible")]
                    Some(&verification),
                    None,
                    Some(&preview),
                    true,
                    Some(&release),
                ),
                vec![
                    format!("pcl releases list {project_id}"),
                    format!("pcl projects show {project_id}"),
                ],
            );
            print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
            return Ok(());
        }

        Self::print_release_success(api_url.as_str(), &project_id, &release);
        Ok(())
    }

    fn apply_command(&self, root: &Path, yes: bool, dry_run: bool) -> String {
        let mut parts = vec![
            "pcl".to_string(),
            "apply".to_string(),
            "--root".to_string(),
            shell_word(root.display().to_string()),
            "--config".to_string(),
            shell_word(self.config.display().to_string()),
        ];
        if yes {
            parts.push("--yes".to_string());
        }
        if dry_run {
            parts.push("--dry-run".to_string());
        }
        if let Some(api_url) = &self.api_url
            && api_url.as_str().trim_end_matches('/') != DEFAULT_PLATFORM_URL
        {
            parts.push("--api-url".to_string());
            parts.push(shell_word(api_url.as_str()));
        }
        parts.join(" ")
    }

    fn build_client(config: &CliConfig, api_url: &Url) -> Result<GeneratedClient, ApplyError> {
        authenticated_client(config, api_url).map_err(client_error_to_apply)
    }

    async fn ensure_fresh_auth(
        config: &mut CliConfig,
        cli_args: &CliArgs,
        api_url: &Url,
    ) -> Result<(), ApplyError> {
        ensure_fresh_auth(config, api_url, cli_args)
            .await
            .map_err(client_error_to_apply)
    }

    #[cfg(feature = "credible")]
    fn print_dry_run_output(
        &self,
        output_mode: OutputMode,
        root: &Path,
        project_id: Uuid,
        payload: &PostProjectsProjectIdReleasesBody,
        verification: &VerificationSummary,
    ) -> Result<(), ApplyError> {
        if output_mode == OutputMode::Human {
            println!(
                "Dry run complete. Built and verified release payload for project {project_id}."
            );
        } else {
            let envelope = ok_envelope(
                apply_data(
                    "dry_run",
                    project_id,
                    Some(verification),
                    Some(payload),
                    None,
                    false,
                    None,
                ),
                vec![self.apply_command(root, true, false)],
            );
            print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
        }
        Ok(())
    }

    #[cfg(not(feature = "credible"))]
    fn print_dry_run_output(
        &self,
        output_mode: OutputMode,
        root: &Path,
        project_id: Uuid,
        payload: &PostProjectsProjectIdReleasesBody,
    ) -> Result<(), ApplyError> {
        if output_mode == OutputMode::Human {
            println!("Dry run complete. Built release payload for project {project_id}.");
        } else {
            let envelope = ok_envelope(
                apply_data("dry_run", project_id, Some(payload), None, false, None),
                vec![self.apply_command(root, true, false)],
            );
            print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
        }
        Ok(())
    }

    async fn call_preview(
        client: &GeneratedClient,
        project_id: &Uuid,
        payload: &PostProjectsProjectIdReleasesBody,
    ) -> Result<PreviewResponse, ApplyError> {
        let project_id_string = project_id.to_string();
        let endpoint = generated_operation_path(
            "post_projects_project_id_releases_preview",
            &[("project_id", project_id_string.as_str())],
        )
        .unwrap_or_else(|| "post_projects_project_id_releases_preview".to_string());
        let body: PostProjectsProjectIdReleasesPreviewBody =
            serde_json::from_value(serde_json::to_value(payload)?)?;
        let response = match client
            .post_projects_project_id_releases_preview(project_id, None, &body)
            .await
        {
            Ok(response) => response.into_inner(),
            Err(error) => {
                return Err(generated_apply_api_error(endpoint.clone(), error).await);
            }
        };

        serde_json::from_value(serde_json::to_value(response)?).map_err(|e| {
            ApplyError::Api {
                endpoint,
                status: None,
                body: format!("Failed to parse preview response: {e}"),
            }
        })
    }

    async fn call_create_release(
        client: &GeneratedClient,
        project_id: &Uuid,
        payload: &PostProjectsProjectIdReleasesBody,
    ) -> Result<PostProjectsProjectIdReleasesResponse, ApplyError> {
        let project_id_string = project_id.to_string();
        let endpoint = generated_operation_path(
            "post_projects_project_id_releases",
            &[("project_id", project_id_string.as_str())],
        )
        .unwrap_or_else(|| "post_projects_project_id_releases".to_string());
        match client
            .post_projects_project_id_releases(project_id, None, payload)
            .await
        {
            Ok(response) => Ok(response.into_inner()),
            Err(error) => Err(generated_apply_api_error(endpoint, error).await),
        }
    }

    fn build_payload(
        credible: &CredibleToml,
        root: &Path,
    ) -> Result<(PostProjectsProjectIdReleasesBody, Vec<(String, Bytes)>), ApplyError> {
        let mut built_assertions = HashMap::new();
        let mut payload_contracts = HashMap::new();
        #[allow(unused_mut)]
        let mut verification_inputs: Vec<(String, Bytes)> = Vec::new();

        for (contract_key, contract) in &credible.contracts {
            let mut assertions = Vec::with_capacity(contract.assertions.len());

            for assertion in &contract.assertions {
                let build_key = assertion.file.clone();
                if !built_assertions.contains_key(&build_key) {
                    let output = BuildAndFlattenArgs {
                        root: Some(root.to_path_buf()),
                        assertion_contract: assertion_contract_name(&assertion.file)?,
                        contracts: assertion_contracts_dir(&assertion.file),
                    }
                    .run()
                    .map_err(ApplyError::BuildFailed)?;
                    built_assertions.insert(build_key.clone(), output);
                }

                let built = built_assertions.get(&build_key).ok_or_else(|| {
                    ApplyError::InvalidConfig(format!(
                        "Missing build output for assertion file {}",
                        assertion.file
                    ))
                })?;

                let contract_name = assertion_contract_name(&assertion.file)?;

                #[cfg(feature = "credible")]
                {
                    let deployment_bytecode =
                        build_deployment_bytecode(&built.bytecode, &built.abi, &assertion.args)
                            .map_err(|e| ApplyError::InvalidConfig(e.to_string()))?;
                    let display_name = format_display_name(&contract_name, &assertion.args);
                    verification_inputs.push((display_name, deployment_bytecode));
                }

                assertions.push(build_assertion_item(assertion, built, &contract_name)?);
            }

            let contract_value = build_contract_value(contract, assertions)?;
            payload_contracts.insert(contract_key.clone(), contract_value);
        }

        let environment = parse_field(&credible.environment, "environment")?;
        let assertions_dir = parse_field("assertions", "assertions dir")?;

        Ok((
            PostProjectsProjectIdReleasesBody {
                environment,
                assertions_dir,
                contracts: payload_contracts,
                compiler_args: vec![],
            },
            verification_inputs,
        ))
    }

    #[cfg(feature = "credible")]
    fn verify_all_assertions(
        inputs: &[(String, Bytes)],
        output_mode: OutputMode,
    ) -> Result<VerificationSummary, ApplyError> {
        let refs: Vec<(&str, Bytes)> = inputs
            .iter()
            .map(|(name, bytecode)| (name.as_str(), bytecode.clone()))
            .collect();

        let summary = run_verification(&refs);

        if output_mode == OutputMode::Human {
            println!("pcl apply \u{2014} Verifying assertions...\n");
            print_verification_summary(&summary);
        }

        if summary.failed > 0 {
            return Err(ApplyError::AssertionsFailed(Box::new(summary)));
        }

        Ok(summary)
    }

    async fn select_project(&self, config: &CliConfig, api_url: &Url) -> Result<Uuid, ApplyError> {
        let auth = config.auth.as_ref().ok_or(ApplyError::NoAuthToken)?;
        let user_id = auth.user_id.as_ref().ok_or_else(|| {
            ApplyError::InvalidConfig(
                "Missing user_id in auth config. Please run `pcl auth logout` then `pcl auth login` to refresh."
                    .to_string(),
            )
        })?;

        let client = Self::build_client(config, api_url)?;
        let projects: Vec<GetProjectsResponseItem> =
            match client.get_projects(None, Some(user_id), None).await {
                Ok(response) => response.into_inner(),
                Err(error) => {
                    return Err(generated_apply_api_error(
                        generated_operation_path("get_projects", &[])
                            .unwrap_or_else(|| "get_projects".to_string()),
                        error,
                    )
                    .await);
                }
            };

        if projects.is_empty() {
            return Err(ApplyError::NoProjectsFound);
        }

        let options: Vec<String> = projects
            .iter()
            .map(|project| format!("{} ({})", *project.project_name, project.project_id))
            .collect();
        let selected = Select::new("Select a project to apply to:", options)
            .prompt()
            .map_err(ApplyError::ProjectSelectionFailed)?;

        projects
            .into_iter()
            .find(|project| selected.ends_with(&format!("({})", project.project_id)))
            .map(|project| project.project_id)
            .ok_or_else(|| ApplyError::InvalidConfig("Selected project was not found".to_string()))
    }

    fn print_release_success(
        platform_url: &str,
        project_id: &Uuid,
        release: &PostProjectsProjectIdReleasesResponse,
    ) {
        let review_url = Url::parse(platform_url).map(|mut url| {
            url.set_path(&format!(
                "/dashboard/projects/{project_id}/releases/{}",
                release.id
            ));
            url
        });
        println!(
            "Release #{} created.\nReview at: {}",
            release.release_number,
            review_url.as_ref().map_or_else(
                |_| {
                    format!(
                        "{}/dashboard/projects/{project_id}/releases/{}",
                        platform_url.trim_end_matches('/'),
                        release.id
                    )
                },
                ToString::to_string
            )
        );
    }
}

#[cfg(feature = "credible")]
fn apply_data(
    outcome: &'static str,
    project_id: Uuid,
    verification: Option<&VerificationSummary>,
    payload: Option<&PostProjectsProjectIdReleasesBody>,
    preview: Option<&PreviewResponse>,
    applied: bool,
    release: Option<&PostProjectsProjectIdReleasesResponse>,
) -> Value {
    json!({
        "outcome": outcome,
        "project_id": project_id,
        "verification": verification,
        "payload": payload,
        "preview": preview,
        "applied": applied,
        "release": release,
    })
}

#[cfg(not(feature = "credible"))]
fn apply_data(
    outcome: &'static str,
    project_id: Uuid,
    payload: Option<&PostProjectsProjectIdReleasesBody>,
    preview: Option<&PreviewResponse>,
    applied: bool,
    release: Option<&PostProjectsProjectIdReleasesResponse>,
) -> Value {
    json!({
        "outcome": outcome,
        "project_id": project_id,
        "payload": payload,
        "preview": preview,
        "applied": applied,
        "release": release,
    })
}

/// Parse a string into a generated newtype, mapping the error to `ApplyError`.
fn parse_field<T>(value: &str, field: &str) -> Result<T, ApplyError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|e| ApplyError::InvalidConfig(format!("Invalid {field}: {e}")))
}

fn assertion_contracts_dir(file: &str) -> PathBuf {
    let source_path = file.split_once(':').map_or(file, |(path, _)| path);
    Path::new(source_path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(
            || PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR),
            Path::to_path_buf,
        )
}

fn build_assertion_item(
    assertion: &crate::credible_config::CredibleAssertion,
    built: &pcl_phoundry::build_and_flatten::BuildAndFlatOutput,
    contract_name: &str,
) -> Result<PostProjectsProjectIdReleasesBodyContractsValueAssertionsItem, ApplyError> {
    let constructor_abi_signature = abi::build_signature(&built.abi, &assertion.args)?;

    Ok(
        PostProjectsProjectIdReleasesBodyContractsValueAssertionsItem {
            file: parse_field(&assertion.file, "assertion file")?,
            args: assertion.args.clone(),
            constructor_abi_signature: Some(parse_field(
                &constructor_abi_signature,
                "constructor abi signature",
            )?),
            bytecode: parse_field(&built.bytecode, "bytecode")?,
            flattened_source: parse_field(&built.flattened_source, "flattened source")?,
            compiler_version: parse_field(&built.compiler_version, "compiler version")?,
            contract_name: parse_field(contract_name, "contract name")?,
            evm_version: parse_field(&built.evm_version, "evm version")?,
            optimizer_runs: built.optimizer_runs,
            optimizer_enabled: built.optimizer_enabled,
            metadata_bytecode_hash: parse_field(
                &built.metadata_bytecode_hash.to_string(),
                "metadata bytecode hash",
            )?,
            libraries: built.libraries.clone(),
        },
    )
}

fn build_contract_value(
    contract: &crate::credible_config::CredibleContract,
    assertions: Vec<PostProjectsProjectIdReleasesBodyContractsValueAssertionsItem>,
) -> Result<PostProjectsProjectIdReleasesBodyContractsValue, ApplyError> {
    Ok(PostProjectsProjectIdReleasesBodyContractsValue {
        address: parse_field(&contract.address, "contract address")?,
        name: Some(parse_field(&contract.name, "contract name")?),
        assertions,
    })
}

fn canonicalize_root(root: &Path) -> Result<PathBuf, ApplyError> {
    std::fs::canonicalize(root).map_err(|e| {
        ApplyError::Io {
            message: format!("Project root not found: {}", root.display()),
            source: e,
        }
    })
}

fn confirm_apply() -> Result<bool, ApplyError> {
    eprint!("Do you want to apply these changes? [Y/n]: ");
    stderr().flush().map_err(|e| {
        ApplyError::Io {
            message: "Failed to flush stderr".to_string(),
            source: e,
        }
    })?;
    let mut input = String::new();
    stdin().read_line(&mut input).map_err(|e| {
        ApplyError::Io {
            message: "Failed to read from stdin".to_string(),
            source: e,
        }
    })?;
    let trimmed = input.trim();
    Ok(trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("y")
        || trimmed.eq_ignore_ascii_case("yes"))
}

fn client_error_to_apply(error: ClientBuildError) -> ApplyError {
    match error {
        ClientBuildError::NoAuthToken => ApplyError::NoAuthToken,
        ClientBuildError::ExpiredAuthToken(expires_at) => ApplyError::ExpiredAuthToken(expires_at),
        ClientBuildError::AuthRefresh(error) => ApplyError::AuthRefresh(error),
        ClientBuildError::InvalidConfig(message) => ApplyError::InvalidConfig(message),
    }
}

async fn generated_apply_api_error<E>(endpoint: String, error: GeneratedError<E>) -> ApplyError
where
    E: serde::Serialize + std::fmt::Debug,
{
    let details = crate::api::generated_error_details(error).await;
    ApplyError::Api {
        endpoint,
        status: details.status,
        body: generated_error_body_string(&details.body),
    }
}

fn generated_error_body_string(body: &Value) -> String {
    body.as_str()
        .map_or_else(|| body.to_string(), ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credible_config::CredibleAssertion;
    use alloy_json_abi::{
        Constructor,
        JsonAbi,
        Param,
        StateMutability,
    };
    use pcl_phoundry::build_and_flatten::BuildAndFlatOutput;

    fn make_built(abi: JsonAbi) -> BuildAndFlatOutput {
        BuildAndFlatOutput {
            compiler_version: "v0.8.28+commit.7893614a".to_string(),
            flattened_source: "// SPDX\ncontract A {}".to_string(),
            abi,
            bytecode: "0x6080".to_string(),
            evm_version: "paris".to_string(),
            ..Default::default()
        }
    }

    /// Regression guard: `build_assertion_item` must forward the typed
    /// constructor signature so the dApp doesn't fall back to guessing
    /// `constructor(string,…)`.
    #[test]
    fn build_assertion_item_forwards_constructor_signature() {
        let abi = JsonAbi {
            constructor: Some(Constructor {
                inputs: vec![Param {
                    ty: "address".to_string(),
                    name: "owner".to_string(),
                    components: vec![],
                    internal_type: None,
                }],
                state_mutability: StateMutability::NonPayable,
            }),
            ..Default::default()
        };
        let built = make_built(abi);
        let assertion = CredibleAssertion {
            file: "src/A.a.sol:A".to_string(),
            args: vec!["0xF31b02F47596AcC7328E9fb04aFc52Fe91Da6071".to_string()],
        };

        let item = build_assertion_item(&assertion, &built, "A").expect("builds item");

        let sig = item
            .constructor_abi_signature
            .as_ref()
            .expect("signature is forwarded");
        assert_eq!(sig.as_str(), "constructor(address)");
        assert_eq!(item.args, assertion.args);
    }

    #[test]
    fn build_assertion_item_forwards_signature_with_no_constructor() {
        let built = make_built(JsonAbi::default());
        let assertion = CredibleAssertion {
            file: "src/A.a.sol:A".to_string(),
            args: vec![],
        };

        let item = build_assertion_item(&assertion, &built, "A").expect("builds item");

        let sig = item
            .constructor_abi_signature
            .as_ref()
            .expect("signature is forwarded");
        assert_eq!(sig.as_str(), "constructor()");
        assert!(item.args.is_empty());
    }
}
