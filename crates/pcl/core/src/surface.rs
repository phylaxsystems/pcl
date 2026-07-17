#![allow(
    clippy::missing_errors_doc,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]

use crate::{
    api::{
        api_manifest,
        envelope_output_string,
        generated_operation_path,
        request_id_from_headers,
        response_body_value,
        with_envelope_metadata,
    },
    auth::refresh_stored_auth,
    config::{
        AUTH_EXPIRES_SOON_SECONDS,
        CliConfig,
        UserAuth,
    },
    error::AuthError,
    output::shell_word,
    request_log,
};
use chrono::Utc;
use dapp_api_client::generated::client::{
    Client as GeneratedClient,
    Error as GeneratedError,
    ResponseValue,
    types::{
        ApiError as GeneratedApiErrorBody,
        GetViewsProjectsProjectIdIncidentsEnvironment,
    },
};
use pcl_common::args::{
    CliArgs,
    OutputMode,
    current_output_mode,
};
use reqwest::header::{
    HeaderMap,
    HeaderName,
    HeaderValue,
};
use serde_json::{
    Map,
    Value,
    json,
};
use sha2::{
    Digest,
    Sha256,
};
use std::{
    collections::HashSet,
    fs,
    io::{
        BufRead,
        BufReader,
        BufWriter,
        Write,
    },
    num::NonZeroU64,
    path::{
        Path,
        PathBuf,
    },
};
use tokio::time::{
    Duration,
    sleep,
};
use uuid::Uuid;

const ARTIFACT_DIR_ENV: &str = "PCL_ARTIFACT_DIR";
const JOBS_FILE_ENV: &str = "PCL_JOBS_FILE";

struct ExportPageResponse {
    status: reqwest::StatusCode,
    request_id: Option<String>,
    body: Value,
    attempts: u64,
}

#[derive(Debug, thiserror::Error)]
enum ExportPageError {
    #[error("Request failed after {attempts} attempts: {source}")]
    Request {
        attempts: u64,
        #[source]
        source: reqwest::Error,
    },
    #[error("Generated API response failed after {attempts} attempts: {message}")]
    Generated { attempts: u64, message: String },
    #[error("Failed to serialize generated response after {attempts} attempts: {source}")]
    Serialization {
        attempts: u64,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProductSurfaceError {
    #[error("Run `pcl auth login` first")]
    NoAuthToken,

    #[error("Stored auth token expired at {0}")]
    ExpiredAuthToken(chrono::DateTime<chrono::Utc>),

    #[error("Failed to refresh stored auth before running the command: {0}")]
    AuthRefresh(#[source] AuthError),

    #[error(transparent)]
    PlatformMismatch(AuthError),

    #[error("{0}")]
    InvalidInput(String),

    #[error("I/O failed for `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Request failed after {attempts} attempts for GET {path} page {page}: {source}")]
    ExportRequest {
        path: String,
        page: u64,
        attempts: u64,
        #[source]
        source: reqwest::Error,
    },

    #[error("Request failed with status {status} for {method} {path}")]
    HttpStatus {
        method: &'static str,
        path: String,
        status: u16,
        request_id: Option<String>,
        body: Box<Value>,
    },
}

impl ProductSurfaceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAuthToken => "auth.no_token",
            Self::ExpiredAuthToken(_) => "auth.expired_token",
            Self::AuthRefresh(_) => "auth.refresh_failed",
            Self::PlatformMismatch(_) => "auth.platform_mismatch",
            Self::InvalidInput(_) => "input.invalid",
            Self::Io { .. } => "io.failed",
            Self::Json(_) => "json.failed",
            Self::Request(_) | Self::ExportRequest { .. } => "network.request_failed",
            Self::HttpStatus { status, .. } => {
                match *status {
                    401 => "auth.unauthorized",
                    403 => "auth.forbidden",
                    404 => "api.not_found",
                    500..=599 => "api.server_error",
                    _ => "api.request_failed",
                }
            }
        }
    }

    pub fn json_envelope(&self) -> Value {
        let mut error = Map::new();
        error.insert("code".to_string(), json!(self.code()));
        error.insert("message".to_string(), json!(self.to_string()));
        error.insert("recoverable".to_string(), json!(self.recoverable()));
        if let Self::ExportRequest {
            path,
            page,
            attempts,
            ..
        } = self
        {
            error.insert(
                "export".to_string(),
                json!({
                    "path": path,
                    "page": page,
                    "attempts": attempts,
                }),
            );
        }

        if let Self::HttpStatus {
            method,
            path,
            status,
            request_id,
            body,
        } = self
        {
            error.insert("request_id".to_string(), json!(request_id));
            error.insert(
                "http".to_string(),
                json!({
                    "method": method,
                    "path": path,
                    "status": status,
                    "request_id": request_id,
                    "body": body.as_ref(),
                }),
            );
        }

        with_envelope_metadata(json!({
            "status": "error",
            "error": error,
            "next_actions": self.next_actions(),
        }))
    }

    fn recoverable(&self) -> bool {
        !matches!(self, Self::Json(_))
    }

    fn next_actions(&self) -> Vec<String> {
        match self {
            Self::NoAuthToken => {
                vec!["pcl auth login".to_string(), "pcl doctor".to_string()]
            }
            Self::ExpiredAuthToken(_) | Self::AuthRefresh(_) => {
                vec![
                    "pcl auth refresh".to_string(),
                    "pcl auth login --force".to_string(),
                    "pcl doctor".to_string(),
                ]
            }
            Self::PlatformMismatch(_) => {
                vec![
                    "pcl auth login --auth-url <platform-url>".to_string(),
                    "pcl auth status --json".to_string(),
                    "Use --allow-unauthenticated only for public endpoints".to_string(),
                ]
            }
            Self::InvalidInput(message) if message.starts_with("Unknown job") => {
                vec![
                    "pcl jobs list".to_string(),
                    "pcl export incidents --help".to_string(),
                ]
            }
            Self::InvalidInput(message) if message.contains("developer pass-through command") => {
                vec![
                    "Run the command again without --json".to_string(),
                    "pcl verify".to_string(),
                    "pcl apply --dry-run".to_string(),
                ]
            }
            Self::InvalidInput(_) => {
                vec!["pcl workflows".to_string(), "pcl schema list".to_string()]
            }
            Self::Io { .. } => vec!["pcl artifacts path".to_string()],
            Self::Json(_) => vec!["Retry with --json to inspect the envelope".to_string()],
            Self::Request(_) => vec!["pcl doctor".to_string(), "Check --api-url".to_string()],
            Self::ExportRequest { .. } => {
                vec![
                    "pcl doctor".to_string(),
                    "Check --api-url".to_string(),
                    "Retry the export with --resume".to_string(),
                    "pcl jobs list".to_string(),
                ]
            }
            Self::HttpStatus {
                status: 401 | 403, ..
            } => vec!["pcl auth login".to_string(), "pcl whoami".to_string()],
            Self::HttpStatus {
                status: 500..=599,
                request_id,
                ..
            } => {
                let mut actions = vec![
                    "Retry later".to_string(),
                    "pcl requests list --limit 20".to_string(),
                ];
                if let Some(request_id) = request_id {
                    actions.push(format!(
                        "Contact platform support with request_id {request_id}"
                    ));
                }
                actions
            }
            Self::HttpStatus { .. } => vec!["pcl requests list".to_string()],
        }
    }
}

#[derive(clap::Args, Debug)]
#[command(about = "Diagnose config, auth, and platform API reachability")]
pub struct DoctorArgs {
    #[arg(
        long = "api-url",
        env = "PCL_API_URL",
        default_value = crate::config::default_platform_url(),
        help = "Base URL for the platform API. Defaults to the URL remembered from the last login"
    )]
    api_url: url::Url,
    #[arg(long, help = "Skip network health checks")]
    offline: bool,
}

#[derive(clap::Args, Debug)]
#[command(about = "Show the current local identity and token state")]
pub struct WhoamiArgs {
    #[arg(long, help = "Only inspect local configuration")]
    offline: bool,
}

#[derive(clap::Args, Debug)]
#[command(about = "Show agent-friendly workflow recipes")]
pub struct WorkflowsArgs {
    #[command(subcommand)]
    command: Option<WorkflowCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum WorkflowCommand {
    #[command(about = "List available workflow recipes")]
    List,
    #[command(about = "Show one workflow recipe")]
    Show { name: String },
}

#[derive(clap::Args, Debug)]
#[command(about = "Manage generated artifacts")]
pub struct ArtifactsArgs {
    #[command(subcommand)]
    command: Option<ArtifactsCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum ArtifactsCommand {
    #[command(about = "Print artifact directory")]
    Path,
    #[command(about = "Create artifact directory")]
    Init,
    #[command(about = "List artifacts")]
    List {
        #[arg(long, default_value_t = 50, help = "Maximum artifacts to list")]
        limit: usize,
    },
}

#[derive(clap::Args, Debug)]
#[command(about = "Inspect local API request logs")]
pub struct RequestsArgs {
    #[command(subcommand)]
    command: Option<RequestsCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum RequestsCommand {
    #[command(about = "Print request log path")]
    Path,
    #[command(about = "List recent request log entries")]
    List {
        #[arg(long, default_value_t = 20, help = "Maximum records to list")]
        limit: usize,
    },
    #[command(about = "Clear the local request log")]
    Clear,
}

#[derive(clap::Args, Debug)]
#[command(about = "Inspect machine-readable command and body schemas")]
pub struct SchemaArgs {
    #[command(subcommand)]
    command: Option<SchemaCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum SchemaCommand {
    #[command(about = "List workflow schemas")]
    List,
    #[command(about = "Get one workflow schema, optionally narrowed to one action")]
    Get {
        workflow: String,
        #[arg(long, help = "Action name within the workflow")]
        action: Option<String>,
    },
}

#[derive(clap::Args, Debug)]
#[command(about = "Print a CLI-native LLM usage guide")]
pub struct LlmsArgs;

#[derive(clap::Args, Debug)]
#[command(about = "Inspect and resume local CLI jobs")]
pub struct JobsArgs {
    #[command(subcommand)]
    command: Option<JobsCommand>,
}

#[derive(clap::Subcommand, Debug)]
enum JobsCommand {
    #[command(about = "List known local jobs")]
    List {
        #[arg(long, default_value_t = 20, help = "Maximum jobs to list")]
        limit: usize,
    },
    #[command(about = "Show one local job")]
    Status { job_id: String },
    #[command(about = "Show the command needed to resume one local job")]
    Resume { job_id: String },
    #[command(about = "Mark one local job canceled")]
    Cancel { job_id: String },
    #[command(about = "Print the local job registry path")]
    Path,
}

#[derive(clap::Args, Debug)]
#[command(about = "Export investigation data as resumable artifacts")]
pub struct ExportArgs {
    #[command(subcommand)]
    command: ExportCommand,
}

#[derive(clap::Subcommand, Debug)]
enum ExportCommand {
    #[command(about = "Export incident list data as JSONL")]
    Incidents(ExportIncidentsArgs),
}

#[derive(clap::Args, Debug)]
struct ExportIncidentsArgs {
    #[arg(
        long,
        alias = "project",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project_id: Option<String>,
    #[arg(long, help = "Filter by environment")]
    environment: Option<String>,
    #[arg(long, default_value_t = 1, help = "Starting page")]
    page: u64,
    #[arg(long, default_value_t = 50, help = "Items per page")]
    limit: u64,
    #[arg(long, default_value_t = 100, help = "Maximum pages to fetch")]
    max_pages: u64,
    #[arg(long, help = "Write incidents as JSONL to this path")]
    out: Option<PathBuf>,
    #[arg(long, help = "Write export errors as JSONL to this path")]
    errors: Option<PathBuf>,
    #[arg(long, help = "Checkpoint file for resumable exports")]
    checkpoint: Option<PathBuf>,
    #[arg(long, help = "Resume from checkpoint when available")]
    resume: bool,
    #[arg(long, help = "Continue after page-level API errors")]
    continue_on_error: bool,
    #[arg(
        long,
        default_value_t = 3,
        help = "Retry transient page fetch failures before recording an export error"
    )]
    max_retries: u64,
    #[arg(long, help = "Print the export plan without fetching data")]
    dry_run: bool,
    #[arg(
        long = "api-url",
        env = "PCL_API_URL",
        default_value = crate::config::default_platform_url(),
        help = "Base URL for the platform API. Defaults to the URL remembered from the last login"
    )]
    api_url: url::Url,
    #[arg(long, help = "Do not attach a stored bearer token")]
    allow_unauthenticated: bool,
}

