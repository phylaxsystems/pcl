#![allow(
    clippy::format_push_string,
    clippy::map_unwrap_or,
    clippy::missing_errors_doc,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]

use crate::{
    DEFAULT_PLATFORM_URL,
    api::{
        api_manifest,
        toon_string,
        with_envelope_metadata,
    },
    config::{
        CliConfig,
        UserAuth,
    },
    request_log,
};
use chrono::Utc;
use pcl_common::args::CliArgs;
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
use std::{
    collections::{
        HashSet,
        hash_map::DefaultHasher,
    },
    fs,
    hash::{
        Hash,
        Hasher,
    },
    io::{
        BufRead,
        BufReader,
        BufWriter,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
};

const ARTIFACT_DIR_ENV: &str = "PCL_ARTIFACT_DIR";
const JOBS_FILE_ENV: &str = "PCL_JOBS_FILE";

#[derive(Debug, thiserror::Error)]
pub enum ProductSurfaceError {
    #[error("Run `pcl auth login` first")]
    NoAuthToken,

    #[error("Stored auth token expired at {0}")]
    ExpiredAuthToken(chrono::DateTime<chrono::Utc>),

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
            Self::InvalidInput(_) => "input.invalid",
            Self::Io { .. } => "io.failed",
            Self::Json(_) => "json.failed",
            Self::Request(_) => "network.request_failed",
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
            "recoverable": self.recoverable(),
            "next_actions": self.next_actions(),
        }))
    }

    fn recoverable(&self) -> bool {
        !matches!(self, Self::Json(_))
    }

    fn next_actions(&self) -> Vec<String> {
        match self {
            Self::NoAuthToken | Self::ExpiredAuthToken(_) => {
                vec!["pcl auth login".to_string(), "pcl doctor".to_string()]
            }
            Self::InvalidInput(_) => {
                vec!["pcl workflows".to_string(), "pcl schema list".to_string()]
            }
            Self::Io { .. } => vec!["pcl artifacts path".to_string()],
            Self::Json(_) => vec!["Retry with --json to inspect the envelope".to_string()],
            Self::Request(_) => vec!["pcl doctor".to_string(), "Check --api-url".to_string()],
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
        default_value = DEFAULT_PLATFORM_URL,
        help = "Base URL for the platform API"
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
    #[arg(
        long,
        value_delimiter = ',',
        help = "Requested related data to include"
    )]
    include: Vec<String>,
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
        help = "Reserved retry budget in export metadata"
    )]
    max_retries: u64,
    #[arg(long, help = "Print the export plan without fetching data")]
    dry_run: bool,
    #[arg(
        long = "api-url",
        env = "PCL_API_URL",
        default_value = DEFAULT_PLATFORM_URL,
        help = "Base URL for the platform API"
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
                "path": request_log::request_log_path().display().to_string(),
            }),
            json!({
                "name": "artifacts",
                "status": "ok",
                "path": artifact_dir(cli_args).display().to_string(),
            }),
        ];

        if !self.offline {
            checks.push(health_check(&self.api_url).await);
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

        print_output(
            &json!({
                "status": status,
                "data": {
                    "checks": checks,
                    "default_output": "toon",
                    "json_output_flag": "--json",
                    "api_url": self.api_url.as_str(),
                },
                "next_actions": [
                    "pcl whoami",
                    "pcl workflows",
                    "pcl requests list --limit 20",
                ],
            }),
            json_output,
        )
    }
}

