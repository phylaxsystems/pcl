//! Download assertion source code for a protocol.
//!
//! Resolves a project by UUID (`--project-id`), fetches every assertion
//! registered against that project
//! via the platform API, and writes each Solidity source file to a local
//! output directory.

use crate::{
    DEFAULT_PLATFORM_URL,
    client::{
        ClientBuildError,
        authorization_header,
        ensure_fresh_auth,
    },
    config::CliConfig,
    error::AuthError,
    output::{
        OutputStream,
        ok_envelope,
        print_envelope,
    },
};
use pcl_common::args::{
    CliArgs,
    OutputMode,
};
use serde::Serialize;
use serde_json::{
    Value,
    json,
};
use std::path::{
    Path,
    PathBuf,
};
use uuid::Uuid;

#[derive(clap::Parser, Debug)]
#[command(
    name = "download",
    about = "Download assertion source code for a protocol"
)]
pub struct DownloadArgs {
    #[arg(long, help = "Project UUID to download assertions from")]
    pub project_id: Option<Uuid>,

    #[arg(
        short = 'o',
        long = "output-dir",
        value_hint = clap::ValueHint::DirPath,
        help = "Output directory for .sol files (default: <project_name>-assertions/)"
    )]
    pub output_dir: Option<PathBuf>,

    #[arg(
        short = 'u',
        long = "api-url",
        env = "PCL_API_URL",
        value_hint = clap::ValueHint::Url,
        default_value = DEFAULT_PLATFORM_URL,
        help = "Base URL for the platform API"
    )]
    pub api_url: url::Url,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("Run `pcl auth login` first")]
    NoAuthToken,

    #[error(
        "Stored auth token expired at {0}. Run `pcl auth refresh --toon` or `pcl auth login` again."
    )]
    ExpiredAuthToken(chrono::DateTime<chrono::Utc>),

    #[error("Failed to refresh stored auth before downloading assertions: {0}")]
    AuthRefresh(#[source] AuthError),

    #[error("--project-id is required")]
    MissingIdentifier,

    #[error("No assertions found for project")]
    NoAssertionsFound,

    #[error("API request to {endpoint} failed{status_part}{request_part}: {body}", status_part = .status.map_or(String::new(), |s| format!(" with status {s}")), request_part = .request_id.as_ref().map_or(String::new(), |id| format!(" request_id {id}")))]
    Api {
        endpoint: String,
        status: Option<u16>,
        request_id: Option<String>,
        body: Value,
    },

    #[error("{message}: {source}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to encode JSON output: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Failed to write structured output: {0}")]
    Output(#[from] crate::output::OutputError),

    #[error("Invalid config: {0}")]
    InvalidConfig(String),
}

#[derive(Debug, Serialize)]
struct DownloadedFile {
    assertion_id: String,
    contract_name: String,
    file_name: String,
    source: String,
}

#[derive(Debug)]
struct AssertionSummary {
    assertion_id: String,
    contract_name: Option<String>,
}

impl DownloadArgs {
    pub async fn run(
        &self,
        cli_args: &CliArgs,
        config: &mut CliConfig,
    ) -> Result<(), DownloadError> {
        let output_mode = cli_args.output_mode();

        ensure_fresh_auth(config, &self.api_url, cli_args)
            .await
            .map_err(client_error_to_download)?;
        let client = Self::build_client(config)?;

        let (project_id, project_name) = self.resolve_project(&client).await?;

        let assertions = self.fetch_assertions_list(&client, &project_id).await?;

        if assertions.is_empty() {
            return Self::handle_empty_assertions(output_mode, project_id, &project_name);
        }

        let output_dir = self.prepare_output_dir(&project_name)?;

        if output_mode == OutputMode::Human {
            println!(
                "Downloading {} assertion{} for project \"{project_name}\"...\n",
                assertions.len(),
                if assertions.len() == 1 { "" } else { "s" },
            );
        }

        let (downloaded, skipped) = self
            .download_assertions(&client, &project_id, &assertions, &output_dir, output_mode)
            .await?;

        Self::print_result(
            output_mode,
            project_id,
            &project_name,
            &downloaded,
            skipped,
            &output_dir,
        )
    }

    fn handle_empty_assertions(
        output_mode: OutputMode,
        project_id: Uuid,
        project_name: &str,
    ) -> Result<(), DownloadError> {
        if output_mode == OutputMode::Human {
            println!("No assertions found for project \"{project_name}\".");
            return Ok(());
        }
        let envelope = ok_envelope(
            download_data("no_assertions", project_id, project_name, None, &[], 0),
            no_assertions_next_actions(project_id),
        );
        print_envelope(&envelope, output_mode, OutputStream::Stdout).map_err(DownloadError::Output)
    }

    fn prepare_output_dir(&self, project_name: &str) -> Result<PathBuf, DownloadError> {
        let output_dir = self
            .output_dir
            .clone()
            .unwrap_or_else(|| default_output_dir(project_name));

        std::fs::create_dir_all(&output_dir).map_err(|e| {
            DownloadError::Io {
                message: format!(
                    "Failed to create output directory: {}",
                    output_dir.display()
                ),
                source: e,
            }
        })?;

        Ok(output_dir)
    }

    async fn download_assertions(
        &self,
        client: &reqwest::Client,
        project_id: &Uuid,
        assertions: &[AssertionSummary],
        output_dir: &Path,
        output_mode: OutputMode,
    ) -> Result<(Vec<DownloadedFile>, usize), DownloadError> {
        let mut downloaded = Vec::new();
        let mut skipped = 0usize;

        for assertion in assertions {
            let assertion_id = &assertion.assertion_id;
            let contract_name = assertion
                .contract_name
                .clone()
                .unwrap_or_else(|| "unknown".to_string());

            let detail = self
                .fetch_assertion_detail(client, project_id, assertion_id)
                .await?;

            let source_code = detail
                .pointer("/source/source_code")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    detail
                        .pointer("/artifact/solidity_source")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                });

            if let Some(code) = source_code {
                let id_prefix = assertion_id.get(..8).unwrap_or(assertion_id);
                let file_name = format!("{}_{}.sol", safe_file_stem(&contract_name), id_prefix);
                let file_path = output_dir.join(&file_name);

                std::fs::write(&file_path, &code).map_err(|e| {
                    DownloadError::Io {
                        message: format!("Failed to write file: {}", file_path.display()),
                        source: e,
                    }
                })?;

                if output_mode == OutputMode::Human {
                    println!("  {file_name}");
                }

                let source_label = detail
                    .pointer("/source/source_code")
                    .and_then(Value::as_str)
                    .map_or_else(
                        || {
                            detail
                                .get("artifact")
                                .map(|_| "artifact".to_string())
                                .unwrap_or_default()
                        },
                        |_| {
                            detail
                                .pointer("/source/verification_status")
                                .and_then(Value::as_str)
                                .unwrap_or("source")
                                .to_string()
                        },
                    );

                downloaded.push(DownloadedFile {
                    assertion_id: assertion_id.clone(),
                    contract_name: contract_name.clone(),
                    file_name: file_name.clone(),
                    source: source_label,
                });
            } else {
                skipped += 1;
                if output_mode == OutputMode::Human {
                    println!("  [skipped] {contract_name} — no source code available");
                }
            }
        }

        Ok((downloaded, skipped))
    }

    fn print_result(
        output_mode: OutputMode,
        project_id: Uuid,
        project_name: &str,
        downloaded: &[DownloadedFile],
        skipped: usize,
        output_dir: &Path,
    ) -> Result<(), DownloadError> {
        if output_mode == OutputMode::Human {
            println!(
                "\nDone. {} file{} written to {}/ ({skipped} skipped)",
                downloaded.len(),
                if downloaded.len() == 1 { "" } else { "s" },
                output_dir.display(),
            );
        } else {
            let envelope = ok_envelope(
                download_data(
                    "downloaded",
                    project_id,
                    project_name,
                    Some(output_dir),
                    downloaded,
                    skipped,
                ),
                downloaded_next_actions(project_id, output_dir),
            );
            print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
        }

        Ok(())
    }

    fn build_client(config: &CliConfig) -> Result<reqwest::Client, DownloadError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::HeaderName::from_static("api-version"),
            reqwest::header::HeaderValue::from_static("1"),
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            authorization_header(config).map_err(client_error_to_download)?,
        );

        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|error| {
                DownloadError::InvalidConfig(format!("Failed to build HTTP client: {error}"))
            })
    }

    async fn resolve_project(
        &self,
        client: &reqwest::Client,
    ) -> Result<(Uuid, String), DownloadError> {
        let pid = self.project_id.ok_or(DownloadError::MissingIdentifier)?;
        let project = self.get_json(client, &format!("/projects/{pid}")).await?;
        let project_id = project
            .get("project_id")
            .or_else(|| project.get("projectId"))
            .and_then(Value::as_str)
            .map_or(Ok(pid), Uuid::parse_str)
            .map_err(|error| {
                DownloadError::InvalidConfig(format!("Invalid project ID: {error}"))
            })?;
        let project_name = project
            .get("project_name")
            .or_else(|| project.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("project")
            .to_string();

        Ok((project_id, project_name))
    }

    async fn fetch_assertions_list(
        &self,
        client: &reqwest::Client,
        project_id: &Uuid,
    ) -> Result<Vec<AssertionSummary>, DownloadError> {
        let response = self
            .get_json(client, &format!("/views/projects/{project_id}/assertions"))
            .await?;
        let assertions = response
            .pointer("/data/assertions")
            .or_else(|| response.get("assertions"))
            .and_then(Value::as_array)
            .ok_or_else(|| {
                DownloadError::InvalidConfig(
                    "Invalid assertions response: missing data.assertions array".to_string(),
                )
            })?;

        assertions
            .iter()
            .map(|assertion| {
                let assertion_id = assertion
                    .get("assertion_id")
                    .or_else(|| assertion.get("assertionId"))
                    .or_else(|| assertion.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        DownloadError::InvalidConfig(
                            "Invalid assertions response: missing assertion_id".to_string(),
                        )
                    })?
                    .to_string();
                let contract_name = assertion
                    .get("contract_name")
                    .or_else(|| assertion.get("contractName"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                Ok(AssertionSummary {
                    assertion_id,
                    contract_name,
                })
            })
            .collect()
    }

    async fn fetch_assertion_detail(
        &self,
        client: &reqwest::Client,
        project_id: &Uuid,
        assertion_id: &str,
    ) -> Result<Value, DownloadError> {
        let response = self
            .get_json(
                client,
                &format!("/views/projects/{project_id}/assertions/{assertion_id}"),
            )
            .await?;
        Ok(response.get("data").cloned().unwrap_or(response))
    }

    async fn get_json(
        &self,
        client: &reqwest::Client,
        endpoint: &str,
    ) -> Result<Value, DownloadError> {
        let url = self.endpoint_url(endpoint);
        let response = client.get(url).send().await.map_err(|error| {
            DownloadError::Api {
                endpoint: endpoint.to_string(),
                status: error.status().map(|status| status.as_u16()),
                request_id: None,
                body: json!(error.to_string()),
            }
        })?;
        let status = response.status();
        let request_id = crate::api::request_id_from_headers(response.headers());
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.bytes().await.map_err(|error| {
            DownloadError::Api {
                endpoint: endpoint.to_string(),
                status: Some(status.as_u16()),
                request_id: request_id.clone(),
                body: json!(error.to_string()),
            }
        })?;
        let body = crate::api::response_body_value(&content_type, &bytes);
        if !status.is_success() {
            return Err(DownloadError::Api {
                endpoint: endpoint.to_string(),
                status: Some(status.as_u16()),
                request_id,
                body,
            });
        }
        Ok(body)
    }

    fn endpoint_url(&self, endpoint: &str) -> url::Url {
        let mut url = self.api_url.clone();
        url.set_path(&format!("/api/v1/{}", endpoint.trim_start_matches('/')));
        url
    }
}

fn client_error_to_download(error: ClientBuildError) -> DownloadError {
    match error {
        ClientBuildError::NoAuthToken => DownloadError::NoAuthToken,
        ClientBuildError::ExpiredAuthToken(expires_at) => {
            DownloadError::ExpiredAuthToken(expires_at)
        }
        ClientBuildError::AuthRefresh(error) => DownloadError::AuthRefresh(error),
        ClientBuildError::InvalidConfig(message) => DownloadError::InvalidConfig(message),
    }
}

fn download_data(
    outcome: &'static str,
    project_id: Uuid,
    project_name: &str,
    output_dir: Option<&Path>,
    files: &[DownloadedFile],
    files_skipped: usize,
) -> Value {
    json!({
        "outcome": outcome,
        "project_id": project_id,
        "project_name": project_name,
        "output_dir": output_dir.map(|path| path.display().to_string()),
        "files_downloaded": files.len(),
        "files_skipped": files_skipped,
        "files": files,
    })
}

fn no_assertions_next_actions(project_id: Uuid) -> Vec<String> {
    vec![format!("pcl assertions --project-id {project_id}")]
}

fn downloaded_next_actions(project_id: Uuid, output_dir: &Path) -> Vec<String> {
    vec![
        format!(
            "Inspect downloaded Solidity files in {}",
            output_dir.display()
        ),
        format!("pcl assertions --project-id {project_id}"),
    ]
}

fn default_output_dir(project_name: &str) -> PathBuf {
    PathBuf::from(format!("{}-assertions", safe_file_stem(project_name)))
}

fn safe_file_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "assertion".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: TestCommand,
    }

    #[derive(clap::Subcommand)]
    enum TestCommand {
        Download(DownloadArgs),
    }

    #[test]
    fn parses_download_with_project_id() {
        let cli = TestCli::try_parse_from([
            "pcl",
            "download",
            "--project-id",
            "550e8400-e29b-41d4-a716-446655440000",
        ])
        .unwrap();
        match cli.command {
            TestCommand::Download(args) => {
                assert_eq!(
                    args.project_id.unwrap().to_string(),
                    "550e8400-e29b-41d4-a716-446655440000"
                );
                assert!(args.output_dir.is_none());
            }
        }
    }

    #[test]
    fn parses_download_with_output_dir() {
        let cli = TestCli::try_parse_from([
            "pcl",
            "download",
            "--project-id",
            "550e8400-e29b-41d4-a716-446655440000",
            "--output-dir",
            "/tmp/my-sol-files",
        ])
        .unwrap();
        match cli.command {
            TestCommand::Download(args) => {
                assert_eq!(args.output_dir.unwrap(), PathBuf::from("/tmp/my-sol-files"));
            }
        }
    }

    #[test]
    fn rejects_download_local_json_flag_without_global_cli() {
        let result = TestCli::try_parse_from([
            "pcl",
            "download",
            "--project-id",
            "550e8400-e29b-41d4-a716-446655440000",
            "--json",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_manager_flag() {
        let result = TestCli::try_parse_from([
            "pcl",
            "download",
            "--manager",
            "0x1234567890abcdef1234567890abcdef12345678",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn no_assertions_next_actions_use_concrete_project_id() {
        let project_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();

        assert_eq!(
            no_assertions_next_actions(project_id),
            vec!["pcl assertions --project-id 550e8400-e29b-41d4-a716-446655440000"]
        );
    }

    #[test]
    fn downloaded_next_actions_do_not_suggest_invalid_verify_command() {
        let project_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap();
        let actions = downloaded_next_actions(project_id, Path::new("/tmp/pcl review download"));

        assert_eq!(
            actions[0],
            "Inspect downloaded Solidity files in /tmp/pcl review download"
        );
        assert_eq!(
            actions[1],
            "pcl assertions --project-id 550e8400-e29b-41d4-a716-446655440000"
        );
        assert!(
            !actions
                .iter()
                .any(|action| action.starts_with("pcl verify"))
        );
    }

    #[test]
    fn default_output_dir_sanitizes_project_name() {
        assert_eq!(
            default_output_dir("../escape").display().to_string(),
            "escape-assertions"
        );
        assert_eq!(
            default_output_dir("private test lea").display().to_string(),
            "private_test_lea-assertions"
        );
    }

    #[test]
    fn safe_file_stem_removes_path_separators() {
        assert_eq!(
            safe_file_stem("../Allowance/Guard.sol"),
            "Allowance_Guard_sol"
        );
        assert_eq!(safe_file_stem("   "), "assertion");
    }
}