impl DoctorArgs {
    pub async fn run(
        &self,
        config: &CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ProductSurfaceError> {
        let mut checks = vec![
            json!({
                "name": "config",
                "status": "ok",
                "path": CliConfig::config_file_path(cli_args).display().to_string(),
                "exists": CliConfig::config_file_path(cli_args).exists(),
            }),
            json!({
                "name": "auth",
                "status": auth_check_status(config.auth.as_ref()),
                "details": auth_value(config.auth.as_ref()),
            }),
            json!({
                "name": "request_log",
                "status": "ok",
                "path": request_log::request_log_path_for_args(cli_args).display().to_string(),
            }),
            json!({
                "name": "artifacts",
                "status": "ok",
                "path": artifact_dir(cli_args).display().to_string(),
            }),
        ];

        if !self.offline {
            checks.push(health_check(&self.api_url).await);
            checks.push(auth_capability_check(&self.api_url).await);
        }

        let status = if checks
            .iter()
            .any(|check| check["status"].as_str() == Some("error"))
        {
            "error"
        } else if checks
            .iter()
            .any(|check| matches!(check["status"].as_str(), Some("warning" | "missing")))
        {
            "warning"
        } else {
            "ok"
        };

        let next_actions = doctor_next_actions(status, config.auth.as_ref());

        print_output(
            &json!({
                "status": status,
                "data": {
                    "checks": checks,
                    "default_output": "human",
                    "json_output_flag": "--json",
                    "api_url": self.api_url.as_str(),
                },
                "next_actions": next_actions,
            }),
            json_output,
        )
    }
}

impl WhoamiArgs {
    pub fn run(&self, config: &CliConfig, json_output: bool) -> Result<(), ProductSurfaceError> {
        if let Some(auth) = &config.auth
            && auth.expires_at <= Utc::now()
        {
            return Err(ProductSurfaceError::ExpiredAuthToken(auth.expires_at));
        }

        print_output(
            &json!({
                "status": "ok",
                "data": {
                    "offline": self.offline,
                    "auth": auth_value(config.auth.as_ref()),
                },
                "next_actions": if config.auth.is_some() {
                    json!(["pcl account", "pcl projects mine", "pcl doctor"])
                } else {
                    json!(["pcl auth login", "pcl doctor"])
                },
            }),
            json_output,
        )
    }
}

impl WorkflowsArgs {
    pub fn run(&self, json_output: bool) -> Result<(), ProductSurfaceError> {
        let workflows = workflow_recipes();
        let data = match &self.command {
            None | Some(WorkflowCommand::List) => json!({ "workflows": workflows }),
            Some(WorkflowCommand::Show { name }) => {
                workflows
                    .iter()
                    .find(|workflow| workflow["name"].as_str() == Some(name.as_str()))
                    .cloned()
                    .ok_or_else(|| {
                        ProductSurfaceError::InvalidInput(format!("Unknown workflow `{name}`"))
                    })?
            }
        };
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": ["pcl schema list", "pcl api manifest"],
            }),
            json_output,
        )
    }
}

impl ArtifactsArgs {
    pub fn run(&self, cli_args: &CliArgs, json_output: bool) -> Result<(), ProductSurfaceError> {
        let dir = artifact_dir(cli_args);
        let data = match &self.command {
            Some(ArtifactsCommand::Path) => json!({ "artifact_dir": dir }),
            Some(ArtifactsCommand::Init) => {
                fs::create_dir_all(&dir).map_err(|source| {
                    ProductSurfaceError::Io {
                        path: dir.clone(),
                        source,
                    }
                })?;
                json!({ "artifact_dir": dir, "created": true })
            }
            None | Some(ArtifactsCommand::List { .. }) => {
                let limit = match &self.command {
                    Some(ArtifactsCommand::List { limit }) => *limit,
                    _ => 50,
                };
                json!({
                    "artifact_dir": dir,
                    "artifacts": list_artifacts(&dir, limit)?,
                })
            }
        };
        let next_actions = match &self.command {
            Some(ArtifactsCommand::Path) => {
                json!(["pcl artifacts list", "pcl export incidents --help"])
            }
            Some(ArtifactsCommand::Init) => json!(["pcl artifacts list", "pcl artifacts path"]),
            None | Some(ArtifactsCommand::List { .. }) => {
                json!(["pcl export incidents --help", "pcl artifacts path"])
            }
        };
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": next_actions,
            }),
            json_output,
        )
    }
}

impl RequestsArgs {
    pub fn run(&self, json_output: bool) -> Result<(), ProductSurfaceError> {
        let path = request_log::request_log_path();
        self.run_with_path(&path, json_output)
    }

    pub fn run_with_cli_args(
        &self,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ProductSurfaceError> {
        let path = request_log::request_log_path_for_args(cli_args);
        self.run_with_path(&path, json_output)
    }

    fn run_with_path(&self, path: &Path, json_output: bool) -> Result<(), ProductSurfaceError> {
        let data = match &self.command {
            Some(RequestsCommand::Path) => json!({ "request_log": path }),
            Some(RequestsCommand::Clear) => {
                let deleted = request_log::clear_request_log_at(path).map_err(|source| {
                    ProductSurfaceError::Io {
                        path: path.to_path_buf(),
                        source,
                    }
                })?;
                json!({ "request_log": path, "deleted": deleted })
            }
            None | Some(RequestsCommand::List { .. }) => {
                let limit = match &self.command {
                    Some(RequestsCommand::List { limit }) => *limit,
                    _ => 20,
                };
                let records =
                    request_log::read_request_records_at(path, limit).map_err(|source| {
                        ProductSurfaceError::Io {
                            path: path.to_path_buf(),
                            source,
                        }
                    })?;
                json!({ "request_log": path, "records": records })
            }
        };
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": ["pcl doctor", "pcl api call get /health --allow-unauthenticated"],
            }),
            json_output,
        )
    }
}

impl SchemaArgs {
    pub fn run(&self, json_output: bool) -> Result<(), ProductSurfaceError> {
        let manifest = api_manifest();
        let commands = manifest["commands"].as_array().cloned().unwrap_or_default();
        let data = match &self.command {
            None | Some(SchemaCommand::List) => {
                let schemas = commands
                    .iter()
                    .filter_map(|command| {
                        command["output_policy"].as_str()?;
                        let command_text = command["command"].as_str()?;
                        let workflow = command_text.split_whitespace().nth(1)?;
                        Some(json!({
                            "workflow": workflow,
                            "command": command_text,
                            "description": command["description"],
                            "output": command["output"],
                            "output_policy": command["output_policy"],
                            "actions": command["actions"].as_array().map_or(0, Vec::len),
                        }))
                    })
                    .collect::<Vec<_>>();
                json!({ "schemas": schemas })
            }
            Some(SchemaCommand::Get { workflow, action }) => {
                let mut schema = find_workflow_schema(&commands, workflow)?;
                if let Some(action_name) = action {
                    let action_value = schema["actions"]
                        .as_array()
                        .and_then(|actions| {
                            actions
                                .iter()
                                .find(|candidate| candidate["name"].as_str() == Some(action_name))
                        })
                        .cloned()
                        .ok_or_else(|| {
                            ProductSurfaceError::InvalidInput(format!(
                                "Unknown action `{action_name}` for workflow `{workflow}`"
                            ))
                        })?;
                    schema = json!({
                        "workflow": workflow,
                        "action": action_value,
                    });
                }
                schema
            }
        };
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": ["pcl workflows", "pcl api manifest"],
            }),
            json_output,
        )
    }
}

impl LlmsArgs {
    pub fn run(&self, json_output: bool) -> Result<(), ProductSurfaceError> {
        print_llms_guide(json_output)
    }
}

impl JobsArgs {
    pub fn run(&self, cli_args: &CliArgs, json_output: bool) -> Result<(), ProductSurfaceError> {
        let path = jobs_path(cli_args);
        let data = match &self.command {
            Some(JobsCommand::Path) => json!({ "jobs_path": path }),
            None | Some(JobsCommand::List { .. }) => {
                let limit = match &self.command {
                    Some(JobsCommand::List { limit }) => *limit,
                    _ => 20,
                };
                json!({
                    "jobs_path": path,
                    "jobs": list_jobs(cli_args, limit)?,
                })
            }
            Some(JobsCommand::Status { job_id }) => find_job(cli_args, job_id)?,
            Some(JobsCommand::Resume { job_id }) => {
                let job = find_job(cli_args, job_id)?;
                json!({
                    "job": job,
                    "resume_command": job["resume_command"],
                })
            }
            Some(JobsCommand::Cancel { job_id }) => {
                let mut job = find_job(cli_args, job_id)?;
                let updated_at = Utc::now().to_rfc3339();
                if let Some(object) = job.as_object_mut() {
                    object.insert("status".to_string(), json!("canceled"));
                    object.insert("updated_at".to_string(), json!(updated_at));
                }
                append_job_record(cli_args, &job)?;
                job
            }
        };
        let next_actions = match &self.command {
            None | Some(JobsCommand::List { .. }) => {
                data.get("jobs")
                    .and_then(Value::as_array)
                    .and_then(|jobs| jobs.first())
                    .and_then(|job| job.get("job_id"))
                    .and_then(Value::as_str)
                    .map_or_else(
                        || json!(["pcl export incidents --help"]),
                        |job_id| {
                            json!([
                                format!("pcl jobs status {job_id}"),
                                format!("pcl jobs resume {job_id}"),
                                "pcl export incidents --help",
                            ])
                        },
                    )
            }
            _ => json!(["pcl jobs list", "pcl export incidents --help"]),
        };
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": next_actions,
            }),
            json_output,
        )
    }
}