impl WhoamiArgs {
    pub fn run(&self, config: &CliConfig, json_output: bool) -> Result<(), ProductSurfaceError> {
        print_output(
            &json!({
                "status": "ok",
                "data": {
                    "offline": self.offline,
                    "auth": auth_value(config.auth.as_ref()),
                },
                "next_actions": if config.auth.is_some() {
                    json!(["pcl account", "pcl projects --home", "pcl doctor"])
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
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": ["pcl export incidents --help", "pcl artifacts path"],
            }),
            json_output,
        )
    }
}

impl RequestsArgs {
    pub fn run(&self, json_output: bool) -> Result<(), ProductSurfaceError> {
        let path = request_log::request_log_path();
        let data = match &self.command {
            Some(RequestsCommand::Path) => json!({ "request_log": path }),
            Some(RequestsCommand::Clear) => {
                let deleted = request_log::clear_request_log().map_err(|source| {
                    ProductSurfaceError::Io {
                        path: path.clone(),
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
                let records = request_log::read_request_records(limit).map_err(|source| {
                    ProductSurfaceError::Io {
                        path: path.clone(),
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
                        let command_text = command["command"].as_str()?;
                        let workflow = command_text.split_whitespace().nth(1)?;
                        Some(json!({
                            "workflow": workflow,
                            "command": command_text,
                            "description": command["description"],
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
        print_output(
            &json!({
                "status": "ok",
                "data": data,
                "next_actions": [
                    "pcl jobs list",
                    "pcl jobs resume <job-id>",
                    "pcl export incidents --help",
                ],
            }),
            json_output,
        )
    }
}

impl ExportArgs {
    pub async fn run(
        &self,
        config: &CliConfig,
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
    config: &CliConfig,
    cli_args: &CliArgs,
    json_output: bool,
) -> Result<(), ProductSurfaceError> {
    if args.limit == 0 {
        return Err(ProductSurfaceError::InvalidInput(
            "--limit must be greater than zero".to_string(),
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
    let resume_command = incident_export_resume_command(args, &out, &errors, &checkpoint);

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

    let start_page = if args.resume {
        read_checkpoint_page(&checkpoint).unwrap_or(args.page)
    } else {
        args.page
    };
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

    let client = reqwest::Client::builder()
        .default_headers(default_headers(
            config,
            args.project_id.is_some(),
            args.allow_unauthenticated,
        )?)
        .build()?;
    let mut pages_fetched = 0_u64;
    let mut incidents_written = 0_u64;
    let mut errors_written = 0_u64;
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
        let page = start_page + offset;
        let path = args.project_id.as_ref().map_or_else(
            || "/views/public/incidents".to_string(),
            |project_id| format!("/views/projects/{project_id}/incidents"),
        );
        let url = build_api_url(&args.api_url, &path)?;
        let mut query = vec![
            ("page".to_string(), page.to_string()),
            ("limit".to_string(), args.limit.to_string()),
        ];
        if let Some(environment) = &args.environment {
            query.push(("environment".to_string(), environment.clone()));
        }

        let response = client.get(url).query(&query).send().await?;
        let status = response.status();
        let request_id = request_id_from_headers(response.headers());
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.bytes().await?;
        let body = response_body_value(&content_type, &bytes);
        log_request(
            "export",
            "GET",
            &path,
            status.as_u16(),
            request_id.as_deref(),
        );

        if !status.is_success() {
            errors_written += 1;
            write_jsonl(
                &mut error_file,
                &json!({
                    "page": page,
                    "path": path,
                    "status": status.as_u16(),
                    "request_id": request_id,
                    "body": body,
                }),
            )?;
            if args.continue_on_error {
                continue;
            }
            append_job_record(
                cli_args,
                &job_record(
                    &job_id,
                    "incident_export",
                    "failed",
                    &resume_command,
                    &out,
                    &errors,
                    &checkpoint,
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
        write_checkpoint(&checkpoint, page + 1, incidents_written)?;
        if page_count < usize::try_from(args.limit).unwrap_or(usize::MAX) {
            break;
        }
    }
    append_job_record(
        cli_args,
        &job_record(
            &job_id,
            "incident_export",
            "completed",
            &resume_command,
            &out,
            &errors,
            &checkpoint,
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
                "include_requested": args.include,
                "include_note": "incident export currently writes incident list records; use pcl incidents trace/detail commands for per-incident enrichment",
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
    let value = with_envelope_metadata(value.clone());
    if json_output {
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        print!("{}", toon_string(&value));
    }
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
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact_dir(cli_args).join("jobs.jsonl"))
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

fn incident_export_job_id(args: &ExportIncidentsArgs, checkpoint: &Path) -> String {
    let mut hasher = DefaultHasher::new();
    "incident_export".hash(&mut hasher);
    args.project_id.hash(&mut hasher);
    args.environment.hash(&mut hasher);
    args.page.hash(&mut hasher);
    args.limit.hash(&mut hasher);
    args.max_pages.hash(&mut hasher);
    checkpoint.hash(&mut hasher);
    format!("incident-export-{:016x}", hasher.finish())
}

fn incident_export_resume_command(
    args: &ExportIncidentsArgs,
    out: &Path,
    errors: &Path,
    checkpoint: &Path,
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
    if !args.include.is_empty() {
        parts.push("--include".to_string());
        parts.push(shell_word(args.include.join(",")));
    }
    if args.continue_on_error {
        parts.push("--continue-on-error".to_string());
    }
    if args.allow_unauthenticated {
        parts.push("--allow-unauthenticated".to_string());
    }

    parts.join(" ")
}

fn shell_word(value: impl AsRef<str>) -> String {
    let value = value.as_ref();
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'/' | b'.' | b'_' | b'-' | b':' | b'@' | b'=')
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn print_llms_guide(json_output: bool) -> Result<(), ProductSurfaceError> {
    print_output(
        &json!({
            "status": "ok",
            "data": llms_guide(),
            "next_actions": [
                "pcl doctor",
                "pcl api manifest --json",
                "pcl completions bash > ~/.local/share/bash-completion/completions/pcl",
                "pcl jobs list",
            ],
        }),
        json_output,
    )
}

fn llms_guide() -> Value {
    json!({
        "name": "pcl",
        "purpose": "CLI-native control surface for Credible Layer API investigation and assertion workflows.",
        "default_output": "toon",
        "json_flag": "--json",
        "no_mcp_required": true,
        "principles": [
            "Use top-level workflow commands first.",
            "Use pcl api list/inspect/call as the raw OpenAPI escape hatch.",
            "Treat every output as an envelope with status, data, error, and next_actions.",
            "Use JSONL export artifacts for long investigations.",
            "Use request IDs from errors and pcl requests for audit trails.",
            "Prefer CLI contracts over MCP, browser automation, or scraped help text."
        ],
        "consumption_order": [
            "pcl --llms",
            "pcl doctor",
            "pcl whoami",
            "pcl workflows",
            "pcl schema list",
            "pcl api manifest --json",
            "top-level workflow commands",
            "pcl api inspect <operation-id> --json",
            "pcl api call <method> <path> --json"
        ],
        "orientation": [
            {
                "goal": "Check local readiness and auth truthfulness",
                "commands": ["pcl doctor", "pcl whoami", "pcl auth status --json"]
            },
            {
                "goal": "Discover available workflows",
                "commands": ["pcl workflows", "pcl schema list", "pcl api manifest --json"]
            },
            {
                "goal": "Inspect raw API shape",
                "commands": ["pcl api list --filter incidents --json", "pcl api inspect <operation-id> --json"]
            },
            {
                "goal": "Run raw calls",
                "commands": ["pcl api call get /health --allow-unauthenticated", "pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated"]
            },
            {
                "goal": "Export resumable incident data",
                "commands": ["pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --resume", "pcl jobs list"]
            }
        ],
        "command_surfaces": {
            "workflows": ["pcl incidents", "pcl projects", "pcl assertions", "pcl account", "pcl contracts", "pcl releases", "pcl deployments", "pcl access", "pcl integrations", "pcl protocol-manager", "pcl transfers", "pcl events", "pcl search"],
            "discovery": ["pcl --llms", "pcl llms", "pcl workflows", "pcl schema", "pcl api manifest", "pcl api list", "pcl api inspect"],
            "execution": ["pcl api call", "pcl export incidents"],
            "state": ["pcl artifacts", "pcl requests", "pcl jobs"],
            "shell": ["pcl completions bash", "pcl completions zsh", "pcl completions fish"]
        },
        "output_contract": {
            "default": "TOON envelope",
            "json": "Pass --json for pretty JSON envelopes.",
            "envelope_fields": ["status", "data", "error", "next_actions", "schema_version", "pcl_version"],
            "errors": "Parser, auth, config, validation, network, and API failures return structured envelopes and nonzero exit codes.",
            "error_fields": ["error.code", "error.message", "error.recoverable", "error.http.status", "error.request_id"],
            "long_running": "Export commands write JSONL artifacts, error files, checkpoints, and job records."
        },
        "mutation_safety": {
            "order": ["--body-template", "--dry-run", "typed flags", "--field key=value", "--body-file body.json"],
            "body_templates": "Print payload contracts before writes; choose a concrete body variant when body_variants is returned.",
            "dry_run": "Use dry-run request plans before destructive project, assertion, release, access, integration, transfer, or protocol-manager operations."
        },
        "raw_api": {
            "inspect_first": "Use pcl api inspect <operation-id> --json before unfamiliar calls.",
            "query_strings": "pcl api call accepts both /path?key=value and repeated --query key=value.",
            "public_endpoints": "Use --allow-unauthenticated for public raw calls so stale local tokens are not required.",
            "pagination": "Use --paginate <array-field> --limit <n> --max-pages <n> and optionally --jsonl --output <file> for generic GET pagination."
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
    let mut entries = fs::read_dir(dir)
        .map_err(|source| {
            ProductSurfaceError::Io {
                path: dir.to_path_buf(),
                source,
            }
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some(json!({
                "path": entry.path(),
                "bytes": metadata.len(),
                "modified": metadata.modified().ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_secs()),
            }))
        })
        .collect::<Vec<_>>();
    entries.truncate(limit);
    Ok(entries)
}

fn auth_check_status(auth: Option<&UserAuth>) -> &'static str {
    match auth {
        None => "missing",
        Some(auth) if auth.expires_at <= Utc::now() => "warning",
        Some(_) => "ok",
    }
}

fn auth_value(auth: Option<&UserAuth>) -> Value {
    let Some(auth) = auth else {
        return json!({
            "authenticated": false,
            "token_present": false,
            "token_valid": false,
            "expired": false,
        });
    };
    let seconds_remaining = (auth.expires_at - Utc::now()).num_seconds();
    let expired = auth.expires_at <= Utc::now();
    json!({
        "authenticated": true,
        "user": auth.display_name(),
        "user_id": auth.user_id.map(|id| id.to_string()),
        "wallet_address": auth.wallet_address.map(|address| address.to_string()),
        "email": auth.email.as_deref(),
        "token_present": !auth.access_token.is_empty(),
        "token_valid": !expired,
        "expired": expired,
        "expires_at": auth.expires_at.to_rfc3339(),
        "seconds_remaining": seconds_remaining,
    })
}

async fn health_check(api_url: &url::Url) -> Value {
    let url = match build_api_url(api_url, "/health") {
        Ok(url) => url,
        Err(error) => {
            return json!({
                "name": "api_health",
                "status": "error",
                "error": error.to_string(),
            });
        }
    };
    let response = reqwest::Client::new().get(url).send().await;
    match response {
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
            json!({
                "name": "api_health",
                "status": "error",
                "error": error.to_string(),
            })
        }
    }
}

fn workflow_recipes() -> Vec<Value> {
    vec![
        json!({
            "name": "incident-investigation",
            "description": "Export incidents, inspect failing detail/trace records, and preserve request IDs.",
            "steps": [
                {"command": "pcl doctor", "output": "environment readiness"},
                {"command": "pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --resume", "output": "incident JSONL artifact"},
                {"command": "pcl incidents --incident-id <incident-id>", "output": "incident detail"},
                {"command": "pcl incidents --incident-id <incident-id> --tx-id <tx-id>", "output": "transaction trace"},
                {"command": "pcl requests list --limit 20", "output": "API request IDs and status history"}
            ],
        }),
        json!({
            "name": "submit-assertions",
            "description": "Construct, submit, and verify submitted assertion state.",
            "steps": [
                {"command": "pcl assertions --project-id <project-id> --body-template", "output": "submission body contract"},
                {"command": "pcl assertions --project-id <project-id> --submit --body-file submitted-assertions.json", "output": "submit result"},
                {"command": "pcl assertions --project-id <project-id> --submitted", "output": "submitted assertion state"}
            ],
        }),
        json!({
            "name": "deploy-release",
            "description": "Create release payloads, preview, create, and fetch deploy calldata.",
            "steps": [
                {"command": "pcl releases --project <project-id> --body-template", "output": "release body contract"},
                {"command": "pcl releases --project <project-id> --preview --body-file release.json", "output": "release preview"},
                {"command": "pcl releases --project <project-id> --create --body-file release.json", "output": "created release"},
                {"command": "pcl releases --project <project-id> --release-id <release-id> --deploy-calldata --signer-address <address>", "output": "deployment calldata"}
            ],
        }),
        json!({
            "name": "invite-member",
            "description": "Invite a project member and inspect pending invitations.",
            "steps": [
                {"command": "pcl access --project <project-id> --invite --body-template", "output": "invite body contract"},
                {"command": "pcl access --project <project-id> --invite --body-file invite.json", "output": "invitation result"},
                {"command": "pcl access --project <project-id> --invitations", "output": "project invitations"}
            ],
        }),
        json!({
            "name": "protocol-manager-transfer",
            "description": "Inspect manager state, produce transfer calldata, and confirm transfer variants.",
            "steps": [
                {"command": "pcl protocol-manager --project <project-id> --pending-transfer", "output": "pending transfer"},
                {"command": "pcl protocol-manager --project <project-id> --nonce --address <manager-address>", "output": "manager nonce"},
                {"command": "pcl protocol-manager --project <project-id> --transfer-calldata --new-manager <address>", "output": "transfer calldata"},
                {"command": "pcl protocol-manager --confirm-transfer --body-template", "output": "direct/onchain confirm variants"}
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
        "include_requested": args.include,
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

fn build_api_url(base: &url::Url, path: &str) -> Result<url::Url, ProductSurfaceError> {
    if !path.starts_with('/') {
        return Err(ProductSurfaceError::InvalidInput(format!(
            "API path `{path}` must start with /"
        )));
    }
    let mut url = base.clone();
    url.set_path(&format!("/api/v1{path}"));
    Ok(url)
}

fn response_body_value(content_type: &str, bytes: &[u8]) -> Value {
    if content_type.contains("application/json") {
        return serde_json::from_slice(bytes).unwrap_or_else(|_| {
            json!({
                "parse_error": "response declared JSON but could not be parsed",
                "raw": String::from_utf8_lossy(bytes),
            })
        });
    }
    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(bytes).to_string()))
}

fn request_id_from_headers(headers: &HeaderMap) -> Option<String> {
    [
        "x-request-id",
        "x-correlation-id",
        "x-amzn-requestid",
        "cf-ray",
        "request-id",
    ]
    .into_iter()
    .find_map(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
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

fn write_checkpoint(
    path: &Path,
    next_page: u64,
    items_written: u64,
) -> Result<(), ProductSurfaceError> {
    fs::write(
        path,
        serde_json::to_vec_pretty(&json!({
            "next_page": next_page,
            "items_written": items_written,
            "updated_at": Utc::now().to_rfc3339(),
        }))?,
    )
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

fn log_request(kind: &str, method: &str, path: &str, status: u16, request_id: Option<&str>) {
    let _ = request_log::append_request_record(&json!({
        "timestamp": Utc::now().to_rfc3339(),
        "kind": kind,
        "method": method,
        "path": path,
        "status": status,
        "success": (200..=299).contains(&status),
        "request_id": request_id,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcl_common::args::CliArgs;
    use tempfile::tempdir;

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
    fn schema_finds_action_contract() {
        let commands = api_manifest()["commands"].as_array().cloned().unwrap();
        let schema = find_workflow_schema(&commands, "incidents").unwrap();
        assert!(schema["actions"].as_array().unwrap().iter().any(|action| {
            action["name"] == "list_public" && action["example"] == "pcl incidents --limit 5"
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

        assert_eq!(guide["default_output"], "toon");
        assert_eq!(guide["no_mcp_required"], true);
        assert_eq!(guide["agent_files"]["repo_instructions"], "AGENTS.md");
        assert!(
            guide["command_surfaces"]["discovery"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command == "pcl --llms")
        );
        assert!(
            guide["consumption_order"]
                .as_array()
                .unwrap()
                .iter()
                .any(|command| command == "pcl api manifest --json")
        );
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
            include: vec!["transactions".to_string(), "traces".to_string()],
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
        );

        assert!(command.contains("--resume"));
        assert!(command.contains("'project one'"));
        assert!(command.contains("'/tmp/pcl artifacts/incidents.jsonl'"));
        assert!(command.contains("--continue-on-error"));
    }
}