impl ExportArgs {
    pub async fn run(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ProductSurfaceError> {
        match &self.command {
            ExportCommand::Incidents(args) => {
                export_incidents(args, config, cli_args, json_output).await
            }
        }
    }
}

async fn export_incidents(
    args: &ExportIncidentsArgs,
    config: &mut CliConfig,
    cli_args: &CliArgs,
    json_output: bool,
) -> Result<(), ProductSurfaceError> {
    if args.limit == 0 {
        return Err(ProductSurfaceError::InvalidInput(
            "--limit must be greater than zero".to_string(),
        ));
    }
    if args.page == 0 {
        return Err(ProductSurfaceError::InvalidInput(
            "--page must be greater than zero".to_string(),
        ));
    }
    if args.max_pages == 0 {
        return Err(ProductSurfaceError::InvalidInput(
            "--max-pages must be greater than zero".to_string(),
        ));
    }
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| artifact_dir(cli_args).join("incidents.jsonl"));
    let errors = args
        .errors
        .clone()
        .unwrap_or_else(|| artifact_dir(cli_args).join("incident-errors.jsonl"));
    let checkpoint = args
        .checkpoint
        .clone()
        .unwrap_or_else(|| artifact_dir(cli_args).join("incident-export-checkpoint.json"));
    let plan = export_plan(args, &out, &errors, &checkpoint);
    let job_id = incident_export_job_id(args, &checkpoint);
    let output_mode = if json_output {
        OutputMode::Json
    } else {
        current_output_mode()
    };
    let resume_command =
        incident_export_resume_command(args, &out, &errors, &checkpoint, output_mode);

    if args.dry_run {
        return print_output(
            &json!({
                "status": "ok",
                "data": {
                    "job_id": job_id,
                    "resume_command": resume_command,
                    "plan": plan,
                },
                "next_actions": ["Remove --dry-run to execute", "pcl artifacts list"],
            }),
            json_output,
        );
    }

    ensure_parent_dir(&out)?;
    ensure_parent_dir(&errors)?;
    ensure_parent_dir(&checkpoint)?;
    ensure_export_auth(
        config,
        cli_args,
        &args.api_url,
        args.project_id.is_some(),
        args.allow_unauthenticated,
    )
    .await?;

    let start_page = if args.resume {
        read_checkpoint_page(&checkpoint).unwrap_or(args.page)
    } else {
        args.page
    };
    if start_page == 0 {
        return Err(ProductSurfaceError::InvalidInput(
            "Checkpoint page must be greater than zero".to_string(),
        ));
    }
    let limit = NonZeroU64::new(args.limit).ok_or_else(|| {
        ProductSurfaceError::InvalidInput("Export limit must be greater than zero".to_string())
    })?;
    if args.project_id.is_none() && args.environment.is_some() {
        return Err(ProductSurfaceError::InvalidInput(
            "--environment requires --project-id for incident exports".to_string(),
        ));
    }
    let mut out_file = BufWriter::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)
            .map_err(|source| {
                ProductSurfaceError::Io {
                    path: out.clone(),
                    source,
                }
            })?,
    );
    let mut error_file = BufWriter::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&errors)
            .map_err(|source| {
                ProductSurfaceError::Io {
                    path: errors.clone(),
                    source,
                }
            })?,
    );

    let http_client = reqwest::Client::builder()
        .default_headers(default_headers(
            config,
            args.project_id.is_some(),
            args.allow_unauthenticated,
        )?)
        .build()?;
    let client = generated_client_with_http_client(&args.api_url, http_client);
    let project_id = match args.project_id.as_deref() {
        Some(project_ref) => Some(resolve_export_project_id(&client, project_ref).await?),
        None => None,
    };
    let environment = args
        .environment
        .as_deref()
        .map(GetViewsProjectsProjectIdIncidentsEnvironment::try_from)
        .transpose()
        .map_err(|error| {
            ProductSurfaceError::InvalidInput(format!("Invalid export environment: {error}"))
        })?;
    let mut pages_fetched = 0_u64;
    let mut incidents_written = 0_u64;
    let mut errors_written = 0_u64;
    let mut retries_attempted = 0_u64;
    let mut log_warnings = 0_u64;
    append_job_record(
        cli_args,
        &job_record(
            &job_id,
            "incident_export",
            "running",
            &resume_command,
            &out,
            &errors,
            &checkpoint,
        ),
    )?;

    for offset in 0..args.max_pages {
        let page_value = start_page + offset;
        let page = NonZeroU64::new(page_value).ok_or_else(|| {
            ProductSurfaceError::InvalidInput("Export page must be greater than zero".to_string())
        })?;
        let path = export_incidents_operation_path(project_id.as_ref());

        let response = match fetch_export_page(
            &client,
            project_id.as_ref(),
            environment,
            page,
            limit,
            args.max_retries,
        )
        .await
        {
            Ok(response) => response,
            Err(ExportPageError::Request { attempts, source }) => {
                retries_attempted += attempts.saturating_sub(1);
                errors_written += 1;
                if log_request(cli_args, "export", "GET", &path, 0, None) {
                    log_warnings += 1;
                }
                write_jsonl(
                    &mut error_file,
                    &json!({
                        "page": page_value,
                        "path": path.clone(),
                        "status": null,
                        "request_id": null,
                        "attempts": attempts,
                        "error": {
                            "code": "network.request_failed",
                            "message": source.to_string(),
                        },
                    }),
                )?;
                flush_jsonl_writer(&mut error_file, &errors)?;
                append_job_record(
                    cli_args,
                    &with_job_stats(
                        job_record(
                            &job_id,
                            "incident_export",
                            "failed",
                            &resume_command,
                            &out,
                            &errors,
                            &checkpoint,
                        ),
                        incident_export_stats(
                            pages_fetched,
                            incidents_written,
                            errors_written,
                            retries_attempted,
                            log_warnings,
                        ),
                    ),
                )?;
                return Err(ProductSurfaceError::ExportRequest {
                    path,
                    page: page.get(),
                    attempts,
                    source,
                });
            }
            Err(ExportPageError::Generated { attempts, message }) => {
                retries_attempted += attempts.saturating_sub(1);
                errors_written += 1;
                if log_request(cli_args, "export", "GET", &path, 0, None) {
                    log_warnings += 1;
                }
                write_jsonl(
                    &mut error_file,
                    &json!({
                        "page": page.get(),
                        "path": path.clone(),
                        "status": null,
                        "request_id": null,
                        "attempts": attempts,
                        "error": {
                            "code": "api.invalid_generated_response",
                            "message": message,
                        },
                    }),
                )?;
                flush_jsonl_writer(&mut error_file, &errors)?;
                append_job_record(
                    cli_args,
                    &with_job_stats(
                        job_record(
                            &job_id,
                            "incident_export",
                            "failed",
                            &resume_command,
                            &out,
                            &errors,
                            &checkpoint,
                        ),
                        incident_export_stats(
                            pages_fetched,
                            incidents_written,
                            errors_written,
                            retries_attempted,
                            log_warnings,
                        ),
                    ),
                )?;
                return Err(ProductSurfaceError::InvalidInput(message));
            }
            Err(ExportPageError::Serialization { source, .. }) => {
                return Err(ProductSurfaceError::Json(source));
            }
        };
        let status = response.status;
        let request_id = response.request_id;
        let body = response.body;
        retries_attempted += response.attempts.saturating_sub(1);
        if log_request(
            cli_args,
            "export",
            "GET",
            &path,
            status.as_u16(),
            request_id.as_deref(),
        ) {
            log_warnings += 1;
        }

        if !status.is_success() {
            errors_written += 1;
            write_jsonl(
                &mut error_file,
                &json!({
                    "page": page_value,
                    "path": path,
                    "status": status.as_u16(),
                    "request_id": request_id,
                    "attempts": response.attempts,
                    "body": body.clone(),
                }),
            )?;
            if args.continue_on_error {
                flush_jsonl_writer(&mut error_file, &errors)?;
                continue;
            }
            flush_jsonl_writer(&mut error_file, &errors)?;
            append_job_record(
                cli_args,
                &with_job_stats(
                    job_record(
                        &job_id,
                        "incident_export",
                        "failed",
                        &resume_command,
                        &out,
                        &errors,
                        &checkpoint,
                    ),
                    incident_export_stats(
                        pages_fetched,
                        incidents_written,
                        errors_written,
                        retries_attempted,
                        log_warnings,
                    ),
                ),
            )?;
            return Err(ProductSurfaceError::HttpStatus {
                method: "GET",
                path,
                status: status.as_u16(),
                request_id,
                body: Box::new(body),
            });
        }

        let incidents = extract_items(&body, "incidents");
        let page_count = incidents.len();
        for incident in incidents {
            write_jsonl(&mut out_file, &incident)?;
            incidents_written += 1;
        }
        pages_fetched += 1;
        flush_jsonl_writer(&mut out_file, &out)?;
        write_checkpoint(&checkpoint, page_value + 1, incidents_written)?;
        if page_count < usize::try_from(args.limit).unwrap_or(usize::MAX) {
            break;
        }
    }
    flush_jsonl_writer(&mut out_file, &out)?;
    flush_jsonl_writer(&mut error_file, &errors)?;
    let final_status = if errors_written == 0 {
        "completed"
    } else {
        "completed_with_errors"
    };
    append_job_record(
        cli_args,
        &with_job_stats(
            job_record(
                &job_id,
                "incident_export",
                final_status,
                &resume_command,
                &out,
                &errors,
                &checkpoint,
            ),
            incident_export_stats(
                pages_fetched,
                incidents_written,
                errors_written,
                retries_attempted,
                log_warnings,
            ),
        ),
    )?;

    print_output(
        &json!({
            "status": "ok",
            "data": {
                "job_id": job_id,
                "export": "incidents",
                "resume_command": resume_command,
                "out": out,
                "errors": errors,
                "checkpoint": checkpoint,
                "pages_fetched": pages_fetched,
                "incidents_written": incidents_written,
                "errors_written": errors_written,
                "retries_attempted": retries_attempted,
                "log_warnings": log_warnings,
            },
            "next_actions": [
                "pcl artifacts list",
                "pcl requests list --limit 20",
            ],
        }),
        json_output,
    )
}

fn print_output(value: &Value, json_output: bool) -> Result<(), ProductSurfaceError> {
    print!("{}", envelope_output_string(value, json_output)?);
    Ok(())
}

fn artifact_dir(cli_args: &CliArgs) -> PathBuf {
    std::env::var_os(ARTIFACT_DIR_ENV).map_or_else(
        || {
            cli_args
                .config_dir
                .clone()
                .unwrap_or_else(CliConfig::get_config_dir)
                .join("artifacts")
        },
        PathBuf::from,
    )
}

fn jobs_path(cli_args: &CliArgs) -> PathBuf {
    std::env::var_os(JOBS_FILE_ENV)
        .map_or_else(|| artifact_dir(cli_args).join("jobs.jsonl"), PathBuf::from)
}

fn append_job_record(cli_args: &CliArgs, record: &Value) -> Result<(), ProductSurfaceError> {
    let path = jobs_path(cli_args);
    ensure_parent_dir(&path)?;
    let mut file = BufWriter::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| {
                ProductSurfaceError::Io {
                    path: path.clone(),
                    source,
                }
            })?,
    );
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n").map_err(|source| {
        ProductSurfaceError::Io {
            path: path.clone(),
            source,
        }
    })?;
    flush_jsonl_writer(&mut file, &path)?;
    Ok(())
}

fn read_job_records(cli_args: &CliArgs) -> Result<Vec<Value>, ProductSurfaceError> {
    let path = jobs_path(cli_args);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path).map_err(|source| {
        ProductSurfaceError::Io {
            path: path.clone(),
            source,
        }
    })?;
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line = match line {
                Ok(line) if line.trim().is_empty() => return None,
                Ok(line) => line,
                Err(source) => {
                    return Some(Err(ProductSurfaceError::Io {
                        path: path.clone(),
                        source,
                    }));
                }
            };
            Some(serde_json::from_str(&line).map_err(|source| {
                ProductSurfaceError::InvalidInput(format!(
                    "Invalid job record at {}:{}: {source}",
                    path.display(),
                    index + 1
                ))
            }))
        })
        .collect()
}

fn list_jobs(cli_args: &CliArgs, limit: usize) -> Result<Vec<Value>, ProductSurfaceError> {
    let records = read_job_records(cli_args)?;
    let mut seen = HashSet::new();
    let mut jobs = Vec::new();
    for record in records.into_iter().rev() {
        let Some(job_id) = record.get("job_id").and_then(Value::as_str) else {
            continue;
        };
        if seen.insert(job_id.to_string()) {
            jobs.push(record);
            if jobs.len() == limit {
                break;
            }
        }
    }
    Ok(jobs)
}

fn find_job(cli_args: &CliArgs, job_id: &str) -> Result<Value, ProductSurfaceError> {
    read_job_records(cli_args)?
        .into_iter()
        .rev()
        .find(|record| record.get("job_id").and_then(Value::as_str) == Some(job_id))
        .ok_or_else(|| ProductSurfaceError::InvalidInput(format!("Unknown job `{job_id}`")))
}

fn job_record(
    job_id: &str,
    kind: &str,
    status: &str,
    resume_command: &str,
    out: &Path,
    errors: &Path,
    checkpoint: &Path,
) -> Value {
    let now = Utc::now().to_rfc3339();
    json!({
        "job_id": job_id,
        "kind": kind,
        "status": status,
        "created_at": now,
        "updated_at": now,
        "resume_command": resume_command,
        "artifacts": {
            "out": out,
            "errors": errors,
            "checkpoint": checkpoint,
        },
    })
}

fn with_job_stats(mut record: Value, stats: Value) -> Value {
    if let Some(object) = record.as_object_mut() {
        object.insert("stats".to_string(), stats);
    }
    record
}

fn incident_export_stats(
    pages_fetched: u64,
    incidents_written: u64,
    errors_written: u64,
    retries_attempted: u64,
    log_warnings: u64,
) -> Value {
    json!({
        "pages_fetched": pages_fetched,
        "incidents_written": incidents_written,
        "errors_written": errors_written,
        "retries_attempted": retries_attempted,
        "log_warnings": log_warnings,
    })
}

fn incident_export_job_id(args: &ExportIncidentsArgs, checkpoint: &Path) -> String {
    let mut hasher = Sha256::new();
    hash_component(&mut hasher, "incident_export");
    hash_optional_component(&mut hasher, args.project_id.as_deref());
    hash_optional_component(&mut hasher, args.environment.as_deref());
    hash_component(&mut hasher, &args.page.to_string());
    hash_component(&mut hasher, &args.limit.to_string());
    hash_component(&mut hasher, &args.max_pages.to_string());
    hash_component(&mut hasher, &checkpoint.to_string_lossy());
    format!("incident-export-{:x}", hasher.finalize())
}

fn hash_optional_component(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hash_component(hasher, "some");
            hash_component(hasher, value);
        }
        None => hash_component(hasher, "none"),
    }
}

fn hash_component(hasher: &mut Sha256, value: &str) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    hasher.update(length.to_be_bytes());
    hasher.update(value.as_bytes());
}

fn incident_export_resume_command(
    args: &ExportIncidentsArgs,
    out: &Path,
    errors: &Path,
    checkpoint: &Path,
    output_mode: OutputMode,
) -> String {
    let mut parts = vec![
        "pcl".to_string(),
        "export".to_string(),
        "incidents".to_string(),
        "--resume".to_string(),
        "--out".to_string(),
        shell_word(out.display().to_string()),
        "--errors".to_string(),
        shell_word(errors.display().to_string()),
        "--checkpoint".to_string(),
        shell_word(checkpoint.display().to_string()),
        "--page".to_string(),
        args.page.to_string(),
        "--limit".to_string(),
        args.limit.to_string(),
        "--max-pages".to_string(),
        args.max_pages.to_string(),
        "--max-retries".to_string(),
        args.max_retries.to_string(),
        "--api-url".to_string(),
        shell_word(args.api_url.as_str()),
    ];

    if let Some(project_id) = &args.project_id {
        parts.push("--project-id".to_string());
        parts.push(shell_word(project_id));
    }
    if let Some(environment) = &args.environment {
        parts.push("--environment".to_string());
        parts.push(shell_word(environment));
    }
    if args.continue_on_error {
        parts.push("--continue-on-error".to_string());
    }
    if args.allow_unauthenticated {
        parts.push("--allow-unauthenticated".to_string());
    }
    match output_mode {
        OutputMode::Human => {}
        OutputMode::Json => parts.push("--json".to_string()),
    }

    parts.join(" ")
}

pub fn print_llms_guide(json_output: bool) -> Result<(), ProductSurfaceError> {
    print_output(
        &json!({
            "status": "ok",
            "data": llms_guide(),
            "next_actions": [
                "pcl doctor --json",
                "pcl api manifest --json",
                "pcl completions bash > ~/.local/share/bash-completion/completions/pcl",
                "pcl jobs list --json",
            ],
        }),
        json_output,
    )
}

fn llms_guide() -> Value {
    json!({
        "name": "pcl",
        "purpose": "CLI-native control surface for Credible Layer API investigation and assertion workflows.",
        "default_output": "human",
        "json_flag": "--json",
        "output_modes": {
            "default": "Human-readable output optimized for people using the CLI directly.",
            "json": "Use --json for machine-readable envelopes; preferred for agents and strict tooling."
        },
        "no_mcp_required": true,
        "principles": [
            "Use top-level workflow commands first.",
            "Use pcl api list/inspect/call/coverage only for debugging, API parity checks, or endpoints without workflow_alternatives.",
            "Treat every output as an envelope with status, data, error, and next_actions.",
            "Use JSONL export artifacts for long investigations.",
            "Use request IDs from errors and pcl requests for audit trails.",
            "Prefer CLI contracts over MCP, browser automation, or scraped help text."
        ],
        "consumption_order": [
            "pcl --json --llms",
            "pcl doctor --json",
            "pcl auth ensure --json",
            "pcl whoami --json",
            "pcl workflows --json",
            "pcl schema list --json",
            "pcl api manifest --json",
            "top-level workflow commands",
            "pcl api inspect <operation-id> --json when debugging",
            "pcl api call <method> <path> --json only after checking workflow_alternatives",
            "pcl api coverage --json"
        ],
        "orientation": [
            {
                "goal": "Check local readiness and auth truthfulness",
                "commands": ["pcl doctor --json", "pcl auth ensure --json", "pcl whoami --json", "pcl auth status --json"]
            },
            {
                "goal": "Discover available workflows",
                "commands": ["pcl workflows --json", "pcl schema list --json", "pcl api manifest --json"]
            },
            {
                "goal": "Debug raw API shape",
                "commands": ["pcl api list --filter incidents --json", "pcl api inspect <operation-id> --json"]
            },
            {
                "goal": "Run raw calls only for debugging or unsupported/internal endpoints",
                "commands": ["pcl api call get /health --allow-unauthenticated --json", "pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated --json"]
            },
            {
                "goal": "Export resumable incident data",
                "commands": ["pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --resume --json", "pcl jobs list --json"]
            },
            {
                "goal": "Ship a foundry project's assertions end-to-end (project, protocol manager, release, on-chain activation)",
                "commands": ["pcl deploy --dry-run --json", "pcl deploy --private-key <key> --rpc-url <url> --yes --json", "pcl config set-rpc <chain-id> <rpc-url>"]
            }
        ],
        "command_surfaces": {
            "workflows": ["pcl incidents", "pcl projects", "pcl assertions", "pcl account", "pcl contracts", "pcl releases", "pcl deployments", "pcl access", "pcl integrations", "pcl protocol-manager", "pcl events", "pcl search"],
            "discovery": ["pcl --json --llms", "pcl llms --json", "pcl workflows --json", "pcl schema --json", "pcl api manifest --json", "pcl api list --json", "pcl api inspect --json"],
            "execution": ["pcl api call", "pcl export incidents", "pcl apply", "pcl deploy"],
            "state": ["pcl artifacts", "pcl requests", "pcl jobs"],
            "shell": ["pcl completions bash", "pcl completions zsh", "pcl completions fish"]
        },
        "output_contract": {
            "default": "Human-readable output for people.",
            "json": "Pass --json for pretty JSON envelopes.",
            "jsonl_exceptions": {
                "pcl auth login --json": "Fresh login emits JSONL progress events and a final event with terminal=true. Already-authenticated login, including --no-wait, returns one auth-status envelope instead of a challenge. Use pcl auth ensure --json or pcl auth login --no-wait --force --json for normal agent flows when JSONL is not required."
            },
            "envelope_fields": ["status", "data", "error", "next_actions", "schema_version", "pcl_version"],
            "errors": "Parser, auth, config, validation, network, and API failures return structured envelopes and nonzero exit codes.",
            "error_fields": ["error.code", "error.message", "error.recoverable", "error.http.status", "error.request_id"],
            "long_running": "Export commands write JSONL artifacts, error files, checkpoints, and job records."
        },
        "auth_behavior": {
            "expiry_source": "Stored token expiry is normalized from the access-token JWT exp claim when available.",
            "ensure_command": "pcl auth ensure --json",
            "expires_soon": "true when five minutes or less remain; renew before long-running work.",
            "renew_command": "pcl auth ensure --force --json",
            "single_envelope_login": "pcl auth login --no-wait --force --json returns status=action_required with device_url, code, device_secret, expires_at, and poll_command.",
            "poll_command": "pcl auth poll --session-id <uuid> --device-secret <secret> --expires-at <rfc3339> --json",
            "refresh_command": "pcl auth refresh --json rotates the stored refresh token when available; if the refresh token is missing or rejected, it returns a login challenge.",
            "logout": "pcl auth logout attempts remote logout first, then clears local credentials; pass --local to skip the remote request."
        },
        "mutation_safety": {
            "order": ["--body-template", "typed flags", "--field key=value", "--body-file body.json"],
            "body_templates": "Print payload contracts before writes; choose a concrete body variant when body_variants is returned.",
            "execution": "Workflow commands execute when invoked; inspect body templates and use typed flags or body files deliberately."
        },
        "onchain": {
            "signing": "Commands that broadcast accept --private-key (env PCL_PRIVATE_KEY) or --account <foundry-keystore-name> (password via PCL_KEYSTORE_PASSWORD); pcl signs StateOracle transactions and EIP-191 challenges locally, calldata always comes from the API.",
            "rpc": "RPC endpoints resolve from --rpc-url / PCL_RPC_URL, then the per-chain map set with pcl config set-rpc <chain-id> <url> [--confirmations N].",
            "broadcast_flags": [
                "pcl releases calldata deploy <project> <release-id> --broadcast",
                "pcl releases calldata remove <project> <release-id> --broadcast",
                "pcl protocol-manager --project <ref> --set --sign --chain-id <id>",
                "pcl protocol-manager --project <ref> --transfer-calldata --new-manager 0x... --broadcast",
                "pcl protocol-manager --project <ref> --accept-calldata --broadcast"
            ],
            "orchestrator": "pcl deploy runs the full flow (resolve/create project, set protocol manager via signed challenge, build+verify assertions, create release, wait for checks, broadcast StateOracle.batch, confirm). Reruns resume: it observes state before each step. Machine output requires --yes.",
            "safety": "Human mode prompts before spending gas (--yes skips); machine mode treats --broadcast/--yes as consent. Tx hash is always surfaced even when the follow-up confirmation call fails; rerunning reconciles via the platform's noop path."
        },
        "raw_api": {
            "policy": "For normal product work, use workflow_alternatives from pcl api list/inspect or a top-level workflow command. Raw api call is for debugging, OpenAPI parity checks, internal/service endpoints, browser-session bridge investigation, or new endpoint exploration before promotion.",
            "inspect_first": "Use pcl api inspect <operation-id> --json before unfamiliar raw calls and check data.workflow_alternatives first.",
            "query_strings": "pcl api call accepts both /path?key=value and repeated --query key=value.",
            "fields": "pcl api call accepts repeated --field key=value for simple JSON object bodies; use --body-file for nested payloads.",
            "public_endpoints": "Known public raw calls do not attach stored tokens; --allow-unauthenticated remains the explicit opt-out for other public endpoints.",
            "pagination": "Use --paginate <array-field> --limit <n> --max-pages <n> and optionally --jsonl --output <file> for generic GET pagination.",
            "coverage": "Use pcl api coverage --json after exploration to find no-hit, hit-without-2xx, side-effecting-without-2xx, and unmatched request-log records."
        },
        "jobs_and_artifacts": {
            "export": "pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --checkpoint checkpoint.json --resume --continue-on-error --json",
            "inspect": ["pcl jobs list --json", "pcl jobs status <job-id> --json", "pcl jobs resume <job-id> --json", "pcl artifacts list --json"],
            "state_fields": ["job_id", "resume_command", "artifacts.out", "artifacts.errors", "artifacts.checkpoint"]
        },
        "provenance": {
            "preserve": ["request_id", "project_id", "incident_id", "transaction_hash", "trace_id", "artifact_path", "command"],
            "request_log": "pcl requests list --json"
        },
        "agent_files": {
            "repo_instructions": "AGENTS.md",
            "readme_section": "README.md#agent-consumption-guide"
        },
    })
}

fn list_artifacts(dir: &Path, limit: usize) -> Result<Vec<Value>, ProductSurfaceError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut entries: Vec<Value> = Vec::new();
    for entry in fs::read_dir(dir)
        .map_err(|source| {
            ProductSurfaceError::Io {
                path: dir.to_path_buf(),
                source,
            }
        })?
        .filter_map(Result::ok)
    {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        entries.push(json!({
            "path": entry.path(),
            "bytes": metadata.len(),
            "modified": metadata.modified().ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs()),
        }));
        sort_artifact_entries(&mut entries);
        entries.truncate(limit);
    }
    Ok(entries)
}

fn sort_artifact_entries(entries: &mut [Value]) {
    entries.sort_by(|left, right| {
        let left_modified = left.get("modified").and_then(Value::as_u64);
        let right_modified = right.get("modified").and_then(Value::as_u64);
        let left_path = left.get("path").map_or_else(String::new, Value::to_string);
        let right_path = right.get("path").map_or_else(String::new, Value::to_string);
        right_modified
            .cmp(&left_modified)
            .then_with(|| left_path.cmp(&right_path))
    });
}

fn auth_check_status(auth: Option<&UserAuth>) -> &'static str {
    match auth {
        None => "missing",
        Some(auth) if auth.expires_at <= Utc::now() => "warning",
        Some(_) => "ok",
    }
}

fn doctor_next_actions(status: &str, auth: Option<&UserAuth>) -> Vec<String> {
    if status == "error" {
        return vec![
            "Check --api-url or PCL_API_URL".to_string(),
            "pcl requests list --limit 20".to_string(),
            "pcl doctor --offline".to_string(),
        ];
    }

    match auth_check_status(auth) {
        "missing" => {
            vec![
                "pcl auth ensure".to_string(),
                "pcl auth login".to_string(),
                "pcl workflows".to_string(),
            ]
        }
        "warning" => {
            vec![
                "pcl auth ensure".to_string(),
                "pcl auth refresh".to_string(),
                "pcl auth login --force".to_string(),
            ]
        }
        _ => {
            vec![
                "pcl whoami".to_string(),
                "pcl workflows".to_string(),
                "pcl requests list --limit 20".to_string(),
            ]
        }
    }
}

fn auth_value(auth: Option<&UserAuth>) -> Value {
    let Some(auth) = auth else {
        return json!({
            "authenticated": false,
            "token_present": false,
            "token_valid": false,
            "token_expired": false,
            "expires_soon": false,
            "expired": false,
        });
    };
    let now = Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    let expired = auth.expires_at <= now;
    let expires_soon = !expired && seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS;
    json!({
        "authenticated": true,
        "user": auth.display_name(),
        "user_id": auth.user_id.map(|id| id.to_string()),
        "wallet_address": auth.wallet_address.map(|address| address.to_string()),
        "email": auth.email.as_deref(),
        "token_present": !auth.access_token.is_empty(),
        "token_valid": !expired,
        "token_expired": expired,
        "expires_soon": expires_soon,
        "expired": expired,
        "expires_at": auth.expires_at.to_rfc3339(),
        "seconds_remaining": seconds_remaining,
    })
}

fn generated_client(api_url: &url::Url) -> GeneratedClient {
    GeneratedClient::new(&generated_client_base_url(api_url))
}

fn generated_client_with_http_client(
    api_url: &url::Url,
    http_client: reqwest::Client,
) -> GeneratedClient {
    GeneratedClient::new_with_client(&generated_client_base_url(api_url), http_client)
}

fn generated_client_base_url(api_url: &url::Url) -> String {
    let mut base = api_url.clone();
    base.set_path("/api/v1");
    base.set_query(None);
    base.to_string()
}

fn generated_error_request_id<E>(error: &GeneratedError<E>) -> Option<String> {
    match error {
        GeneratedError::ErrorResponse(response) => request_id_from_headers(response.headers()),
        GeneratedError::UnexpectedResponse(response) => request_id_from_headers(response.headers()),
        _ => None,
    }
}

async fn generated_product_surface_error<E>(
    method: &'static str,
    path: &str,
    error: GeneratedError<E>,
) -> ProductSurfaceError
where
    E: serde::Serialize + std::fmt::Debug,
{
    match error {
        GeneratedError::ErrorResponse(response) => {
            let status = response.status().as_u16();
            let request_id = request_id_from_headers(response.headers());
            let body = serde_json::to_value(response.as_ref()).unwrap_or_else(|error| {
                json!({
                    "error": error.to_string(),
                    "body": format!("{response:?}"),
                })
            });
            ProductSurfaceError::HttpStatus {
                method,
                path: path.to_string(),
                status,
                request_id,
                body: Box::new(body),
            }
        }
        GeneratedError::UnexpectedResponse(response) => {
            let status = response.status().as_u16();
            let request_id = request_id_from_headers(response.headers());
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let body = match response.bytes().await {
                Ok(bytes) => response_body_value(&content_type, &bytes),
                Err(error) => {
                    return ProductSurfaceError::Request(error);
                }
            };
            ProductSurfaceError::HttpStatus {
                method,
                path: path.to_string(),
                status,
                request_id,
                body: Box::new(body),
            }
        }
        GeneratedError::CommunicationError(error)
        | GeneratedError::InvalidUpgrade(error)
        | GeneratedError::ResponseBodyError(error) => ProductSurfaceError::Request(error),
        GeneratedError::InvalidResponsePayload(bytes, error) => {
            ProductSurfaceError::InvalidInput(format!(
                "Invalid generated API response payload: {error}; body={}",
                String::from_utf8_lossy(&bytes)
            ))
        }
        GeneratedError::InvalidRequest(message) | GeneratedError::Custom(message) => {
            ProductSurfaceError::InvalidInput(message)
        }
    }
}

async fn resolve_export_project_id(
    client: &GeneratedClient,
    project_ref: &str,
) -> Result<Uuid, ProductSurfaceError> {
    if let Ok(project_id) = Uuid::parse_str(project_ref) {
        return Ok(project_id);
    }
    let path = generated_operation_path(
        "get_projects_resolve_project_ref",
        &[("project_ref", project_ref)],
    )
    .unwrap_or_else(|| "get_projects_resolve_project_ref".to_string());
    let response = match client.get_projects_resolve_project_ref(project_ref).await {
        Ok(response) => response.into_inner(),
        Err(error) => return Err(generated_product_surface_error("GET", &path, error).await),
    };
    Uuid::parse_str(&response.project_id).map_err(|error| {
        ProductSurfaceError::InvalidInput(format!(
            "Project reference `{project_ref}` resolved to invalid project_id `{}`: {error}",
            response.project_id
        ))
    })
}

fn export_incidents_operation_path(project_id: Option<&Uuid>) -> String {
    if let Some(project_id) = project_id {
        let project_id = project_id.to_string();
        return generated_operation_path(
            "get_views_projects_project_id_incidents",
            &[("projectId", project_id.as_str())],
        )
        .unwrap_or_else(|| "get_views_projects_project_id_incidents".to_string());
    }
    generated_operation_path("get_views_public_incidents", &[])
        .unwrap_or_else(|| "get_views_public_incidents".to_string())
}

async fn health_check(api_url: &url::Url) -> Value {
    let client = generated_client(api_url);
    match client.get_health().await {
        Ok(response) => {
            let status = response.status();
            json!({
                "name": "api_health",
                "status": if status.is_success() { "ok" } else { "error" },
                "http_status": status.as_u16(),
                "request_id": request_id_from_headers(response.headers()),
            })
        }
        Err(error) => {
            let status = error.status().map(|status| status.as_u16());
            json!({
                "name": "api_health",
                "status": "error",
                "http_status": status,
                "request_id": generated_error_request_id(&error),
                "error": error.to_string(),
            })
        }
    }
}

async fn auth_capability_check(api_url: &url::Url) -> Value {
    let client = generated_client(api_url);
    let response = match client.get_openapi().await {
        Ok(response) => response,
        Err(error) => {
            let http_status = error.status();
            let request_id = generated_error_request_id(&error);
            if let Some(http_status) = http_status {
                return json!({
                    "name": "auth_capabilities",
                    "status": "warning",
                    "http_status": http_status.as_u16(),
                    "request_id": request_id,
                    "error": "OpenAPI manifest could not be fetched, so CLI auth support could not be verified.",
                });
            }
            return json!({
                "name": "auth_capabilities",
                "status": "error",
                "error": error.to_string(),
            });
        }
    };
    let request_id = request_id_from_headers(response.headers());
    let spec = response.into_inner();
    let refresh_supported = openapi_has_operation(&spec, "/auth/refresh", "post");
    let login_supported = openapi_has_operation(&spec, "/cli/auth/code", "get")
        && openapi_has_operation(&spec, "/cli/auth/status", "get")
        && openapi_has_operation(&spec, "/cli/auth/verify", "post");
    let logout_revocation_supported = openapi_has_operation(&spec, "/web/auth/logout", "post")
        || openapi_has_operation(&spec, "/cli/auth/revoke", "post")
        || openapi_has_operation(&spec, "/auth/revoke", "post");
    let status = if refresh_supported && login_supported {
        "ok"
    } else {
        "warning"
    };
    json!({
        "name": "auth_capabilities",
        "status": status,
        "request_id": request_id,
        "refresh_supported": refresh_supported,
        "login_supported": login_supported,
        "logout_revocation_supported": logout_revocation_supported,
        "refresh_endpoint": "/api/v1/auth/refresh",
        "logout_endpoint": "/api/v1/web/auth/logout",
        "login_endpoints": {
            "code": "/api/v1/cli/auth/code",
            "status": "/api/v1/cli/auth/status",
            "verify": "/api/v1/cli/auth/verify",
        },
        "commands": [
            "pcl auth refresh --json",
            "pcl auth login --no-wait --json",
            "pcl auth status --json",
        ],
    })
}

fn openapi_has_operation(spec: &Value, path: &str, method: &str) -> bool {
    spec.get("paths")
        .and_then(Value::as_object)
        .and_then(|paths| paths.get(path))
        .and_then(|path_item| path_item.get(method))
        .is_some()
}

fn workflow_recipes() -> Vec<Value> {
    vec![
        json!({
            "name": "incident-investigation",
            "description": "Export incidents, inspect failing detail/trace records, and preserve request IDs.",
            "steps": [
                {"command": "pcl doctor --json", "output": "environment readiness"},
                {"command": "pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --resume --json", "output": "incident JSONL artifact"},
                {"command": "pcl incidents --incident-id <incident-id> --json", "output": "incident detail"},
                {"command": "pcl incidents --incident-id <incident-id> --tx-id <tx-id> --json", "output": "transaction trace"},
                {"command": "pcl requests list --limit 20 --json", "output": "API request IDs and status history"}
            ],
        }),
        json!({
            "name": "deploy-release",
            "description": "Create release payloads, preview, create, and fetch deploy calldata.",
            "steps": [
                {"command": "pcl releases preview <project-id> --body-template --json", "output": "release body contract"},
                {"command": "pcl releases preview <project-id> --body-file release.json --json", "output": "release preview"},
                {"command": "pcl releases create <project-id> --body-file release.json --json", "output": "created release"},
                {"command": "pcl releases calldata deploy <project-id> <release-id> --signer-address <address> --json", "output": "deployment calldata"}
            ],
        }),
        json!({
            "name": "invite-member",
            "description": "Invite a project member and inspect pending invitations.",
            "steps": [
                {"command": "pcl access invite <project-id> --body-template --json", "output": "invite body contract"},
                {"command": "pcl access invite <project-id> --body-file invite.json --json", "output": "invitation result"},
                {"command": "pcl access invitations <project-id> --json", "output": "project invitations"}
            ],
        }),
        json!({
            "name": "protocol-manager-transfer",
            "description": "Inspect manager state, produce transfer calldata, and confirm transfer variants.",
            "steps": [
                {"command": "pcl protocol-manager --project <project-id> --pending-transfer --json", "output": "pending transfer"},
                {"command": "pcl protocol-manager --project <project-id> --nonce --address <manager-address> --json", "output": "manager nonce"},
                {"command": "pcl protocol-manager --project <project-id> --transfer-calldata --new-manager <address> --json", "output": "transfer calldata"},
                {"command": "pcl protocol-manager --confirm-transfer --body-template --json", "output": "direct/onchain confirm variants"}
            ],
        }),
    ]
}

fn find_workflow_schema(commands: &[Value], workflow: &str) -> Result<Value, ProductSurfaceError> {
    commands
        .iter()
        .find(|command| {
            command["command"]
                .as_str()
                .is_some_and(|text| text.split_whitespace().nth(1) == Some(workflow))
        })
        .cloned()
        .ok_or_else(|| ProductSurfaceError::InvalidInput(format!("Unknown workflow `{workflow}`")))
}

fn export_plan(args: &ExportIncidentsArgs, out: &Path, errors: &Path, checkpoint: &Path) -> Value {
    json!({
        "export": "incidents",
        "project_id": args.project_id,
        "environment": args.environment,
        "out": out,
        "errors": errors,
        "checkpoint": checkpoint,
        "resume": args.resume,
        "continue_on_error": args.continue_on_error,
        "page": args.page,
        "limit": args.limit,
        "max_pages": args.max_pages,
        "max_retries": args.max_retries,
        "output_format": "jsonl",
    })
}

async fn ensure_export_auth(
    config: &mut CliConfig,
    cli_args: &CliArgs,
    api_url: &url::Url,
    require_auth: bool,
    allow_unauthenticated: bool,
) -> Result<(), ProductSurfaceError> {
    if allow_unauthenticated || !require_auth {
        return Ok(());
    }
    // Stored credentials may only ever reach the platform that issued them.
    // This guards both the refresh POST below and the bearer header attached
    // right after in `default_headers` — an export pointed at a foreign
    // --api-url/PCL_API_URL must not leak either token.
    crate::auth::ensure_credential_platform(config, api_url)
        .map_err(ProductSurfaceError::PlatformMismatch)?;
    let Some(auth) = &config.auth else {
        return Err(ProductSurfaceError::NoAuthToken);
    };
    let now = Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    if auth.expires_at <= now || seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS {
        refresh_stored_auth(config, api_url, cli_args, false)
            .await
            .map_err(ProductSurfaceError::AuthRefresh)?;
    }
    Ok(())
}

fn default_headers(
    config: &CliConfig,
    require_auth: bool,
    allow_unauthenticated: bool,
) -> Result<HeaderMap, ProductSurfaceError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("api-version"),
        HeaderValue::from_static("1"),
    );

    if require_auth && !allow_unauthenticated {
        let Some(auth) = &config.auth else {
            return Err(ProductSurfaceError::NoAuthToken);
        };
        if auth.expires_at <= Utc::now() {
            return Err(ProductSurfaceError::ExpiredAuthToken(auth.expires_at));
        }
        let value =
            HeaderValue::from_str(&format!("Bearer {}", auth.access_token)).map_err(|_| {
                ProductSurfaceError::InvalidInput(
                    "Stored auth token is not a valid header".to_string(),
                )
            })?;
        headers.insert(reqwest::header::AUTHORIZATION, value);
    }
    Ok(headers)
}

async fn fetch_export_page(
    client: &GeneratedClient,
    project_id: Option<&Uuid>,
    environment: Option<GetViewsProjectsProjectIdIncidentsEnvironment>,
    page: NonZeroU64,
    limit: NonZeroU64,
    max_retries: u64,
) -> Result<ExportPageResponse, ExportPageError> {
    let max_attempts = max_retries.saturating_add(1);
    for attempt in 1..=max_attempts {
        let response = if let Some(project_id) = project_id {
            generated_export_page_response(
                client
                    .get_views_projects_project_id_incidents(
                        project_id,
                        None,
                        None,
                        environment,
                        None,
                        Some(limit),
                        Some(page),
                        None,
                    )
                    .await,
                attempt,
            )
            .await
        } else {
            generated_export_page_response(
                client
                    .get_views_public_incidents(None, Some(limit), None, Some(page), None)
                    .await,
                attempt,
            )
            .await
        };
        match response {
            Ok(response) => {
                if response.status.is_success()
                    || !should_retry_export_status(response.status.as_u16())
                    || attempt == max_attempts
                {
                    return Ok(response);
                }
            }
            Err(ExportPageError::Request { source, .. }) => {
                if attempt == max_attempts || !should_retry_export_error(&source) {
                    return Err(ExportPageError::Request {
                        attempts: attempt,
                        source,
                    });
                }
            }
            Err(error) => return Err(error),
        }
        sleep(export_retry_delay(attempt)).await;
    }

    unreachable!("export retry loop must return a response or error")
}

async fn generated_export_page_response<T>(
    result: Result<ResponseValue<T>, GeneratedError<GeneratedApiErrorBody>>,
    attempts: u64,
) -> Result<ExportPageResponse, ExportPageError>
where
    T: serde::Serialize + std::fmt::Debug,
{
    match result {
        Ok(response) => {
            let status = response.status();
            let request_id = request_id_from_headers(response.headers());
            let body = serde_json::to_value(response.as_ref())
                .map_err(|source| ExportPageError::Serialization { attempts, source })?;
            Ok(ExportPageResponse {
                status,
                request_id,
                body,
                attempts,
            })
        }
        Err(GeneratedError::ErrorResponse(response)) => {
            let status = response.status();
            let request_id = request_id_from_headers(response.headers());
            let body = serde_json::to_value(response.as_ref())
                .map_err(|source| ExportPageError::Serialization { attempts, source })?;
            Ok(ExportPageResponse {
                status,
                request_id,
                body,
                attempts,
            })
        }
        Err(GeneratedError::UnexpectedResponse(response)) => {
            let status = response.status();
            let request_id = request_id_from_headers(response.headers());
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let bytes = response
                .bytes()
                .await
                .map_err(|source| ExportPageError::Request { attempts, source })?;
            Ok(ExportPageResponse {
                status,
                request_id,
                body: response_body_value(&content_type, &bytes),
                attempts,
            })
        }
        Err(
            GeneratedError::CommunicationError(source)
            | GeneratedError::InvalidUpgrade(source)
            | GeneratedError::ResponseBodyError(source),
        ) => Err(ExportPageError::Request { attempts, source }),
        Err(GeneratedError::InvalidResponsePayload(bytes, error)) => {
            Err(ExportPageError::Generated {
                attempts,
                message: format!(
                    "Invalid generated API response payload: {error}; body={}",
                    String::from_utf8_lossy(&bytes)
                ),
            })
        }
        Err(GeneratedError::InvalidRequest(message) | GeneratedError::Custom(message)) => {
            Err(ExportPageError::Generated { attempts, message })
        }
    }
}

fn should_retry_export_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

fn should_retry_export_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout()
}

fn export_retry_delay(attempt: u64) -> Duration {
    let exponent = attempt.saturating_sub(1).min(5);
    let shift = u32::try_from(exponent).unwrap_or(5);
    let multiplier = 1_u64.checked_shl(shift).unwrap_or(32);
    Duration::from_millis(250_u64.saturating_mul(multiplier))
}

fn extract_items(body: &Value, field: &str) -> Vec<Value> {
    body.get(field)
        .or_else(|| body.pointer(&format!("/data/{field}")))
        .or_else(|| body.get("items"))
        .or_else(|| body.pointer("/data/items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn ensure_parent_dir(path: &Path) -> Result<(), ProductSurfaceError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            ProductSurfaceError::Io {
                path: parent.to_path_buf(),
                source,
            }
        })?;
    }
    Ok(())
}

fn write_jsonl(writer: &mut BufWriter<fs::File>, value: &Value) -> Result<(), ProductSurfaceError> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n").map_err(|source| {
        ProductSurfaceError::Io {
            path: PathBuf::from("<artifact>"),
            source,
        }
    })
}

fn flush_jsonl_writer(
    writer: &mut BufWriter<fs::File>,
    path: &Path,
) -> Result<(), ProductSurfaceError> {
    writer.flush().map_err(|source| {
        ProductSurfaceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    writer.get_ref().sync_all().map_err(|source| {
        ProductSurfaceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn write_checkpoint(
    path: &Path,
    next_page: u64,
    items_written: u64,
) -> Result<(), ProductSurfaceError> {
    let mut file = fs::File::create(path).map_err(|source| {
        ProductSurfaceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    file.write_all(&serde_json::to_vec_pretty(&json!({
        "next_page": next_page,
        "items_written": items_written,
        "updated_at": Utc::now().to_rfc3339(),
    }))?)
    .and_then(|()| file.sync_all())
    .map_err(|source| {
        ProductSurfaceError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn read_checkpoint_page(path: &Path) -> Option<u64> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .get("next_page")?
        .as_u64()
}

fn log_request(
    cli_args: &CliArgs,
    kind: &str,
    method: &str,
    path: &str,
    status: u16,
    request_id: Option<&str>,
) -> bool {
    let request_log_path = request_log::request_log_path_for_args(cli_args);
    request_log::append_request_record_at(
        &request_log_path,
        &json!({
            "timestamp": Utc::now().to_rfc3339(),
            "kind": kind,
            "method": method,
            "path": path,
            "status": status,
            "success": (200..=299).contains(&status),
            "request_id": request_id,
        }),
    )
    .is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DEFAULT_PLATFORM_URL;
    use mockito::Matcher;
    use pcl_common::args::CliArgs;
    use std::{
        collections::BTreeMap,
        net::TcpListener,
    };
    use tempfile::tempdir;

    fn public_incidents_body(ids: &[&str], page: u64, limit: u64) -> String {
        json!({
            "data": {
                "items": ids.iter().map(|id| json!({
                    "id": id,
                    "network": {
                        "chainId": 1,
                        "name": "Testnet",
                    },
                    "referenceId": format!("ref-{id}"),
                    "timestamp": "2026-01-01T00:00:00Z",
                    "title": format!("Incident {id}"),
                })).collect::<Vec<_>>(),
                "pagination": {
                    "hasMore": false,
                    "limit": limit,
                    "page": page,
                    "total": ids.len(),
                },
            },
            "_meta": {
                "fetchedAt": "2026-01-01T00:00:00Z",
                "sources": ["offchain"],
            },
        })
        .to_string()
    }

    fn project_incidents_body(ids: &[&str], page: u64, limit: u64) -> String {
        json!({
            "data": {
                "items": ids.iter().map(|id| json!({
                    "assertionAdopterId": "adopter-1",
                    "assertionId": "assertion-1",
                    "assertionTitle": null,
                    "chainId": 1,
                    "contractAddress": "0x0000000000000000000000000000000000000001",
                    "contractName": null,
                    "createdAt": "2026-01-01T00:00:00Z",
                    "environment": "production",
                    "incidentId": id,
                    "tracesCompleted": 0,
                    "tracesPending": 0,
                    "transactionCount": 1,
                    "windowStart": "2026-01-01T00:00:00Z",
                })).collect::<Vec<_>>(),
                "pagination": {
                    "hasNext": false,
                    "hasPrev": false,
                    "limit": limit,
                    "page": page,
                    "total": ids.len(),
                    "totalPages": 1,
                },
            },
            "_meta": {
                "fetchedAt": "2026-01-01T00:00:00Z",
                "sources": ["offchain"],
            },
        })
        .to_string()
    }

    #[test]
    fn workflows_can_show_one_recipe() {
        let args = WorkflowsArgs {
            command: Some(WorkflowCommand::Show {
                name: "incident-investigation".to_string(),
            }),
        };
        assert!(args.run(true).is_ok());
    }

    #[test]
    fn whoami_errors_on_expired_auth() {
        let args = WhoamiArgs { offline: false };
        let config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                access_token: "expired-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
            platform_url: None,
        };

        let error = args.run(&config, true).unwrap_err();
        assert!(matches!(error, ProductSurfaceError::ExpiredAuthToken(_)));
    }

    #[test]
    fn expired_auth_next_actions_prefer_refresh() {
        let error = ProductSurfaceError::ExpiredAuthToken(
            chrono::Utc::now() - chrono::Duration::minutes(1),
        );

        assert_eq!(
            error.next_actions(),
            vec![
                "pcl auth refresh".to_string(),
                "pcl auth login --force".to_string(),
                "pcl doctor".to_string(),
            ]
        );
    }

    #[test]
    fn surface_error_envelope_keeps_recoverable_inside_error_object() {
        let envelope = ProductSurfaceError::InvalidInput("bad input".to_string()).json_envelope();

        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["error"]["recoverable"], true);
        assert!(envelope.get("recoverable").is_none(), "{envelope}");
    }

    #[test]
    fn doctor_next_actions_recover_missing_or_expired_auth() {
        assert_eq!(
            doctor_next_actions("warning", None),
            vec![
                "pcl auth ensure".to_string(),
                "pcl auth login".to_string(),
                "pcl workflows".to_string(),
            ]
        );

        let auth = UserAuth {
            access_token: "expired-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
            refresh_expires_at: None,
            user_id: None,
            wallet_address: None,
            email: Some("agent@example.com".to_string()),
        };
        assert_eq!(
            doctor_next_actions("warning", Some(&auth)),
            vec![
                "pcl auth ensure".to_string(),
                "pcl auth refresh".to_string(),
                "pcl auth login --force".to_string(),
            ]
        );
    }

    #[test]
    fn schema_finds_action_contract() {
        let commands = api_manifest()["commands"].as_array().cloned().unwrap();
        let schema = find_workflow_schema(&commands, "incidents").unwrap();
        assert!(schema["actions"].as_array().unwrap().iter().any(|action| {
            action["name"] == "list_public" && action["example"] == "pcl incidents --limit 5 --json"
        }));
    }

    #[test]
    fn artifact_dir_respects_config_dir() {
        let temp = tempdir().unwrap();
        let args = CliArgs {
            config_dir: Some(temp.path().to_path_buf()),
            ..Default::default()
        };
        assert_eq!(artifact_dir(&args), temp.path().join("artifacts"));
    }

    #[test]
    fn llms_guide_advertises_cli_native_surfaces() {
        let guide = llms_guide();

        assert_eq!(guide["default_output"], "human");
        assert_eq!(guide["json_flag"], "--json");
        assert_eq!(guide["no_mcp_required"], true);
        assert_eq!(guide["agent_files"]["repo_instructions"], "AGENTS.md");
        assert!(
            guide["command_surfaces"]["discovery"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command == "pcl --json --llms")
        );
        assert!(
            guide["consumption_order"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command == "pcl api manifest --json")
        );
        let consumption_order = guide["consumption_order"].as_array().unwrap();
        let auth_ensure_position = consumption_order
            .iter()
            .position(|command| command == "pcl auth ensure --json")
            .expect("auth ensure in consumption order");
        let whoami_position = consumption_order
            .iter()
            .position(|command| command == "pcl whoami --json")
            .expect("whoami in consumption order");
        assert!(auth_ensure_position < whoami_position);
        assert!(
            guide["command_surfaces"]["state"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command == "pcl jobs")
        );
        assert!(
            guide["mutation_safety"]["order"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step == "--body-template")
        );
        assert!(
            !guide["mutation_safety"]["order"]
                .as_array()
                .unwrap()
                .iter()
                .any(|step| step == "--dry-run")
        );
    }

    #[test]
    fn jobs_are_stored_as_latest_jsonl_records() {
        let temp = tempdir().unwrap();
        let args = CliArgs {
            config_dir: Some(temp.path().to_path_buf()),
            ..Default::default()
        };
        let out = temp.path().join("incidents.jsonl");
        let errors = temp.path().join("errors.jsonl");
        let checkpoint = temp.path().join("checkpoint.json");

        append_job_record(
            &args,
            &job_record(
                "incident-export-test",
                "incident_export",
                "running",
                "pcl export incidents --resume",
                &out,
                &errors,
                &checkpoint,
            ),
        )
        .unwrap();
        append_job_record(
            &args,
            &job_record(
                "incident-export-test",
                "incident_export",
                "completed",
                "pcl export incidents --resume",
                &out,
                &errors,
                &checkpoint,
            ),
        )
        .unwrap();

        let jobs = list_jobs(&args, 20).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0]["job_id"], "incident-export-test");
        assert_eq!(jobs[0]["status"], "completed");
        assert_eq!(
            find_job(&args, "incident-export-test").unwrap()["status"],
            "completed"
        );
    }

    #[test]
    fn incident_export_resume_command_quotes_paths() {
        let args = ExportIncidentsArgs {
            project_id: Some("project one".to_string()),
            environment: Some("production".to_string()),
            page: 1,
            limit: 50,
            max_pages: 10,
            out: None,
            errors: None,
            checkpoint: None,
            resume: false,
            continue_on_error: true,
            max_retries: 3,
            dry_run: false,
            api_url: DEFAULT_PLATFORM_URL.parse().unwrap(),
            allow_unauthenticated: false,
        };

        let command = incident_export_resume_command(
            &args,
            Path::new("/tmp/pcl artifacts/incidents.jsonl"),
            Path::new("/tmp/pcl artifacts/errors.jsonl"),
            Path::new("/tmp/pcl artifacts/checkpoint.json"),
            OutputMode::Human,
        );

        assert!(command.contains("--resume"));
        assert!(command.contains("'project one'"));
        assert!(command.contains("'/tmp/pcl artifacts/incidents.jsonl'"));
        assert!(command.contains("--continue-on-error"));
        assert!(!command.contains("--json"));
        assert!(
            incident_export_resume_command(
                &args,
                Path::new("/tmp/pcl artifacts/incidents.jsonl"),
                Path::new("/tmp/pcl artifacts/errors.jsonl"),
                Path::new("/tmp/pcl artifacts/checkpoint.json"),
                OutputMode::Json,
            )
            .contains("--json")
        );
    }

    #[tokio::test]
    async fn incident_export_retries_transient_page_failures() {
        let mut server = mockito::Server::new_async().await;
        let query = Matcher::AllOf(vec![
            Matcher::UrlEncoded("page".into(), "1".into()),
            Matcher::UrlEncoded("limit".into(), "50".into()),
        ]);
        let transient = server
            .mock("GET", "/api/v1/views/public/incidents")
            .match_query(query.clone())
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"temporary"}"#)
            .expect(1)
            .create_async()
            .await;
        let success = server
            .mock("GET", "/api/v1/views/public/incidents")
            .match_query(query)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(public_incidents_body(&["i1"], 1, 50))
            .expect(1)
            .create_async()
            .await;
        let temp = tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp.path().join("config")),
            ..Default::default()
        };
        let out = temp.path().join("incidents.jsonl");
        let errors = temp.path().join("errors.jsonl");
        let checkpoint = temp.path().join("checkpoint.json");
        let args = ExportIncidentsArgs {
            project_id: None,
            environment: None,
            page: 1,
            limit: 50,
            max_pages: 1,
            out: Some(out.clone()),
            errors: Some(errors.clone()),
            checkpoint: Some(checkpoint),
            resume: false,
            continue_on_error: false,
            max_retries: 1,
            dry_run: false,
            api_url: server.url().parse().unwrap(),
            allow_unauthenticated: true,
        };

        let mut config = CliConfig::default();
        export_incidents(&args, &mut config, &cli_args, true)
            .await
            .unwrap();

        transient.assert_async().await;
        success.assert_async().await;
        let lines = fs::read_to_string(out).unwrap();
        assert!(lines.contains(r#""id":"i1""#));
        assert_eq!(fs::read_to_string(errors).unwrap(), "");
    }

    #[tokio::test]
    async fn incident_export_refreshes_expired_project_auth_before_request() {
        let mut server = mockito::Server::new_async().await;
        let project_id = "11111111-1111-4111-8111-111111111111";
        let refresh = server
            .mock("POST", "/api/v1/auth/refresh")
            .match_header("authorization", Matcher::Missing)
            .match_body(Matcher::Json(json!({ "refresh_token": "old_refresh" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"token":"new_access","refresh_token":"new_refresh","expires_at":"2030-01-01T00:00:00Z","refresh_expires_at":"2030-02-01T00:00:00Z"}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let query = Matcher::AllOf(vec![
            Matcher::UrlEncoded("page".into(), "1".into()),
            Matcher::UrlEncoded("limit".into(), "50".into()),
        ]);
        let export = server
            .mock(
                "GET",
                format!("/api/v1/views/projects/{project_id}/incidents").as_str(),
            )
            .match_header("authorization", "Bearer new_access")
            .match_query(query)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(project_incidents_body(&["i1"], 1, 50))
            .expect(1)
            .create_async()
            .await;

        let temp = tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp.path().join("config")),
            ..Default::default()
        };
        let mut config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                access_token: "old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
            platform_url: Some(server.url()),
        };
        config.write_to_file(&cli_args).unwrap();
        let out = temp.path().join("incidents.jsonl");
        let errors = temp.path().join("errors.jsonl");
        let checkpoint = temp.path().join("checkpoint.json");
        let args = ExportIncidentsArgs {
            project_id: Some(project_id.to_string()),
            environment: None,
            page: 1,
            limit: 50,
            max_pages: 1,
            out: Some(out),
            errors: Some(errors),
            checkpoint: Some(checkpoint),
            resume: false,
            continue_on_error: false,
            max_retries: 0,
            dry_run: false,
            api_url: server.url().parse().unwrap(),
            allow_unauthenticated: false,
        };

        export_incidents(&args, &mut config, &cli_args, true)
            .await
            .unwrap();

        refresh.assert_async().await;
        export.assert_async().await;
        let auth = config.auth.as_ref().expect("refreshed auth");
        assert_eq!(auth.access_token, "new_access");
        assert_eq!(auth.refresh_token, "new_refresh");
    }

    #[tokio::test]
    async fn incident_export_refuses_a_foreign_platform_with_a_valid_token() {
        // Valid production-issued credentials (no remembered platform), export
        // pointed at a different --api-url: neither the bearer token nor any
        // request may reach that platform.
        let mut server = mockito::Server::new_async().await;
        let no_requests = server
            .mock("GET", Matcher::Any)
            .expect(0)
            .create_async()
            .await;

        let temp = tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp.path().join("config")),
            ..Default::default()
        };
        let mut config = CliConfig {
            auth: Some(UserAuth {
                access_token: "valid_access".to_string(),
                refresh_token: "valid_refresh".to_string(),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(2),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
            platform_url: None,
        };
        let args = ExportIncidentsArgs {
            project_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            environment: None,
            page: 1,
            limit: 50,
            max_pages: 1,
            out: Some(temp.path().join("incidents.jsonl")),
            errors: Some(temp.path().join("errors.jsonl")),
            checkpoint: Some(temp.path().join("checkpoint.json")),
            resume: false,
            continue_on_error: false,
            max_retries: 0,
            dry_run: false,
            api_url: server.url().parse().unwrap(),
            allow_unauthenticated: false,
        };

        let error = export_incidents(&args, &mut config, &cli_args, true)
            .await
            .unwrap_err();

        assert!(matches!(error, ProductSurfaceError::PlatformMismatch(_)));
        assert_eq!(error.code(), "auth.platform_mismatch");
        no_requests.assert_async().await;
    }

    #[tokio::test]
    async fn incident_export_refuses_to_refresh_against_a_foreign_platform() {
        // Expiring production-issued credentials: the pre-request refresh must
        // not post the stored refresh token to a foreign --api-url either.
        let mut server = mockito::Server::new_async().await;
        let no_requests = server
            .mock("POST", Matcher::Any)
            .expect(0)
            .create_async()
            .await;

        let temp = tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp.path().join("config")),
            ..Default::default()
        };
        let mut config = CliConfig {
            auth: Some(UserAuth {
                access_token: "old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: chrono::Utc::now() - chrono::Duration::minutes(1),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
            platform_url: None,
        };
        let args = ExportIncidentsArgs {
            project_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            environment: None,
            page: 1,
            limit: 50,
            max_pages: 1,
            out: Some(temp.path().join("incidents.jsonl")),
            errors: Some(temp.path().join("errors.jsonl")),
            checkpoint: Some(temp.path().join("checkpoint.json")),
            resume: false,
            continue_on_error: false,
            max_retries: 0,
            dry_run: false,
            api_url: server.url().parse().unwrap(),
            allow_unauthenticated: false,
        };

        let error = export_incidents(&args, &mut config, &cli_args, true)
            .await
            .unwrap_err();

        assert!(matches!(error, ProductSurfaceError::PlatformMismatch(_)));
        no_requests.assert_async().await;
    }

    #[tokio::test]
    async fn incident_export_records_failed_job_after_network_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        drop(listener);

        let temp = tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp.path().join("config")),
            ..Default::default()
        };
        let out = temp.path().join("incidents.jsonl");
        let errors = temp.path().join("errors.jsonl");
        let checkpoint = temp.path().join("checkpoint.json");
        let args = ExportIncidentsArgs {
            project_id: None,
            environment: None,
            page: 1,
            limit: 50,
            max_pages: 1,
            out: Some(out.clone()),
            errors: Some(errors.clone()),
            checkpoint: Some(checkpoint.clone()),
            resume: false,
            continue_on_error: false,
            max_retries: 0,
            dry_run: false,
            api_url,
            allow_unauthenticated: true,
        };

        let mut config = CliConfig::default();
        let error = export_incidents(&args, &mut config, &cli_args, true)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ProductSurfaceError::ExportRequest {
                page: 1,
                attempts: 1,
                ..
            }
        ));

        let error_lines = fs::read_to_string(errors).unwrap();
        assert!(error_lines.contains(r#""code":"network.request_failed""#));
        assert!(error_lines.contains(r#""attempts":1"#));

        let job_id = incident_export_job_id(&args, &checkpoint);
        let job = find_job(&cli_args, &job_id).unwrap();
        assert_eq!(job["status"], "failed");
        assert_eq!(job["stats"]["pages_fetched"], 0);
        assert_eq!(job["stats"]["incidents_written"], 0);
        assert_eq!(job["stats"]["errors_written"], 1);
        assert_eq!(job["stats"]["retries_attempted"], 0);
    }

    #[tokio::test]
    async fn incident_export_marks_continue_on_error_jobs_partial() {
        let mut server = mockito::Server::new_async().await;
        let query = Matcher::AllOf(vec![
            Matcher::UrlEncoded("page".into(), "1".into()),
            Matcher::UrlEncoded("limit".into(), "50".into()),
        ]);
        let failure = server
            .mock("GET", "/api/v1/views/public/incidents")
            .match_query(query)
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "req_export_partial")
            .with_body(r#"{"error":"temporary"}"#)
            .expect(1)
            .create_async()
            .await;

        let temp = tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp.path().join("config")),
            ..Default::default()
        };
        let out = temp.path().join("incidents.jsonl");
        let errors = temp.path().join("errors.jsonl");
        let checkpoint = temp.path().join("checkpoint.json");
        let args = ExportIncidentsArgs {
            project_id: None,
            environment: None,
            page: 1,
            limit: 50,
            max_pages: 1,
            out: Some(out),
            errors: Some(errors.clone()),
            checkpoint: Some(checkpoint.clone()),
            resume: false,
            continue_on_error: true,
            max_retries: 0,
            dry_run: false,
            api_url: server.url().parse().unwrap(),
            allow_unauthenticated: true,
        };

        let mut config = CliConfig::default();
        export_incidents(&args, &mut config, &cli_args, true)
            .await
            .unwrap();

        failure.assert_async().await;
        let error_lines = fs::read_to_string(errors).unwrap();
        assert!(error_lines.contains(r#""status":500"#));
        assert!(error_lines.contains("req_export_partial"));

        let job_id = incident_export_job_id(&args, &checkpoint);
        let job = find_job(&cli_args, &job_id).unwrap();
        assert_eq!(job["status"], "completed_with_errors");
        assert_eq!(job["stats"]["pages_fetched"], 0);
        assert_eq!(job["stats"]["incidents_written"], 0);
        assert_eq!(job["stats"]["errors_written"], 1);
        assert_eq!(job["stats"]["retries_attempted"], 0);
    }
}
