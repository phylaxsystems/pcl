#![allow(
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::unnested_or_patterns,
    clippy::unused_self
)]

use crate::{
    DEFAULT_PLATFORM_URL,
    auth::refresh_stored_auth,
    config::CliConfig,
    error::AuthError,
};
use clap::{
    ArgGroup,
    ValueEnum,
};
use pcl_common::args::{
    CliArgs,
    OutputMode,
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
use std::{
    cell::Cell,
    fs,
    io::Read,
    path::{
        Path,
        PathBuf,
    },
    str::FromStr,
};

mod definitions;
mod manifest;
mod openapi;
mod render;
mod spec;
mod templates;
mod workflows;

pub use crate::output::{
    ENVELOPE_SCHEMA_VERSION,
    with_envelope_metadata,
};
pub use manifest::api_manifest;
pub use render::{
    envelope_output_string,
    human_string,
    toon_string,
};

use definitions::{
    WorkflowOutputPolicy,
    workflow_output_policy,
};
use openapi::{
    api_coverage,
    command_next_actions,
    inspect_operation,
    list_operations,
    next_actions_for_operations,
    openapi_path_matches,
    public_raw_call_path,
    write_api_coverage_markdown,
};
#[cfg(test)]
use openapi::{
    body_fields,
    body_variants,
    example_call,
    openapi_body_template,
    operation_auth_metadata,
    operation_input_placeholders,
    raw_api_use,
    required_body_fields,
    synthetic_operation_id,
    workflow_alternatives,
};
use render::print_output;
use templates::{
    access_body_template,
    body_template,
    contracts_body_template,
    deployment_body_template,
    integration_body_template,
    project_body_template,
    protocol_manager_body_template,
    release_body_template,
    template_envelope,
    transfer_body_template,
};
use workflows::{
    access_request,
    account_request,
    assertions_next_actions,
    assertions_request,
    compact_deployment_data,
    contracts_next_actions,
    contracts_request,
    deployments_request,
    events_request,
    incidents_next_actions,
    incidents_request,
    integrations_request,
    project_segment,
    projects_next_actions,
    projects_request,
    protocol_manager_next_actions,
    protocol_manager_request,
    releases_next_actions,
    releases_request,
    request_body,
    search_next_actions,
    search_request,
    transfers_next_actions,
    transfers_request,
};

#[derive(Debug, thiserror::Error)]
pub enum ApiCommandError {
    #[error("Run `pcl auth login` first, or pass `--allow-unauthenticated`")]
    NoAuthToken,

    #[error(
        "Stored auth token expired at {0}. Run `pcl auth refresh --toon` or `pcl auth login` again, or pass `--allow-unauthenticated` for public endpoints."
    )]
    ExpiredAuthToken(chrono::DateTime<chrono::Utc>),

    #[error("Failed to refresh stored auth before retrying the API request: {0}")]
    AuthRefresh(#[source] AuthError),

    #[error("Invalid {kind} `{input}`. Expected KEY=VALUE.")]
    InvalidKeyValue { kind: &'static str, input: String },

    #[error("Invalid header name `{name}`: {source}")]
    InvalidHeaderName {
        name: String,
        #[source]
        source: reqwest::header::InvalidHeaderName,
    },

    #[error("Invalid header value for `{name}`: {source}")]
    InvalidHeaderValue {
        name: String,
        #[source]
        source: reqwest::header::InvalidHeaderValue,
    },

    #[error("Invalid API path `{0}`. Paths must start with `/`.")]
    InvalidPath(String),

    #[error("Failed to build API URL: {0}")]
    Url(#[from] url::ParseError),

    #[error("Failed to read body file `{path}`: {source}")]
    BodyFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read request log `{path}`: {source}")]
    RequestLog {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write output file `{path}`: {source}")]
    OutputFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read request body from stdin: {0}")]
    Stdin(std::io::Error),

    #[error("Invalid JSON body: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("API request failed with status {status} for {method} {path}")]
    HttpStatus {
        method: &'static str,
        path: String,
        status: u16,
        request_id: Option<String>,
        body: Box<Value>,
    },

    #[error("OpenAPI spec does not contain a paths object")]
    MissingPaths,

    #[error("No API operation matched `{0}`")]
    OperationNotFound(String),

    #[error("{message}")]
    InvalidWorkflow { message: String },

    #[error("{message}")]
    InvalidWorkflowWithActions {
        message: String,
        next_actions: Vec<String>,
    },
}

impl ApiCommandError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAuthToken => "auth.no_token",
            Self::ExpiredAuthToken(_) => "auth.expired_token",
            Self::AuthRefresh(_) => "auth.refresh_failed",
            Self::InvalidKeyValue { .. } => "input.invalid_key_value",
            Self::InvalidHeaderName { .. } => "input.invalid_header_name",
            Self::InvalidHeaderValue { .. } => "input.invalid_header_value",
            Self::InvalidPath(_) => "input.invalid_path",
            Self::Url(_) => "input.invalid_url",
            Self::BodyFile { .. } => "input.body_file_read_failed",
            Self::RequestLog { .. } => "request_log.read_failed",
            Self::OutputFile { .. } => "output.file_write_failed",
            Self::Stdin(_) => "input.stdin_read_failed",
            Self::Json(_) => "input.invalid_json",
            Self::Request(source) => {
                match source.status().map(|status| status.as_u16()) {
                    Some(400) => "api.bad_request",
                    Some(401) => "auth.unauthorized",
                    Some(403) => "auth.forbidden",
                    Some(404) => "api.not_found",
                    Some(422) => "api.validation_failed",
                    Some(500..=599) => "api.server_error",
                    _ => "network.request_failed",
                }
            }
            Self::HttpStatus { status, .. } => {
                match *status {
                    400 => "api.bad_request",
                    401 => "auth.unauthorized",
                    403 => "auth.forbidden",
                    404 => "api.not_found",
                    422 => "api.validation_failed",
                    500..=599 => "api.server_error",
                    _ => "api.request_failed",
                }
            }
            Self::MissingPaths => "openapi.missing_paths",
            Self::OperationNotFound(_) => "openapi.operation_not_found",
            Self::InvalidWorkflow { .. } | Self::InvalidWorkflowWithActions { .. } => {
                "workflow.invalid_arguments"
            }
        }
    }

    pub fn recoverable(&self) -> bool {
        !matches!(self, Self::MissingPaths)
    }

    pub fn next_actions(&self) -> Vec<String> {
        match self {
            Self::NoAuthToken | Self::ExpiredAuthToken(_) | Self::AuthRefresh(_) => {
                vec![
                    "pcl auth refresh --toon".to_string(),
                    "pcl auth login".to_string(),
                    "pcl api list --allow-unauthenticated --toon".to_string(),
                ]
            }
            Self::InvalidPath(_) => {
                vec![
                    "pcl api list --toon".to_string(),
                    "pcl api call get /views/public/incidents --allow-unauthenticated --toon"
                        .to_string(),
                ]
            }
            Self::InvalidKeyValue { kind, .. } => {
                vec![format!(
                    "Use --{kind} key=value, for example: pcl api call get /views/public/incidents --{kind} limit=5"
                )]
            }
            Self::InvalidHeaderName { .. } | Self::InvalidHeaderValue { .. } => {
                vec![
                    "Use --header name=value, for example: --header x-cl-dev-mode=true".to_string(),
                ]
            }
            Self::Json(_) => {
                vec![
                    "Use --field key=value for simple request bodies".to_string(),
                    "Use --body-file request.json for nested request bodies".to_string(),
                ]
            }
            Self::OperationNotFound(_) => {
                vec![
                    "pcl api list --toon".to_string(),
                    "pcl api inspect get /views/public/incidents --toon".to_string(),
                ]
            }
            Self::InvalidWorkflowWithActions { next_actions, .. } => next_actions.clone(),
            Self::InvalidWorkflow { .. } => {
                vec![
                    "pcl projects mine".to_string(),
                    "pcl schema list".to_string(),
                    "pcl workflows".to_string(),
                ]
            }
            Self::Request(source)
                if matches!(
                    source.status().map(|status| status.as_u16()),
                    Some(401 | 403)
                ) =>
            {
                vec![
                    "pcl auth login".to_string(),
                    "Use --allow-unauthenticated only for public endpoints".to_string(),
                ]
            }
            Self::HttpStatus { status: 401, .. } => {
                vec![
                    "pcl auth refresh --toon".to_string(),
                    "pcl auth login".to_string(),
                    "Use --allow-unauthenticated only for public endpoints".to_string(),
                ]
            }
            Self::HttpStatus { status: 403, .. } => {
                vec![
                    "Read error.http.body for the API-provided reason".to_string(),
                    "Check whether the endpoint is enabled and your user has permission"
                        .to_string(),
                    "Use --allow-unauthenticated only for endpoints documented as public"
                        .to_string(),
                ]
            }
            Self::HttpStatus {
                method,
                path,
                status: 400 | 422,
                ..
            } => {
                vec![
                    format!(
                        "pcl api inspect {} {} --toon",
                        method.to_ascii_lowercase(),
                        path
                    ),
                    "pcl api manifest --toon".to_string(),
                    "Read error.http.body for the rejected field details".to_string(),
                ]
            }
            Self::HttpStatus { status: 404, .. } => {
                vec![
                    "Check the project ID, slug, or API path and retry".to_string(),
                    "pcl projects mine".to_string(),
                ]
            }
            Self::HttpStatus {
                method,
                status: status @ 500..=599,
                request_id,
                path,
                ..
            } => {
                let mut actions = if mutation_outcome_ambiguous(method, *status) {
                    vec![
                        format!(
                            "Do not retry immediately; inspect the target resource for {} {} to confirm whether the mutation applied",
                            method.to_ascii_lowercase(),
                            path
                        ),
                        "pcl requests list --toon".to_string(),
                        "Read error.http.body for API-provided failure details".to_string(),
                    ]
                } else {
                    vec![
                        "Retry the same command once; server errors can be transient".to_string(),
                        "pcl api manifest --toon".to_string(),
                        "Read error.http.body for API-provided failure details".to_string(),
                    ]
                };
                if let Some(request_id) = request_id {
                    actions.push(format!(
                        "Include request_id {request_id} when reporting this server error"
                    ));
                }
                actions
            }
            Self::HttpStatus { .. } => {
                vec![
                    "pcl api manifest --toon".to_string(),
                    "Read error.http.body for API-provided failure details".to_string(),
                ]
            }
            Self::Request(source) if source.status().map(|status| status.as_u16()) == Some(404) => {
                vec![
                    "Check the project ID, slug, or API path and retry".to_string(),
                    "pcl projects mine".to_string(),
                ]
            }
            Self::Request(_) | Self::Url(_) => {
                vec!["Check --api-url and your network connection, then retry".to_string()]
            }
            Self::BodyFile { .. } => {
                vec!["Check --body-file path or pass --body directly".to_string()]
            }
            Self::RequestLog { .. } => {
                vec![
                    "pcl requests path --toon".to_string(),
                    "Check request log permissions or move the PCL state directory".to_string(),
                ]
            }
            Self::OutputFile { .. } => {
                vec!["Check --output path permissions or choose a writable file".to_string()]
            }
            Self::Stdin(_) => vec!["Pipe a JSON body into --body-file -".to_string()],
            Self::MissingPaths => {
                vec!["Check that /api/v1/openapi returns an OpenAPI document".to_string()]
            }
        }
    }

    pub fn suggested_next_actions(&self) -> Vec<&'static str> {
        match self {
            Self::NoAuthToken | Self::ExpiredAuthToken(_) | Self::AuthRefresh(_) => {
                vec!["refresh_or_login", "retry"]
            }
            Self::InvalidKeyValue { .. }
            | Self::InvalidHeaderName { .. }
            | Self::InvalidHeaderValue { .. }
            | Self::InvalidPath(_)
            | Self::Json(_)
            | Self::InvalidWorkflow { .. }
            | Self::InvalidWorkflowWithActions { .. } => vec!["fix_input", "retry"],
            Self::OperationNotFound(_) | Self::MissingPaths => vec!["inspect_manifest"],
            Self::Request(source) if source.status().map(|status| status.as_u16()) == Some(404) => {
                vec!["check_ids", "retry"]
            }
            Self::Request(_) | Self::Url(_) => vec!["check_network", "retry"],
            Self::BodyFile { .. } | Self::Stdin(_) => vec!["fix_body_input", "retry"],
            Self::RequestLog { .. } => vec!["inspect_request_log", "retry"],
            Self::OutputFile { .. } => vec!["fix_output_path", "retry"],
            Self::HttpStatus { status: 401, .. } => vec!["refresh_or_login", "retry"],
            Self::HttpStatus { status: 403, .. } => {
                vec!["check_permissions", "inspect_response_body"]
            }
            Self::HttpStatus {
                status: 400 | 422, ..
            } => vec!["inspect_operation", "fix_request", "retry"],
            Self::HttpStatus { status: 404, .. } => vec!["inspect_manifest", "check_ids"],
            Self::HttpStatus { status: 429, .. } => vec!["retry_later", "reduce_request_rate"],
            Self::HttpStatus {
                method,
                status: status @ 500..=599,
                ..
            } if mutation_outcome_ambiguous(method, *status) => {
                vec!["reconcile_mutation", "contact_platform_with_request_id"]
            }
            Self::HttpStatus {
                status: 500..=599, ..
            } => {
                vec![
                    "retry_later",
                    "export_project_incidents_with_errors",
                    "contact_platform_with_request_id",
                ]
            }
            Self::HttpStatus { .. } => vec!["inspect_response_body", "retry"],
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
            let outcome_ambiguous = mutation_outcome_ambiguous(method, *status);
            if let Some(request_id) = request_id {
                error.insert("request_id".to_string(), json!(request_id));
            }
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
            error.insert(
                "mutation".to_string(),
                json!({
                    "side_effecting": method_side_effecting(method),
                    "outcome_ambiguous": outcome_ambiguous,
                }),
            );
        }

        let mut envelope = json!({
            "status": "error",
            "error": error,
            "suggested_next_actions": self.suggested_next_actions(),
            "next_actions": self.next_actions(),
        });

        if let Self::HttpStatus {
            method,
            path,
            status,
            request_id,
            ..
        } = self
            && let Some(object) = envelope.as_object_mut()
        {
            object.insert("http_status".to_string(), json!(status));
            object.insert("method".to_string(), json!(method));
            object.insert("path".to_string(), json!(path));
            object.insert("request_id".to_string(), json!(request_id));
            object.insert(
                "outcome_ambiguous".to_string(),
                json!(mutation_outcome_ambiguous(method, *status)),
            );
        }

        with_envelope_metadata(envelope)
    }
}

#[derive(clap::Parser, Debug)]
#[command(
    about = "Discover and call the platform API",
    long_about = "Discover and call the Credible Layer platform API. Commands use human-readable output by default. Pass --toon for compact agent envelopes or --json for strict JSON envelopes."
)]
pub struct ApiArgs {
    #[command(subcommand)]
    command: ApiCommand,

    #[arg(
        long = "api-url",
        env = "PCL_API_URL",
        default_value = DEFAULT_PLATFORM_URL,
        global = true,
        help = "Base URL for the platform API"
    )]
    api_url: url::Url,

    #[arg(
        long,
        global = true,
        help = "Do not attach the stored bearer token to API requests"
    )]
    allow_unauthenticated: bool,

    #[arg(
        long = "dry-run",
        global = true,
        help = "Print the request plan without sending an API request"
    )]
    dry_run: bool,

    #[arg(skip = Cell::new(true))]
    refresh_after_401: Cell<bool>,
}

#[derive(clap::Args, Debug)]
struct ApiWorkflowOptions {
    #[arg(
        long = "api-url",
        env = "PCL_API_URL",
        default_value = DEFAULT_PLATFORM_URL,
        global = true,
        help = "Base URL for the platform API"
    )]
    api_url: url::Url,

    #[arg(
        long,
        global = true,
        help = "Do not attach the stored bearer token to API requests"
    )]
    allow_unauthenticated: bool,

    #[arg(
        long = "dry-run",
        global = true,
        help = "Print the request plan without sending an API request"
    )]
    dry_run: bool,
}

impl ApiWorkflowOptions {
    async fn run(
        self,
        command: ApiCommand,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        ApiArgs {
            command,
            api_url: self.api_url,
            allow_unauthenticated: self.allow_unauthenticated,
            dry_run: self.dry_run,
            refresh_after_401: Cell::new(true),
        }
        .run(config, cli_args, json_output)
        .await
    }
}

macro_rules! top_level_workflow_command {
    ($name:ident, $args:ty, $variant:ident, $about:literal, $after_help:literal) => {
        #[derive(clap::Args, Debug)]
        #[command(about = $about, after_help = $after_help)]
        pub struct $name {
            #[command(flatten)]
            globals: ApiWorkflowOptions,
            #[command(flatten)]
            args: $args,
        }

        impl $name {
            pub async fn run(
                self,
                config: &mut CliConfig,
                cli_args: &CliArgs,
                json_output: bool,
            ) -> Result<(), ApiCommandError> {
                self.globals
                    .run(
                        ApiCommand::$variant(self.args),
                        config,
                        cli_args,
                        json_output,
                    )
                    .await
            }
        }
    };
}

#[derive(clap::Subcommand, Debug)]
enum ApiCommand {
    #[command(
        about = "List or inspect incidents",
        after_help = "Examples:\n  pcl incidents --limit 5\n  pcl incidents --project-id <project-id> --environment production\n  pcl incidents --project-id <project-id> --all --limit 50 --output incidents.json\n  pcl incidents --incident-id <incident-id>\n  pcl incidents --incident-id <incident-id> --tx-id <tx-id>\n  pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace"
    )]
    Incidents(IncidentsArgs),

    #[command(
        about = "List, inspect, create, update, save, or delete projects",
        after_help = "Examples:\n  pcl projects mine\n  pcl projects list\n  pcl projects show <project-ref>\n  pcl projects saved --user-id <user-id>\n  pcl projects create --project-name demo --chain-id 1\n  pcl projects update <project-ref> --field github_url=https://github.com/org/repo\n  pcl projects save <project-ref>"
    )]
    Projects(ProjectsArgs),

    #[command(
        about = "List, inspect, and manage project assertions",
        after_help = "Examples:\n  pcl assertions --project-id <project-ref>\n  pcl assertions --project-id <project-ref> --registered\n  pcl assertions --project-id <project-ref> --remove-info"
    )]
    Assertions(AssertionsArgs),

    #[command(
        about = "Search and inspect platform-wide metadata",
        after_help = "Examples:\n  pcl search --query settler\n  pcl search --stats\n  pcl search --system-status\n  pcl search --verified-contract --address 0x... --chain-id 1"
    )]
    Search(SearchArgs),

    #[command(
        about = "Inspect and manage current account onboarding state",
        after_help = "Examples:\n  pcl account\n  pcl account --accept-terms\n  pcl account --logout"
    )]
    Account(AccountArgs),

    #[command(
        about = "List or manage project contracts and assertion adopters",
        after_help = "Examples:\n  pcl contracts --project <project-ref>\n  pcl contracts --project <project-ref> --adopter-id <adopter-id>\n  pcl contracts --unassigned --manager <manager-address>\n  pcl contracts --create --body-template"
    )]
    Contracts(ContractsArgs),

    #[command(
        about = "List, inspect, create, preview, check, retry, deploy, or remove releases",
        after_help = "Examples:\n  pcl releases list <project-ref>\n  pcl releases show <project-ref> <release-id>\n  pcl releases preview <project-ref> --body-file release.json\n  pcl releases backtest-progress <project-ref> <release-id>\n  pcl releases retry-check <project-ref> <release-id> <check-id>\n  pcl releases calldata deploy <project-ref> <release-id> --signer-address <signer-address>"
    )]
    Releases(ReleasesArgs),

    #[command(
        about = "Inspect deployments and confirm deployed assertions",
        after_help = "Examples:\n  pcl deployments --project <project-ref>\n  pcl deployments --project <project-ref> --confirm --body-template"
    )]
    Deployments(DeploymentsArgs),

    #[command(
        about = "Manage members, roles, and invitations",
        after_help = "Examples:\n  pcl access members <project-ref>\n  pcl access invite <project-ref> --body-template\n  pcl access pending\n  pcl access preview <token>"
    )]
    Access(AccessArgs),

    #[command(
        about = "Manage Slack and PagerDuty integrations",
        after_help = "Examples:\n  pcl integrations --project <project-ref> --provider slack\n  pcl integrations --project <project-ref> --provider pagerduty --configure --body-template\n  pcl integrations --project <project-ref> --provider slack --test"
    )]
    Integrations(IntegrationsArgs),

    #[command(
        about = "Manage project protocol manager settings",
        after_help = "Examples:\n  pcl protocol-manager --project <project-ref> --nonce --address <manager-address>\n  pcl protocol-manager --project <project-ref> --transfer-calldata --new-manager 0x...\n  pcl protocol-manager --project <project-ref> --set --body-template"
    )]
    ProtocolManager(ProtocolManagerArgs),

    #[command(
        about = "Inspect or reject protocol manager transfers",
        after_help = "Examples:\n  pcl transfers --pending\n  pcl transfers --transfer-id <transfer-id>\n  pcl transfers --reject --body-template"
    )]
    Transfers(TransfersArgs),

    #[command(
        about = "Inspect project events and audit logs",
        after_help = "Examples:\n  pcl events --project <project-ref>\n  pcl events --project <project-ref> --audit-log"
    )]
    Events(EventsArgs),

    #[command(
        about = "Print an agent-readable command manifest",
        after_help = "Examples:\n  pcl api manifest\n  pcl api manifest --toon\n  pcl api manifest --json"
    )]
    Manifest,

    #[command(
        about = "List OpenAPI operations",
        after_help = "Examples:\n  pcl api list\n  pcl api list --filter incidents\n  pcl api list --method get\n  pcl api list --toon\n  pcl api list --json"
    )]
    List {
        #[arg(long, help = "Filter operation id, summary, tags, or path")]
        filter: Option<String>,
        #[arg(long, value_enum, ignore_case = true, help = "Filter by HTTP method")]
        method: Option<HttpMethod>,
    },

    #[command(
        about = "Inspect one OpenAPI operation",
        after_help = "Examples:\n  pcl api inspect get_views_projects_project_id_incidents\n  pcl api inspect get /views/public/incidents\n  pcl api inspect get_views_projects_project_id_incidents --toon\n  pcl api inspect get_views_projects_project_id_incidents --json"
    )]
    Inspect {
        #[arg(help = "Operation id, or HTTP method when PATH is also provided")]
        operation: String,
        #[arg(help = "OpenAPI path when OPERATION is an HTTP method")]
        path: Option<String>,
        #[arg(long, help = "Include the raw OpenAPI operation")]
        full: bool,
    },

    #[command(
        name = "coverage",
        alias = "audit",
        about = "Compare the local request log against the live OpenAPI surface",
        after_help = "Examples:\n  pcl api coverage --toon\n  pcl api coverage --json\n  pcl api coverage --records 5000 --markdown /tmp/pcl-api-coverage.md"
    )]
    Coverage {
        #[arg(
            long,
            default_value_t = 5000,
            help = "Maximum recent request records to consider"
        )]
        records: usize,
        #[arg(long, help = "Write a markdown coverage report to this path")]
        markdown: Option<PathBuf>,
    },

    #[command(
        about = "Call any platform API endpoint",
        after_help = "Examples:\n  pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated\n  pcl api call get /views/projects/<uuid>/incidents --query environment=production\n  pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --output incidents.json\n  pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --jsonl --output incidents.jsonl\n  pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated --output incidents.json\n  pcl api call post /web/auth/logout --body '{}'\n  pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated --toon"
    )]
    Call {
        #[arg(value_enum, ignore_case = true, help = "HTTP method")]
        method: HttpMethod,
        #[arg(help = "API path below /api/v1, for example /views/public/incidents")]
        path: String,
        #[arg(long = "query", short = 'q', help = "Query parameter as KEY=VALUE")]
        query: Vec<String>,
        #[arg(
            long = "header",
            short = 'H',
            help = "Extra request header as NAME=VALUE"
        )]
        header: Vec<String>,
        #[arg(long, conflicts_with = "body_file", help = "JSON request body")]
        body: Option<String>,
        #[arg(
            long = "body-file",
            conflicts_with = "body",
            help = "Path to JSON request body, or - for stdin"
        )]
        body_file: Option<PathBuf>,
        #[arg(
            long = "field",
            help = "Extra JSON body field as KEY=VALUE; VALUE may be a JSON scalar/object/array"
        )]
        field: Vec<String>,
        #[arg(
            long,
            value_name = "FIELD",
            help = "Fetch every page and aggregate array field/path from each response"
        )]
        paginate: Option<String>,
        #[arg(
            long,
            requires = "paginate",
            help = "Explicitly fetch all pages; --paginate already enables this"
        )]
        all: bool,
        #[arg(long, requires = "paginate", help = "Starting page for --paginate")]
        page: Option<u64>,
        #[arg(long, requires = "paginate", help = "Items per page for --paginate")]
        limit: Option<u64>,
        #[arg(
            long = "page-param",
            requires = "paginate",
            help = "Query parameter name for page number"
        )]
        page_param: Option<String>,
        #[arg(
            long = "limit-param",
            requires = "paginate",
            help = "Query parameter name for page size"
        )]
        limit_param: Option<String>,
        #[arg(
            long,
            requires = "paginate",
            help = "Maximum pages to fetch with --paginate"
        )]
        max_pages: Option<u64>,
        #[arg(
            long,
            requires = "paginate",
            help = "With --paginate and --output, write items as JSON Lines"
        )]
        jsonl: bool,
        #[arg(long, help = "Write response body to a JSON file")]
        output: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }

    fn openapi_key(self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Post => "post",
            Self::Put => "put",
            Self::Patch => "patch",
            Self::Delete => "delete",
        }
    }

    fn reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
        }
    }
}

fn method_side_effecting(method: &str) -> bool {
    !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD")
}

fn mutation_outcome_ambiguous(method: &str, status: u16) -> bool {
    method_side_effecting(method) && status >= 500
}

struct ApiRequestInput<'a> {
    method: HttpMethod,
    path: &'a str,
    query: &'a [String],
    header: &'a [String],
    body: Option<&'a str>,
    body_file: Option<&'a PathBuf>,
    field: &'a [String],
    require_auth: bool,
}

struct PreparedApiRequest<'a> {
    attach_auth: bool,
    method: HttpMethod,
    url: &'a url::Url,
    headers: &'a HeaderMap,
    query: &'a [(String, String)],
    body: Option<&'a Value>,
}

#[derive(Clone, Copy)]
struct RawPaginationOptions<'a> {
    item_field: &'a str,
    start_page: u64,
    limit: u64,
    page_param: &'a str,
    limit_param: &'a str,
    max_pages: u64,
}

#[derive(Clone, Copy)]
struct WorkflowPaginationOptions<'a> {
    item_field: &'a str,
    start_page: u64,
    limit: u64,
    max_pages: u64,
}

#[derive(Debug)]
struct WorkflowCallResult {
    body: Value,
    request: Value,
    response: Value,
}

#[derive(Clone, Debug)]
struct WorkflowRequest {
    method: HttpMethod,
    path: String,
    query: Vec<(String, String)>,
    body: Option<String>,
    require_auth: bool,
    attach_auth: bool,
    next_actions: Vec<String>,
}

impl WorkflowRequest {
    fn get(
        path: impl Into<String>,
        require_auth: bool,
        next_actions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::get_with_query(path, Vec::new(), require_auth, next_actions)
    }

    fn get_with_query(
        path: impl Into<String>,
        query: Vec<(String, String)>,
        require_auth: bool,
        next_actions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            query,
            body: None,
            require_auth,
            attach_auth: require_auth,
            next_actions: next_actions.into_iter().map(Into::into).collect(),
        }
    }

    fn with_optional_auth(mut self) -> Self {
        self.attach_auth = true;
        self
    }
}

#[derive(clap::Args, Debug)]
struct IncidentsArgs {
    #[arg(
        long,
        visible_alias = "project",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project_id: Option<String>,
    #[arg(long, alias = "incident_id", help = "Incident ID to inspect")]
    incident_id: Option<String>,
    #[arg(long, alias = "tx_id", help = "Transaction ID for incident trace")]
    tx_id: Option<String>,
    #[arg(
        long,
        alias = "assertion_id",
        help = "Filter project incidents by assertion"
    )]
    assertion_id: Option<String>,
    #[arg(
        long,
        alias = "assertion_adopter_id",
        help = "Filter project incidents by assertion adopter"
    )]
    assertion_adopter_id: Option<String>,
    #[arg(long, help = "Filter project incidents by environment")]
    environment: Option<String>,
    #[arg(
        long,
        alias = "from_date",
        help = "Filter project incidents from an ISO date"
    )]
    from_date: Option<String>,
    #[arg(
        long,
        alias = "to_date",
        help = "Filter project incidents until an ISO date"
    )]
    to_date: Option<String>,
    #[arg(long, help = "Page number")]
    page: Option<u64>,
    #[arg(long, help = "Items per page")]
    limit: Option<u64>,
    #[arg(long, help = "Filter public incidents by chain ID")]
    network: Option<u64>,
    #[arg(long, help = "Sort direction for public incidents")]
    sort: Option<String>,
    #[arg(
        long,
        alias = "dev_mode",
        help = "Include development-mode public incidents"
    )]
    dev_mode: Option<String>,
    #[arg(long, help = "Return incident stats for --project-id")]
    stats: bool,
    #[arg(long, alias = "retry_trace", help = "Retry failed trace generation")]
    retry_trace: bool,
    #[arg(long, help = "Fetch every page for incident list workflows")]
    all: bool,
    #[arg(long, help = "Maximum pages to fetch with --all")]
    max_pages: Option<u64>,
    #[arg(long, help = "Write response data to a JSON file")]
    output: Option<PathBuf>,
    #[arg(
        long,
        requires = "all",
        help = "With --all and --output, write incident items as JSON Lines"
    )]
    jsonl: bool,
}

#[derive(clap::Args, Debug, Default)]
#[command(group(
    ArgGroup::new("project_action")
        .args(["mine", "saved", "create", "update", "delete", "save", "unsave", "resolve", "widget"])
        .multiple(false)
))]
struct ProjectsArgs {
    #[arg(
        long,
        visible_alias = "project",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project_id: Option<String>,
    #[arg(long, visible_alias = "home", help = "Show projects you belong to")]
    mine: bool,
    #[arg(long, help = "Return saved projects")]
    saved: bool,
    #[arg(long, alias = "user_id", help = "User ID for --saved")]
    user_id: Option<String>,
    #[arg(long, help = "Page number for project explorer")]
    page: Option<u64>,
    #[arg(long, help = "Items per page for project explorer")]
    limit: Option<u64>,
    #[arg(long, help = "Filter by search term if supported by the API")]
    search: Option<String>,
    #[arg(long, help = "Create a project")]
    create: bool,
    #[arg(long, help = "Update --project-id")]
    update: bool,
    #[arg(long, help = "Delete --project-id")]
    delete: bool,
    #[arg(long, help = "Save --project-id for current user")]
    save: bool,
    #[arg(long, help = "Unsave --project-id for current user")]
    unsave: bool,
    #[arg(
        long,
        help = "Resolve --project-id slug or UUID to canonical identifiers"
    )]
    resolve: bool,
    #[arg(long, help = "Return lightweight widget data for --project-id")]
    widget: bool,
    #[arg(long, alias = "project_name", help = "Project name for create/update")]
    project_name: Option<String>,
    #[arg(long, alias = "project_description", help = "Project description")]
    project_description: Option<String>,
    #[arg(long, alias = "profile_image_url", help = "Project profile image URL")]
    profile_image_url: Option<String>,
    #[arg(long, alias = "github_url", help = "Project GitHub URL")]
    github_url: Option<String>,
    #[arg(long, alias = "chain_id", help = "Chain ID for create")]
    chain_id: Option<u64>,
    #[arg(long, alias = "is_private", help = "Project privacy flag")]
    is_private: Option<bool>,
    #[arg(long, alias = "is_dev", help = "Project dev-mode flag")]
    is_dev: Option<bool>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(
    about = "List, inspect, create, update, save, or delete projects",
    after_help = "Examples:\n  pcl projects mine\n  pcl projects list\n  pcl projects show <project-ref>\n  pcl projects saved --user-id <user-id>\n  pcl projects create --project-name demo --chain-id 1\n  pcl projects update <project-ref> --field github_url=https://github.com/org/repo\n  pcl projects save <project-ref>\n\nLegacy flag forms are still supported."
)]
pub struct ProjectsCommand {
    #[command(flatten)]
    globals: ApiWorkflowOptions,
    #[command(subcommand)]
    command: Option<ProjectsSubcommand>,
    #[command(flatten)]
    legacy: ProjectsArgs,
}

#[derive(clap::Subcommand, Debug)]
enum ProjectsSubcommand {
    #[command(about = "List public projects")]
    List(ProjectListArgs),
    #[command(about = "Show projects you belong to")]
    Mine,
    #[command(about = "Show one project")]
    Show(ProjectRefArgs),
    #[command(about = "List projects saved by a user")]
    Saved(ProjectSavedArgs),
    #[command(about = "Create a project")]
    Create(ProjectWriteArgs),
    #[command(about = "Update a project")]
    Update(ProjectUpdateArgs),
    #[command(about = "Delete a project")]
    Delete(ProjectRefArgs),
    #[command(about = "Save a project for the current user")]
    Save(ProjectRefArgs),
    #[command(about = "Unsave a project for the current user")]
    Unsave(ProjectRefArgs),
    #[command(about = "Resolve a project slug or UUID")]
    Resolve(ProjectRefArgs),
    #[command(about = "Show lightweight project widget data")]
    Widget(ProjectRefArgs),
}

#[derive(clap::Args, Debug)]
struct ProjectRefArgs {
    #[arg(value_name = "PROJECT")]
    project: String,
}

#[derive(clap::Args, Debug, Default)]
struct ProjectListArgs {
    #[arg(long, help = "Page number for project explorer")]
    page: Option<u64>,
    #[arg(long, help = "Items per page for project explorer")]
    limit: Option<u64>,
    #[arg(long, help = "Filter by search term if supported by the API")]
    search: Option<String>,
}

#[derive(clap::Args, Debug, Default)]
struct ProjectSavedArgs {
    #[arg(long, alias = "user_id", help = "User ID for saved projects")]
    user_id: Option<String>,
}

#[derive(clap::Args, Debug, Default)]
struct ProjectUpdateArgs {
    #[arg(value_name = "PROJECT")]
    project: String,
    #[command(flatten)]
    write: ProjectWriteArgs,
}

#[derive(clap::Args, Debug, Default)]
struct ProjectWriteArgs {
    #[arg(long, alias = "project_name", help = "Project name for create/update")]
    project_name: Option<String>,
    #[arg(long, alias = "project_description", help = "Project description")]
    project_description: Option<String>,
    #[arg(long, alias = "profile_image_url", help = "Project profile image URL")]
    profile_image_url: Option<String>,
    #[arg(long, alias = "github_url", help = "Project GitHub URL")]
    github_url: Option<String>,
    #[arg(long, alias = "chain_id", help = "Chain ID for create")]
    chain_id: Option<u64>,
    #[arg(long, alias = "is_private", help = "Project privacy flag")]
    is_private: Option<bool>,
    #[arg(long, alias = "is_dev", help = "Project dev-mode flag")]
    is_dev: Option<bool>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

impl ProjectsCommand {
    pub async fn run(
        self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        let args = match self.command {
            Some(command) => command.into_args(),
            None => self.legacy,
        };
        self.globals
            .run(ApiCommand::Projects(args), config, cli_args, json_output)
            .await
    }
}

impl ProjectsSubcommand {
    fn into_args(self) -> ProjectsArgs {
        match self {
            Self::List(args) => {
                ProjectsArgs {
                    page: args.page,
                    limit: args.limit,
                    search: args.search,
                    ..ProjectsArgs::default()
                }
            }
            Self::Mine => {
                ProjectsArgs {
                    mine: true,
                    ..ProjectsArgs::default()
                }
            }
            Self::Show(args) => project_ref_args(args.project),
            Self::Saved(args) => {
                ProjectsArgs {
                    saved: true,
                    user_id: args.user_id,
                    ..ProjectsArgs::default()
                }
            }
            Self::Create(args) => {
                let mut project_args = ProjectsArgs {
                    create: true,
                    ..ProjectsArgs::default()
                };
                args.apply_to(&mut project_args);
                project_args
            }
            Self::Update(args) => {
                let mut project_args = ProjectsArgs {
                    project_id: Some(args.project),
                    update: true,
                    ..ProjectsArgs::default()
                };
                args.write.apply_to(&mut project_args);
                project_args
            }
            Self::Delete(args) => {
                ProjectsArgs {
                    project_id: Some(args.project),
                    delete: true,
                    ..ProjectsArgs::default()
                }
            }
            Self::Save(args) => {
                ProjectsArgs {
                    project_id: Some(args.project),
                    save: true,
                    ..ProjectsArgs::default()
                }
            }
            Self::Unsave(args) => {
                ProjectsArgs {
                    project_id: Some(args.project),
                    unsave: true,
                    ..ProjectsArgs::default()
                }
            }
            Self::Resolve(args) => {
                ProjectsArgs {
                    project_id: Some(args.project),
                    resolve: true,
                    ..ProjectsArgs::default()
                }
            }
            Self::Widget(args) => {
                ProjectsArgs {
                    project_id: Some(args.project),
                    widget: true,
                    ..ProjectsArgs::default()
                }
            }
        }
    }
}

impl ProjectWriteArgs {
    fn apply_to(self, args: &mut ProjectsArgs) {
        args.project_name = self.project_name;
        args.project_description = self.project_description;
        args.profile_image_url = self.profile_image_url;
        args.github_url = self.github_url;
        args.chain_id = self.chain_id;
        args.is_private = self.is_private;
        args.is_dev = self.is_dev;
        args.field = self.field;
        args.body = self.body;
        args.body_file = self.body_file;
        args.body_template = self.body_template;
    }
}

fn project_ref_args(project: String) -> ProjectsArgs {
    ProjectsArgs {
        project_id: Some(project),
        ..ProjectsArgs::default()
    }
}

#[derive(clap::Args, Debug, Default)]
struct WorkflowBodyArgs {
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

impl WorkflowBodyArgs {
    fn apply_to_release(self, args: &mut ReleasesArgs) {
        args.body = self.body;
        args.field = self.field;
        args.body_file = self.body_file;
        args.body_template = self.body_template;
    }

    fn apply_to_access(self, args: &mut AccessArgs) {
        args.body = self.body;
        args.field = self.field;
        args.body_file = self.body_file;
        args.body_template = self.body_template;
    }
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("assertions_action")
        .args(["assertion_id", "adopter_address", "submitted", "registered", "submit", "remove_info", "remove_calldata"])
        .multiple(false)
))]
struct AssertionsArgs {
    #[arg(
        long,
        visible_alias = "project",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project_id: Option<String>,
    #[arg(long, alias = "assertion_id", help = "Assertion ID to inspect")]
    assertion_id: Option<String>,
    #[arg(long, alias = "adopter_id", help = "Filter by assertion adopter")]
    adopter_id: Option<String>,
    #[arg(
        long,
        alias = "adopter_address",
        alias = "aa-address",
        help = "Assertion adopter contract address for /assertions lookup"
    )]
    adopter_address: Option<String>,
    #[arg(long, help = "Network/chain ID for --adopter-address")]
    network: Option<String>,
    #[arg(
        long,
        alias = "include_onchain_only",
        help = "Only include on-chain assertions for --adopter-address"
    )]
    include_onchain_only: Option<bool>,
    #[arg(long, help = "Filter by assertion environment")]
    environment: Option<String>,
    #[arg(long, help = "Page number")]
    page: Option<u64>,
    #[arg(long, help = "Items per page")]
    limit: Option<u64>,
    #[arg(
        long,
        hide = true,
        help = "Removed: submitted assertions are no longer exposed by the API"
    )]
    submitted: bool,
    #[arg(long, help = "Return registered assertions for --project-id")]
    registered: bool,
    #[arg(
        long,
        hide = true,
        help = "Removed: submitted assertions are no longer exposed by the API"
    )]
    submit: bool,
    #[arg(long, alias = "remove_info", help = "Return remove assertions info")]
    remove_info: bool,
    #[arg(
        long,
        alias = "remove_calldata",
        help = "Generate remove assertions calldata"
    )]
    remove_calldata: bool,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("search_action")
        .args(["stats", "system_status", "health", "whitelist", "verified_contract"])
        .multiple(false)
))]
struct SearchArgs {
    #[arg(value_name = "QUERY", help = "Search query")]
    term: Option<String>,
    #[arg(long, short = 'q', help = "Search query")]
    query: Option<String>,
    #[arg(long, help = "Return network statistics")]
    stats: bool,
    #[arg(long, alias = "system_status", help = "Return system status")]
    system_status: bool,
    #[arg(long, help = "Return health check")]
    health: bool,
    #[arg(long, help = "Return whitelist status for the authenticated user")]
    whitelist: bool,
    #[arg(
        long,
        alias = "verified_contract",
        help = "Look up verified contract info"
    )]
    verified_contract: bool,
    #[arg(long, help = "Contract address for --verified-contract")]
    address: Option<String>,
    #[arg(long, alias = "chain_id", help = "Chain ID for --verified-contract")]
    chain_id: Option<u64>,
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("account_action")
        .args(["me", "accept_terms", "logout"])
        .multiple(false)
))]
struct AccountArgs {
    #[arg(long, help = "Return current authenticated user info")]
    me: bool,
    #[arg(long, alias = "accept_terms", help = "Accept terms of service")]
    accept_terms: bool,
    #[arg(long, help = "Clear web auth session")]
    logout: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("contracts_action")
        .args(["unassigned", "create", "assign_project", "remove", "remove_calldata"])
        .multiple(false)
))]
struct ContractsArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(
        long,
        alias = "adopter_id",
        help = "Assertion adopter ID for contract detail"
    )]
    adopter_id: Option<String>,
    #[arg(
        long,
        alias = "aa_address",
        help = "Assertion adopter contract address"
    )]
    aa_address: Option<String>,
    #[arg(long, help = "Manager address for --unassigned")]
    manager: Option<String>,
    #[arg(long, help = "Network/chain ID for adopter calldata requests")]
    network: Option<String>,
    #[arg(long, help = "Environment for adopter calldata requests")]
    environment: Option<String>,
    #[arg(
        long = "assertion-id",
        alias = "assertion_id",
        alias = "assertion-ids",
        alias = "assertion_ids",
        help = "Assertion ID to include in --remove-calldata; repeat for multiple assertions"
    )]
    assertion_ids: Vec<String>,
    #[arg(long, help = "List unassigned assertion adopters")]
    unassigned: bool,
    #[arg(long, help = "Create an assertion adopter")]
    create: bool,
    #[arg(long, alias = "assign_project", help = "Assign adopters to a project")]
    assign_project: bool,
    #[arg(long, help = "Remove assertion adopter from --project")]
    remove: bool,
    #[arg(
        long,
        alias = "remove_calldata",
        help = "Get remove assertions calldata"
    )]
    remove_calldata: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug, Default)]
#[command(group(
    ArgGroup::new("releases_action")
        .args(["create", "preview", "deploy", "remove", "deploy_calldata", "remove_calldata", "backtest_progress", "retry_check"])
        .multiple(false)
))]
struct ReleasesArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(long, alias = "release_id", help = "Release ID")]
    release_id: Option<String>,
    #[arg(
        long,
        alias = "signer_address",
        help = "Signer address for --deploy-calldata"
    )]
    signer_address: Option<String>,
    #[arg(long, alias = "check_id", help = "Release check ID for --retry-check")]
    check_id: Option<String>,
    #[arg(long, help = "Create a release")]
    create: bool,
    #[arg(long, help = "Preview release diff without persisting")]
    preview: bool,
    #[arg(long, help = "Confirm release deployment")]
    deploy: bool,
    #[arg(long, help = "Confirm release removal")]
    remove: bool,
    #[arg(long, alias = "deploy_calldata", help = "Build deploy calldata")]
    deploy_calldata: bool,
    #[arg(long, alias = "remove_calldata", help = "Build remove calldata")]
    remove_calldata: bool,
    #[arg(
        long,
        alias = "backtest_progress",
        help = "Get release backtest/check progress"
    )]
    backtest_progress: bool,
    #[arg(long, alias = "retry_check", help = "Retry a failed release check")]
    retry_check: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(
    about = "List, inspect, create, preview, check, retry, deploy, or remove releases",
    after_help = "Examples:\n  pcl releases list <project-ref>\n  pcl releases show <project-ref> <release-id>\n  pcl releases preview <project-ref> --body-file release.json\n  pcl releases deploy <project-ref> <release-id> --body-file deploy.json\n  pcl releases calldata deploy <project-ref> <release-id> --signer-address <address>\n\nLegacy flag forms are still supported."
)]
pub struct ReleasesCommand {
    #[command(flatten)]
    globals: ApiWorkflowOptions,
    #[command(subcommand)]
    command: Option<ReleasesSubcommand>,
    #[command(flatten)]
    legacy: ReleasesArgs,
}

#[derive(clap::Subcommand, Debug)]
enum ReleasesSubcommand {
    #[command(about = "List releases for a project")]
    List(ReleaseProjectArgs),
    #[command(about = "Show one release")]
    Show(ReleaseRefArgs),
    #[command(about = "Create a release")]
    Create(ReleaseProjectBodyArgs),
    #[command(about = "Preview a release body without persisting")]
    Preview(ReleaseProjectBodyArgs),
    #[command(about = "Confirm release deployment")]
    Deploy(ReleaseBodyArgs),
    #[command(about = "Confirm release removal")]
    Remove(ReleaseBodyArgs),
    #[command(about = "Build release calldata")]
    Calldata(ReleaseCalldataArgs),
    #[command(
        name = "backtest-progress",
        about = "Show release backtest/check progress"
    )]
    BacktestProgress(ReleaseRefArgs),
    #[command(name = "retry-check", about = "Retry a failed release check")]
    RetryCheck(ReleaseRetryCheckArgs),
}

#[derive(clap::Args, Debug)]
struct ReleaseProjectArgs {
    #[arg(value_name = "PROJECT")]
    project: String,
}

#[derive(clap::Args, Debug)]
struct ReleaseRefArgs {
    #[arg(value_name = "PROJECT")]
    project: String,
    #[arg(value_name = "RELEASE_ID")]
    release_id: String,
}

#[derive(clap::Args, Debug)]
struct ReleaseProjectBodyArgs {
    #[arg(value_name = "PROJECT")]
    project: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

#[derive(clap::Args, Debug)]
struct ReleaseBodyArgs {
    #[arg(value_name = "PROJECT")]
    project: Option<String>,
    #[arg(value_name = "RELEASE_ID")]
    release_id: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

#[derive(clap::Args, Debug)]
struct ReleaseRetryCheckArgs {
    #[arg(value_name = "PROJECT")]
    project: Option<String>,
    #[arg(value_name = "RELEASE_ID")]
    release_id: Option<String>,
    #[arg(value_name = "CHECK_ID")]
    check_id: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

#[derive(clap::Args, Debug)]
struct ReleaseCalldataArgs {
    #[command(subcommand)]
    command: ReleaseCalldataSubcommand,
}

#[derive(clap::Subcommand, Debug)]
enum ReleaseCalldataSubcommand {
    #[command(about = "Build deploy calldata")]
    Deploy(ReleaseDeployCalldataArgs),
    #[command(about = "Build remove calldata")]
    Remove(ReleaseRefArgs),
}

#[derive(clap::Args, Debug)]
struct ReleaseDeployCalldataArgs {
    #[arg(value_name = "PROJECT")]
    project: String,
    #[arg(value_name = "RELEASE_ID")]
    release_id: String,
    #[arg(long, alias = "signer_address", help = "Signer address")]
    signer_address: String,
}

impl ReleasesCommand {
    pub async fn run(
        self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        let args = match self.command {
            Some(command) => command.into_args(),
            None => self.legacy,
        };
        self.globals
            .run(ApiCommand::Releases(args), config, cli_args, json_output)
            .await
    }
}

impl ReleasesSubcommand {
    fn into_args(self) -> ReleasesArgs {
        match self {
            Self::List(args) => release_project_args(Some(args.project)),
            Self::Show(args) => release_ref_args(args.project, args.release_id),
            Self::Create(args) => {
                let mut release_args = release_project_args(args.project);
                release_args.create = true;
                args.body.apply_to_release(&mut release_args);
                release_args
            }
            Self::Preview(args) => {
                let mut release_args = release_project_args(args.project);
                release_args.preview = true;
                args.body.apply_to_release(&mut release_args);
                release_args
            }
            Self::Deploy(args) => {
                let mut release_args = release_ref_args_optional(args.project, args.release_id);
                release_args.deploy = true;
                args.body.apply_to_release(&mut release_args);
                release_args
            }
            Self::Remove(args) => {
                let mut release_args = release_ref_args_optional(args.project, args.release_id);
                release_args.remove = true;
                args.body.apply_to_release(&mut release_args);
                release_args
            }
            Self::Calldata(args) => {
                match args.command {
                    ReleaseCalldataSubcommand::Deploy(args) => {
                        let mut release_args = release_ref_args(args.project, args.release_id);
                        release_args.deploy_calldata = true;
                        release_args.signer_address = Some(args.signer_address);
                        release_args
                    }
                    ReleaseCalldataSubcommand::Remove(args) => {
                        let mut release_args = release_ref_args(args.project, args.release_id);
                        release_args.remove_calldata = true;
                        release_args
                    }
                }
            }
            Self::BacktestProgress(args) => {
                let mut release_args = release_ref_args(args.project, args.release_id);
                release_args.backtest_progress = true;
                release_args
            }
            Self::RetryCheck(args) => {
                let mut release_args = release_ref_args_optional(args.project, args.release_id);
                release_args.retry_check = true;
                release_args.check_id = args.check_id;
                args.body.apply_to_release(&mut release_args);
                release_args
            }
        }
    }
}

fn release_project_args(project: Option<String>) -> ReleasesArgs {
    ReleasesArgs {
        project,
        ..ReleasesArgs::default()
    }
}

fn release_ref_args(project: impl Into<String>, release_id: impl Into<String>) -> ReleasesArgs {
    ReleasesArgs {
        project: Some(project.into()),
        release_id: Some(release_id.into()),
        ..ReleasesArgs::default()
    }
}

fn release_ref_args_optional(project: Option<String>, release_id: Option<String>) -> ReleasesArgs {
    ReleasesArgs {
        project,
        release_id,
        ..ReleasesArgs::default()
    }
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("deployments_action")
        .args(["confirm"])
        .multiple(false)
))]
struct DeploymentsArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(long, help = "Confirm deployment")]
    confirm: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug, Default)]
#[command(group(
    ArgGroup::new("access_action")
        .args(["members", "invitations", "pending", "preview", "accept", "invite", "resend", "revoke", "update_role", "remove", "my_role"])
        .multiple(false)
))]
struct AccessArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(long, alias = "member_user_id", help = "Member user ID")]
    member_user_id: Option<String>,
    #[arg(long, alias = "invitation_id", help = "Invitation ID")]
    invitation_id: Option<String>,
    #[arg(long, help = "Invitation token")]
    token: Option<String>,
    #[arg(long, help = "List members")]
    members: bool,
    #[arg(long, help = "List project invitations")]
    invitations: bool,
    #[arg(long, help = "List pending invitations for current user")]
    pending: bool,
    #[arg(long, help = "Preview invitation token")]
    preview: bool,
    #[arg(long, help = "Accept invitation token")]
    accept: bool,
    #[arg(long, help = "Create invitation")]
    invite: bool,
    #[arg(long, help = "Resend invitation")]
    resend: bool,
    #[arg(long, help = "Revoke invitation")]
    revoke: bool,
    #[arg(long, alias = "update_role", help = "Update member role")]
    update_role: bool,
    #[arg(long, help = "Remove member")]
    remove: bool,
    #[arg(long, alias = "my_role", help = "Get current user's project role")]
    my_role: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(
    about = "Manage members, roles, and invitations",
    after_help = "Examples:\n  pcl access members <project-ref>\n  pcl access invitations <project-ref>\n  pcl access pending\n  pcl access preview <token>\n  pcl access invite <project-ref> --body-file invite.json\n  pcl access role update <project-ref> <member-user-id> --field role=admin\n\nLegacy flag forms are still supported."
)]
pub struct AccessCommand {
    #[command(flatten)]
    globals: ApiWorkflowOptions,
    #[command(subcommand)]
    command: Option<AccessSubcommand>,
    #[command(flatten)]
    legacy: AccessArgs,
}

#[derive(clap::Subcommand, Debug)]
enum AccessSubcommand {
    #[command(about = "List project members")]
    Members(AccessProjectArgs),
    #[command(about = "List project invitations")]
    Invitations(AccessProjectArgs),
    #[command(about = "List pending invitations for the current user")]
    Pending,
    #[command(about = "Preview an invitation token")]
    Preview(AccessTokenArgs),
    #[command(about = "Accept an invitation token")]
    Accept(AccessTokenBodyArgs),
    #[command(about = "Invite a project member")]
    Invite(AccessProjectBodyArgs),
    #[command(about = "Resend a project invitation")]
    Resend(AccessInvitationArgs),
    #[command(about = "Revoke a project invitation")]
    Revoke(AccessInvitationArgs),
    #[command(about = "Manage project roles")]
    Role(AccessRoleArgs),
    #[command(about = "Manage project members")]
    Member(AccessMemberCommand),
    #[command(name = "my-role", about = "Show the current user's project role")]
    MyRole(AccessProjectArgs),
}

#[derive(clap::Args, Debug)]
struct AccessProjectArgs {
    #[arg(value_name = "PROJECT")]
    project: String,
}

#[derive(clap::Args, Debug)]
struct AccessTokenArgs {
    #[arg(value_name = "TOKEN")]
    token: String,
}

#[derive(clap::Args, Debug)]
struct AccessTokenBodyArgs {
    #[arg(value_name = "TOKEN")]
    token: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

#[derive(clap::Args, Debug)]
struct AccessProjectBodyArgs {
    #[arg(value_name = "PROJECT")]
    project: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

#[derive(clap::Args, Debug)]
struct AccessInvitationArgs {
    #[arg(value_name = "PROJECT")]
    project: Option<String>,
    #[arg(value_name = "INVITATION_ID")]
    invitation_id: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

#[derive(clap::Args, Debug)]
struct AccessRoleArgs {
    #[command(subcommand)]
    command: AccessRoleSubcommand,
}

#[derive(clap::Subcommand, Debug)]
enum AccessRoleSubcommand {
    #[command(about = "Update a project member role")]
    Update(AccessMemberBodyArgs),
}

#[derive(clap::Args, Debug)]
struct AccessMemberCommand {
    #[command(subcommand)]
    command: AccessMemberSubcommand,
}

#[derive(clap::Subcommand, Debug)]
enum AccessMemberSubcommand {
    #[command(about = "Remove a project member")]
    Remove(AccessMemberBodyArgs),
}

#[derive(clap::Args, Debug)]
struct AccessMemberBodyArgs {
    #[arg(value_name = "PROJECT")]
    project: Option<String>,
    #[arg(value_name = "MEMBER_USER_ID")]
    member_user_id: Option<String>,
    #[command(flatten)]
    body: WorkflowBodyArgs,
}

impl AccessCommand {
    pub async fn run(
        self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        let args = match self.command {
            Some(command) => command.into_args(),
            None => self.legacy,
        };
        self.globals
            .run(ApiCommand::Access(args), config, cli_args, json_output)
            .await
    }
}

impl AccessSubcommand {
    fn into_args(self) -> AccessArgs {
        match self {
            Self::Members(args) => {
                AccessArgs {
                    project: Some(args.project),
                    members: true,
                    ..AccessArgs::default()
                }
            }
            Self::Invitations(args) => {
                AccessArgs {
                    project: Some(args.project),
                    invitations: true,
                    ..AccessArgs::default()
                }
            }
            Self::Pending => {
                AccessArgs {
                    pending: true,
                    ..AccessArgs::default()
                }
            }
            Self::Preview(args) => {
                AccessArgs {
                    token: Some(args.token),
                    preview: true,
                    ..AccessArgs::default()
                }
            }
            Self::Accept(args) => {
                let mut access_args = AccessArgs {
                    token: args.token,
                    accept: true,
                    ..AccessArgs::default()
                };
                args.body.apply_to_access(&mut access_args);
                access_args
            }
            Self::Invite(args) => {
                let mut access_args = AccessArgs {
                    project: args.project,
                    invite: true,
                    ..AccessArgs::default()
                };
                args.body.apply_to_access(&mut access_args);
                access_args
            }
            Self::Resend(args) => {
                let mut access_args = access_invitation_args(args.project, args.invitation_id);
                access_args.resend = true;
                args.body.apply_to_access(&mut access_args);
                access_args
            }
            Self::Revoke(args) => {
                let mut access_args = access_invitation_args(args.project, args.invitation_id);
                access_args.revoke = true;
                args.body.apply_to_access(&mut access_args);
                access_args
            }
            Self::Role(args) => {
                match args.command {
                    AccessRoleSubcommand::Update(args) => {
                        let mut access_args = access_member_args(args.project, args.member_user_id);
                        access_args.update_role = true;
                        args.body.apply_to_access(&mut access_args);
                        access_args
                    }
                }
            }
            Self::Member(args) => {
                match args.command {
                    AccessMemberSubcommand::Remove(args) => {
                        let mut access_args = access_member_args(args.project, args.member_user_id);
                        access_args.remove = true;
                        args.body.apply_to_access(&mut access_args);
                        access_args
                    }
                }
            }
            Self::MyRole(args) => {
                AccessArgs {
                    project: Some(args.project),
                    my_role: true,
                    ..AccessArgs::default()
                }
            }
        }
    }
}

fn access_invitation_args(project: Option<String>, invitation_id: Option<String>) -> AccessArgs {
    AccessArgs {
        project,
        invitation_id,
        ..AccessArgs::default()
    }
}

fn access_member_args(project: Option<String>, member_user_id: Option<String>) -> AccessArgs {
    AccessArgs {
        project,
        member_user_id,
        ..AccessArgs::default()
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum IntegrationProvider {
    Slack,
    Pagerduty,
}

impl IntegrationProvider {
    fn path(self) -> &'static str {
        match self {
            Self::Slack => "slack",
            Self::Pagerduty => "pagerduty",
        }
    }
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("integrations_action")
        .args(["configure", "test", "delete"])
        .multiple(false)
))]
struct IntegrationsArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(long, value_enum, help = "Integration provider")]
    provider: Option<IntegrationProvider>,
    #[arg(long, help = "Configure integration")]
    configure: bool,
    #[arg(long, help = "Test integration")]
    test: bool,
    #[arg(long, help = "Delete integration")]
    delete: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("protocol_manager_action")
        .args(["nonce", "set", "clear", "transfer_calldata", "accept_calldata", "pending_transfer", "confirm_transfer"])
        .multiple(false)
))]
struct ProtocolManagerArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(long, help = "Get nonce")]
    nonce: bool,
    #[arg(long, help = "Set protocol manager")]
    set: bool,
    #[arg(long, help = "Clear protocol manager")]
    clear: bool,
    #[arg(long, alias = "transfer_calldata", help = "Get transfer calldata")]
    transfer_calldata: bool,
    #[arg(long, alias = "accept_calldata", help = "Get accept calldata")]
    accept_calldata: bool,
    #[arg(long, alias = "pending_transfer", help = "Get pending transfer")]
    pending_transfer: bool,
    #[arg(long, alias = "confirm_transfer", help = "Confirm transfer")]
    confirm_transfer: bool,
    #[arg(
        long,
        alias = "new_manager",
        help = "New manager address for transfer calldata"
    )]
    new_manager: Option<String>,
    #[arg(long, help = "Address for --nonce")]
    address: Option<String>,
    #[arg(long, alias = "chain_id", help = "Chain ID for --nonce")]
    chain_id: Option<u64>,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("transfers_action")
        .args(["pending", "transfer_id", "reject"])
        .multiple(false)
))]
struct TransfersArgs {
    #[arg(long, alias = "transfer_id", help = "Transfer ID")]
    transfer_id: Option<String>,
    #[arg(long, help = "List pending transfers")]
    pending: bool,
    #[arg(long, help = "Reject an incoming transfer")]
    reject: bool,
    #[arg(long, help = "JSON request body")]
    body: Option<String>,
    #[arg(long = "field", help = "Extra JSON body field as KEY=VALUE")]
    field: Vec<String>,
    #[arg(
        long = "body-file",
        conflicts_with = "body",
        help = "Path to JSON body, or - for stdin"
    )]
    body_file: Option<PathBuf>,
    #[arg(long, alias = "body_template", help = "Print a JSON body template")]
    body_template: bool,
}

#[derive(clap::Args, Debug)]
struct EventsArgs {
    #[arg(
        long,
        visible_alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: Option<String>,
    #[arg(
        long,
        alias = "audit_log",
        help = "Return audit log instead of project events"
    )]
    audit_log: bool,
    #[arg(long, help = "Page number")]
    page: Option<u64>,
    #[arg(long, help = "Items per page")]
    limit: Option<u64>,
    #[arg(long, help = "Environment filter")]
    environment: Option<String>,
}

top_level_workflow_command!(
    IncidentsCommand,
    IncidentsArgs,
    Incidents,
    "List, inspect, export, and retry incidents",
    "Examples:\n  pcl incidents --limit 5\n  pcl incidents --project-id <project-id> --environment production\n  pcl incidents --project-id <project-id> --all --limit 50 --output incidents.json\n  pcl incidents --incident-id <incident-id>\n  pcl incidents --incident-id <incident-id> --tx-id <tx-id>\n  pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace\n\nCompatibility alias:\n  pcl api incidents ..."
);

top_level_workflow_command!(
    AssertionsCommand,
    AssertionsArgs,
    Assertions,
    "List, inspect, and manage assertions",
    "Examples:\n  pcl assertions --project-id <project-ref>\n  pcl assertions --adopter-address 0x... --network 1\n  pcl assertions --project-id <project-ref> --registered\n  pcl assertions --project-id <project-ref> --remove-info\n\nCompatibility alias:\n  pcl api assertions ..."
);

top_level_workflow_command!(
    SearchCommand,
    SearchArgs,
    Search,
    "Search and inspect platform-wide metadata",
    "Examples:\n  pcl search --query settler\n  pcl search --stats\n  pcl search --system-status\n  pcl search --verified-contract --address 0x... --chain-id 1\n\nCompatibility alias:\n  pcl api search ..."
);

top_level_workflow_command!(
    AccountCommand,
    AccountArgs,
    Account,
    "Inspect and manage current account onboarding state",
    "Examples:\n  pcl account\n  pcl account --accept-terms\n  pcl account --logout\n\nCompatibility alias:\n  pcl api account ..."
);

top_level_workflow_command!(
    ContractsCommand,
    ContractsArgs,
    Contracts,
    "List or manage project contracts and assertion adopters",
    "Examples:\n  pcl contracts --project <project-ref>\n  pcl contracts --project <project-ref> --adopter-id <adopter-id>\n  pcl contracts --unassigned --manager <manager-address>\n  pcl contracts --create --body-template\n\nCompatibility alias:\n  pcl api contracts ..."
);

top_level_workflow_command!(
    DeploymentsCommand,
    DeploymentsArgs,
    Deployments,
    "Inspect deployments and confirm deployed assertions",
    "Examples:\n  pcl deployments --project <project-ref>\n  pcl deployments --project <project-ref> --confirm --body-template\n\nCompatibility alias:\n  pcl api deployments ..."
);

top_level_workflow_command!(
    IntegrationsCommand,
    IntegrationsArgs,
    Integrations,
    "Manage Slack and PagerDuty integrations",
    "Examples:\n  pcl integrations --project <project-ref> --provider slack\n  pcl integrations --project <project-ref> --provider pagerduty --configure --body-template\n  pcl integrations --project <project-ref> --provider slack --test\n\nCompatibility alias:\n  pcl api integrations ..."
);

top_level_workflow_command!(
    ProtocolManagerCommand,
    ProtocolManagerArgs,
    ProtocolManager,
    "Manage project protocol manager settings",
    "Examples:\n  pcl protocol-manager --project <project-ref> --nonce --address <manager-address>\n  pcl protocol-manager --project <project-ref> --transfer-calldata --new-manager 0x...\n  pcl protocol-manager --project <project-ref> --set --body-template\n\nCompatibility alias:\n  pcl api protocol-manager ..."
);

top_level_workflow_command!(
    TransfersCommand,
    TransfersArgs,
    Transfers,
    "Inspect or reject protocol manager transfers",
    "Examples:\n  pcl transfers --pending\n  pcl transfers --transfer-id <transfer-id>\n  pcl transfers --reject --body-template\n\nCompatibility alias:\n  pcl api transfers ..."
);

top_level_workflow_command!(
    EventsCommand,
    EventsArgs,
    Events,
    "Inspect project events and audit logs",
    "Examples:\n  pcl events --project <project-ref>\n  pcl events --project <project-ref> --audit-log\n\nCompatibility alias:\n  pcl api events ..."
);

impl ApiArgs {
    pub async fn run(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        let request_log_path = crate::request_log::request_log_path_for_args(cli_args);
        match &self.command {
            ApiCommand::Incidents(args) => {
                let output = self
                    .run_incidents(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Projects(args) => {
                let output = self
                    .run_projects(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Assertions(args) => {
                let output = self
                    .run_assertions(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Search(args) => {
                let output = self
                    .run_search(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Account(args) => {
                if args.body_template {
                    let output = template_envelope(body_template("empty_object"));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_workflow(
                        config,
                        cli_args,
                        "account",
                        account_request(args)?,
                        &request_log_path,
                    )
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Contracts(args) => {
                if args.body_template {
                    let output = template_envelope(contracts_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_contracts(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Releases(args) => {
                if args.body_template {
                    let output = template_envelope(release_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_releases(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Deployments(args) => {
                if args.body_template {
                    let output = template_envelope(deployment_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_deployments(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Access(args) => {
                if args.body_template {
                    let output = template_envelope(access_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_workflow(
                        config,
                        cli_args,
                        "access",
                        access_request(args)?,
                        &request_log_path,
                    )
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Integrations(args) => {
                if args.body_template {
                    let output = template_envelope(integration_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_workflow(
                        config,
                        cli_args,
                        "integrations",
                        integrations_request(args)?,
                        &request_log_path,
                    )
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::ProtocolManager(args) => {
                if args.body_template {
                    let output = template_envelope(protocol_manager_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_protocol_manager(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Transfers(args) => {
                if args.body_template {
                    let output = template_envelope(transfer_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_transfers(config, cli_args, args, &request_log_path)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Events(args) => {
                let output = self
                    .run_workflow(
                        config,
                        cli_args,
                        "events",
                        events_request(args)?,
                        &request_log_path,
                    )
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Manifest => {
                let output = ok_envelope(api_manifest());
                print_output(&output, json_output)?;
            }
            ApiCommand::List { filter, method } => {
                let spec = self.fetch_openapi(config).await?;
                let operations = list_operations(&spec, filter.as_deref(), *method)?;
                let next_actions = next_actions_for_operations(&operations);
                let output = json!({
                    "status": "ok",
                    "data": {
                        "operations": operations,
                    },
                    "next_actions": next_actions,
                });
                print_output(&output, json_output)?;
            }
            ApiCommand::Inspect {
                operation,
                path,
                full,
            } => {
                let spec = self.fetch_openapi(config).await?;
                let inspected = inspect_operation(&spec, operation, path.as_deref(), *full)?;
                let next_actions = command_next_actions(&inspected);
                let output = json!({
                    "status": "ok",
                    "data": inspected,
                    "next_actions": next_actions,
                });
                print_output(&output, json_output)?;
            }
            ApiCommand::Coverage { records, markdown } => {
                let spec = self.fetch_openapi(config).await?;
                let coverage =
                    api_coverage(&spec, &request_log_path, *records, self.api_url.as_str())?;
                if let Some(path) = markdown {
                    write_api_coverage_markdown(path, &coverage)?;
                }
                let output = json!({
                    "status": "ok",
                    "data": coverage,
                    "next_actions": [
                        "pcl requests list --toon",
                        "pcl api list --toon",
                        "pcl api coverage --markdown api-coverage.md",
                    ],
                });
                print_output(&output, json_output)?;
            }
            ApiCommand::Call {
                method,
                path,
                query,
                header,
                body,
                body_file,
                field,
                paginate,
                all: _,
                page,
                limit,
                page_param,
                limit_param,
                max_pages,
                jsonl,
                output,
            } => {
                if *jsonl && output.is_none() {
                    return Err(ApiCommandError::InvalidWorkflow {
                        message: "--jsonl requires --output".to_string(),
                    });
                }
                let input = ApiRequestInput {
                    method: *method,
                    path,
                    query,
                    header,
                    body: body.as_deref(),
                    body_file: body_file.as_ref(),
                    field,
                    require_auth: self.raw_call_requires_auth(*method, path)?,
                };
                let pagination = paginate.as_ref().map(|item_field| {
                    RawPaginationOptions {
                        item_field,
                        start_page: page.unwrap_or(1),
                        limit: limit.unwrap_or(50),
                        page_param: page_param.as_deref().unwrap_or("page"),
                        limit_param: limit_param.as_deref().unwrap_or("limit"),
                        max_pages: max_pages.unwrap_or(100),
                    }
                });
                if self.dry_run {
                    let output = dry_run_envelope(self.raw_call_plan(input, pagination, config)?);
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let (mut response, next_actions) = if let Some(pagination) = pagination {
                    let response = self
                        .call_api_paginated(config, cli_args, input, pagination, &request_log_path)
                        .await?;
                    (
                        response,
                        vec![
                            "Adjust --limit or --max-pages if the result set was truncated"
                                .to_string(),
                            "Use --output results.json to save paginated data".to_string(),
                            "pcl api manifest --toon".to_string(),
                        ],
                    )
                } else {
                    let response = self
                        .call_api(config, cli_args, input, &request_log_path)
                        .await?;
                    (
                        response,
                        vec![
                            "pcl api list --toon".to_string(),
                            "pcl api manifest --toon".to_string(),
                        ],
                    )
                };
                if let Some(path) = output {
                    if *jsonl {
                        write_jsonl_items_output_file(path, &response)?;
                    } else {
                        let body = response.pointer("/response/body").unwrap_or(&response);
                        write_json_output_file(path, body)?;
                    }
                    if let Some(object) = response.as_object_mut() {
                        object.insert("output_path".to_string(), json!(path.display().to_string()));
                    }
                }
                let output = json!({
                    "status": "ok",
                    "data": response,
                    "next_actions": next_actions,
                });
                print_output(&output, json_output)?;
            }
        }

        Ok(())
    }

    async fn call_api_paginated(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        input: ApiRequestInput<'_>,
        pagination: RawPaginationOptions<'_>,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if input.method.openapi_key() != "get" {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--paginate is only supported for GET requests".to_string(),
            });
        }
        if input.body.is_some() || input.body_file.is_some() || !input.field.is_empty() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--paginate cannot be used with request bodies".to_string(),
            });
        }
        if pagination.limit == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--limit must be greater than zero".to_string(),
            });
        }
        if pagination.max_pages == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--max-pages must be greater than zero".to_string(),
            });
        }

        let (path, mut base_query) = split_path_and_inline_query(input.path)?;
        base_query.extend(parse_key_values("query", input.query)?);
        let url = self.api_url(&path)?;
        let headers = parse_headers(input.header)?;
        let operation_id = self.resolve_operation_id(config, input.method, &path).await;
        self.ensure_request_auth(config, cli_args, input.require_auth)
            .await?;
        let client = self.http_client(
            config,
            input.require_auth && !self.allow_unauthenticated,
            input.require_auth && !self.allow_unauthenticated,
        )?;

        let mut items = Vec::new();
        let mut pages_fetched = 0_u64;
        let mut last_page_count = 0_usize;

        for offset in 0..pagination.max_pages {
            let page = pagination.start_page + offset;
            let mut page_query = base_query.clone();
            upsert_query(&mut page_query, pagination.page_param, page.to_string());
            upsert_query(
                &mut page_query,
                pagination.limit_param,
                pagination.limit.to_string(),
            );

            let response = client
                .get(url.clone())
                .headers(headers.clone())
                .query(&page_query)
                .send()
                .await?;
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
            write_request_log(
                request_log_path,
                "raw_paginated",
                input.method.as_str(),
                &path,
                status.as_u16(),
                request_id.as_deref(),
                operation_id.as_deref(),
            );
            if !status.is_success() {
                return Err(ApiCommandError::HttpStatus {
                    method: input.method.as_str(),
                    path,
                    status: status.as_u16(),
                    request_id,
                    body: Box::new(body),
                });
            }

            let page_items =
                extract_paginated_items(&body, pagination.item_field).ok_or_else(|| {
                ApiCommandError::InvalidWorkflow {
                    message: format!(
                        "Could not find an array at `{}` or common pagination fields in response",
                        pagination.item_field
                    ),
                }
            })?;
            last_page_count = page_items.len();
            pages_fetched += 1;
            items.extend(page_items);

            if last_page_count < usize::try_from(pagination.limit).unwrap_or(usize::MAX) {
                break;
            }
        }

        let count = items.len();
        Ok(json!({
            "request": {
                "method": input.method.as_str(),
                "path": path,
                "operation_id": operation_id,
                "query": query_pairs_value(&base_query),
                "pagination": {
                    "field": pagination.item_field,
                    "start_page": pagination.start_page,
                    "limit": pagination.limit,
                    "page_param": pagination.page_param,
                    "limit_param": pagination.limit_param,
                    "max_pages": pagination.max_pages,
                }
            },
            "items": items,
            "count": count,
            "pages_fetched": pages_fetched,
            "last_page_count": last_page_count,
        }))
    }

    async fn run_incidents(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &IncidentsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = incidents_request(args)?;
        if args.jsonl && args.output.is_none() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--jsonl requires --output".to_string(),
            });
        }
        if self.dry_run {
            let pagination = args.all.then(|| {
                json!({
                    "enabled": true,
                    "item_field": "incidents",
                    "start_page": args.page.unwrap_or(1),
                    "limit": args.limit.unwrap_or(50),
                    "max_pages": args.max_pages.unwrap_or(100),
                    "output": args.output.as_ref().map(|path| path.display().to_string()),
                    "jsonl": args.jsonl,
                })
            });
            return Ok(dry_run_envelope(
                self.workflow_request_plan(&request, pagination, config),
            ));
        }
        if args.all {
            let mut data = self
                .call_workflow_paginated(
                    config,
                    cli_args,
                    request.clone(),
                    WorkflowPaginationOptions {
                        item_field: "incidents",
                        start_page: args.page.unwrap_or(1),
                        limit: args.limit.unwrap_or(50),
                        max_pages: args.max_pages.unwrap_or(100),
                    },
                    request_log_path,
                )
                .await?;
            if let Some(path) = &args.output {
                if args.jsonl {
                    write_jsonl_items_output_file(path, &data)?;
                } else {
                    write_json_output_file(path, &data)?;
                }
                if let Some(object) = data.as_object_mut() {
                    object.insert("output_path".to_string(), json!(path.display().to_string()));
                }
            }
            let mut next_actions = request.next_actions;
            if args.output.is_none() {
                next_actions.insert(
                    0,
                    "Use --output incidents.json to save large paginated results".to_string(),
                );
            }
            return Ok(json!({
                "status": "ok",
                "data": data,
                "next_actions": next_actions,
            }));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = incidents_next_actions(&result.body, args, request.next_actions);
        Ok(workflow_success_envelope(result, next_actions))
    }

    async fn run_projects(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProjectsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(project_body_template(args)));
        }
        let request = projects_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "projects",
            request,
            request_log_path,
            projects_next_actions,
        )
        .await
    }

    async fn run_assertions(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &AssertionsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(body_template("empty_object")));
        }
        let request = assertions_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "assertions",
            request,
            request_log_path,
            |data, fallback| assertions_next_actions(data, args, fallback),
        )
        .await
    }

    async fn run_search(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &SearchArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = search_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "search",
            request,
            request_log_path,
            search_next_actions,
        )
        .await
    }

    async fn run_contracts(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ContractsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = contracts_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "contracts",
            request,
            request_log_path,
            |data, fallback| contracts_next_actions(data, args, fallback),
        )
        .await
    }

    async fn run_releases(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ReleasesArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = releases_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "releases",
            request,
            request_log_path,
            |data, fallback| releases_next_actions(data, args, fallback),
        )
        .await
    }

    async fn run_deployments(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &DeploymentsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = deployments_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "deployments",
            request,
            request_log_path,
            |_data, fallback| fallback,
        )
        .await
    }

    async fn run_transfers(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &TransfersArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = transfers_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "transfers",
            request,
            request_log_path,
            |data, fallback| transfers_next_actions(data, args, fallback),
        )
        .await
    }

    async fn run_protocol_manager(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProtocolManagerArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = protocol_manager_request(args)?;
        self.run_prepared_workflow(
            config,
            cli_args,
            "protocol-manager",
            request,
            request_log_path,
            |data, fallback| protocol_manager_next_actions(data, args, fallback),
        )
        .await
    }

    async fn run_workflow(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        workflow: &'static str,
        request: WorkflowRequest,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        self.run_prepared_workflow(
            config,
            cli_args,
            workflow,
            request,
            request_log_path,
            |_data, fallback| fallback,
        )
        .await
    }

    async fn run_prepared_workflow<F>(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        workflow: &'static str,
        request: WorkflowRequest,
        request_log_path: &Path,
        next_actions_for: F,
    ) -> Result<Value, ApiCommandError>
    where
        F: FnOnce(&Value, Vec<String>) -> Vec<String>,
    {
        if self.dry_run {
            return Ok(dry_run_envelope(
                self.workflow_request_plan(&request, None, config),
            ));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = next_actions_for(&result.body, request.next_actions);
        let data = workflow_data_for_output_mode(workflow, &result.body, cli_args.output_mode());
        Ok(workflow_success_envelope_with_data(
            result,
            data,
            next_actions,
        ))
    }

    fn workflow_request_plan(
        &self,
        request: &WorkflowRequest,
        pagination: Option<Value>,
        config: &CliConfig,
    ) -> Value {
        let body = request.body.as_deref().map_or(Ok(Value::Null), |body| {
            serde_json::from_str(body).map_err(ApiCommandError::Json)
        });
        let body = match body {
            Ok(body) => body,
            Err(error) => {
                return json!({
                    "dry_run": true,
                    "valid": false,
                    "error": {
                        "code": error.code(),
                        "message": error.to_string(),
                    },
                });
            }
        };

        let destructive = request_is_destructive(request.method, &request.path);
        json!({
            "dry_run": true,
            "valid": true,
            "request": {
                "method": request.method.as_str(),
                "path": request.path.as_str(),
                "query": query_pairs_value(&request.query),
                "body": body,
                "auth": self.auth_plan(request.require_auth, request.attach_auth, config),
                "side_effecting": request.method != HttpMethod::Get,
                "destructive": destructive,
                "project_resolution": "not_performed",
            },
            "pagination": pagination,
        })
    }

    fn raw_call_plan(
        &self,
        input: ApiRequestInput<'_>,
        pagination: Option<RawPaginationOptions<'_>>,
        config: &CliConfig,
    ) -> Result<Value, ApiCommandError> {
        let (path, mut query) = split_path_and_inline_query(input.path)?;
        query.extend(parse_key_values("query", input.query)?);
        let header = parse_key_values("header", input.header)?;
        let body = request_body(input.body, input.body_file, input.field)?
            .map(|body| serde_json::from_str::<Value>(&body))
            .transpose()?;
        let destructive = request_is_destructive(input.method, &path);

        Ok(json!({
            "dry_run": true,
            "valid": true,
            "request": {
                "method": input.method.as_str(),
                "path": path.as_str(),
                "query": query_pairs_value(&query),
                "headers": query_pairs_value(&header),
                "body": body.unwrap_or(Value::Null),
                "auth": self.auth_plan(input.require_auth, input.require_auth, config),
                "side_effecting": input.method != HttpMethod::Get,
                "destructive": destructive,
            },
            "pagination": pagination.map(|pagination| json!({
                "enabled": true,
                "item_field": pagination.item_field,
                "start_page": pagination.start_page,
                "limit": pagination.limit,
                "page_param": pagination.page_param,
                "limit_param": pagination.limit_param,
                "max_pages": pagination.max_pages,
            })),
        }))
    }

    fn auth_plan(&self, require_auth: bool, attach_auth: bool, config: &CliConfig) -> Value {
        let now = chrono::Utc::now();
        let stored_token_present = config
            .auth
            .as_ref()
            .is_some_and(|auth| !auth.access_token.trim().is_empty());
        let stored_token_valid = config
            .auth
            .as_ref()
            .is_some_and(|auth| !auth.access_token.trim().is_empty() && auth.expires_at > now);
        let will_attach_stored_token =
            attach_auth && !self.allow_unauthenticated && stored_token_valid;
        json!({
            "required": require_auth,
            "will_attach_stored_token": will_attach_stored_token,
            "stored_token_present": stored_token_present,
            "stored_token_valid": stored_token_valid,
            "allow_unauthenticated": self.allow_unauthenticated,
        })
    }

    fn raw_call_requires_auth(
        &self,
        method: HttpMethod,
        path: &str,
    ) -> Result<bool, ApiCommandError> {
        if self.allow_unauthenticated {
            return Ok(false);
        }
        let (path, _) = split_path_and_inline_query(path)?;
        Ok(!public_raw_call_path(method, &path))
    }

    async fn fetch_openapi(&self, config: &CliConfig) -> Result<Value, ApiCommandError> {
        let url = self.api_url("/openapi")?;
        let request = self.http_client(config, false, false)?.get(url);
        let response = request.send().await?;
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
        if !status.is_success() {
            return Err(ApiCommandError::HttpStatus {
                method: "GET",
                path: "/openapi".to_string(),
                status: status.as_u16(),
                request_id,
                body: Box::new(body),
            });
        }
        Ok(body)
    }

    async fn try_refresh_after_401(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
    ) -> Result<bool, ApiCommandError> {
        if !self.refresh_after_401.get() {
            return Ok(false);
        }

        match refresh_stored_auth(config, &self.api_url, cli_args, true).await {
            Ok(_) => Ok(true),
            Err(AuthError::RefreshEndpointNotFound { .. }) => {
                self.refresh_after_401.set(false);
                Ok(false)
            }
            Err(error) => Err(ApiCommandError::AuthRefresh(error)),
        }
    }

    async fn call_api(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        input: ApiRequestInput<'_>,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let (path, mut query) = split_path_and_inline_query(input.path)?;
        query.extend(parse_key_values("query", input.query)?);
        let url = self.api_url(&path)?;
        let headers = parse_headers(input.header)?;
        let body = request_body(input.body, input.body_file, input.field)?;
        let operation_id = self.resolve_operation_id(config, input.method, &path).await;
        let requires_auth = input.require_auth && !self.allow_unauthenticated;
        self.ensure_request_auth(config, cli_args, input.require_auth)
            .await?;

        let json_body = body
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()?;
        let mut response = self
            .send_api_request(
                config,
                PreparedApiRequest {
                    attach_auth: requires_auth,
                    method: input.method,
                    url: &url,
                    headers: &headers,
                    query: &query,
                    body: json_body.as_ref(),
                },
            )
            .await?;
        let mut status = response.status();
        let request_id = request_id_from_headers(response.headers());
        let response_headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), json!(value)))
            })
            .collect::<serde_json::Map<_, _>>();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.bytes().await?;
        let mut body = response_body_value(&content_type, &bytes);
        write_request_log(
            request_log_path,
            "raw",
            input.method.as_str(),
            &path,
            status.as_u16(),
            request_id.as_deref(),
            operation_id.as_deref(),
        );
        if status.as_u16() == 401
            && requires_auth
            && self.try_refresh_after_401(config, cli_args).await?
        {
            response = self
                .send_api_request(
                    config,
                    PreparedApiRequest {
                        attach_auth: requires_auth,
                        method: input.method,
                        url: &url,
                        headers: &headers,
                        query: &query,
                        body: json_body.as_ref(),
                    },
                )
                .await?;
            status = response.status();
            let retry_request_id = request_id_from_headers(response.headers());
            let retry_headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.as_str().to_string(), json!(value)))
                })
                .collect::<serde_json::Map<_, _>>();
            let retry_content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let retry_bytes = response.bytes().await?;
            body = response_body_value(&retry_content_type, &retry_bytes);
            write_request_log(
                request_log_path,
                "raw_retry_after_refresh",
                input.method.as_str(),
                &path,
                status.as_u16(),
                retry_request_id.as_deref(),
                operation_id.as_deref(),
            );
            if !status.is_success() {
                return Err(ApiCommandError::HttpStatus {
                    method: input.method.as_str(),
                    path,
                    status: status.as_u16(),
                    request_id: retry_request_id,
                    body: Box::new(body),
                });
            }
            return Ok(json!({
                "request": {
                    "method": input.method.as_str(),
                    "path": path,
                    "operation_id": operation_id,
                    "query": query_pairs_value(&query),
                    "retried_after_refresh": true,
                },
                "response": {
                    "status": status.as_u16(),
                    "success": status.is_success(),
                    "request_id": retry_request_id,
                    "headers": retry_headers,
                    "body": body,
                }
            }));
        }
        if !status.is_success() {
            return Err(ApiCommandError::HttpStatus {
                method: input.method.as_str(),
                path,
                status: status.as_u16(),
                request_id,
                body: Box::new(body),
            });
        }

        Ok(json!({
            "request": {
                "method": input.method.as_str(),
                "path": path,
                "operation_id": operation_id,
                "query": query_pairs_value(&query),
            },
            "response": {
                "status": status.as_u16(),
                "success": status.is_success(),
                "request_id": request_id,
                "headers": response_headers,
                "body": body,
            }
        }))
    }

    async fn call_workflow_result(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        request: &WorkflowRequest,
        request_log_path: &Path,
    ) -> Result<WorkflowCallResult, ApiCommandError> {
        let requires_auth = request.require_auth && !self.allow_unauthenticated;
        self.ensure_request_auth(config, cli_args, request.require_auth)
            .await?;
        let attach_auth = self.workflow_attach_auth(request, config);
        let path = self
            .normalize_project_path(
                config,
                &request.path,
                attach_auth,
                requires_auth,
                request_log_path,
            )
            .await?;
        let url = self.api_url(&path)?;
        let json_body = if let Some(body) = &request.body {
            Some(
                self.normalize_request_body(
                    config,
                    &path,
                    body,
                    attach_auth,
                    requires_auth,
                    request_log_path,
                )
                .await?,
            )
        } else {
            None
        };
        let mut response = self
            .send_workflow_request(config, request, &url, json_body.as_ref())
            .await?;
        let mut status = response.status();
        let mut request_id = request_id_from_headers(response.headers());
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.bytes().await?;
        let mut body = response_body_value(&content_type, &bytes);
        write_request_log(
            request_log_path,
            "workflow",
            request.method.as_str(),
            &path,
            status.as_u16(),
            request_id.as_deref(),
            None,
        );
        let mut retried_after_refresh = false;
        if status.as_u16() == 401
            && requires_auth
            && self.try_refresh_after_401(config, cli_args).await?
        {
            response = self
                .send_workflow_request(config, request, &url, json_body.as_ref())
                .await?;
            status = response.status();
            request_id = request_id_from_headers(response.headers());
            let retry_content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let retry_bytes = response.bytes().await?;
            body = response_body_value(&retry_content_type, &retry_bytes);
            retried_after_refresh = true;
            write_request_log(
                request_log_path,
                "workflow_retry_after_refresh",
                request.method.as_str(),
                &path,
                status.as_u16(),
                request_id.as_deref(),
                None,
            );
        }
        if !status.is_success() {
            return Err(ApiCommandError::HttpStatus {
                method: request.method.as_str(),
                path,
                status: status.as_u16(),
                request_id,
                body: Box::new(body),
            });
        }
        Ok(WorkflowCallResult {
            body,
            request: json!({
                "method": request.method.as_str(),
                "path": path,
                "query": query_pairs_value(&request.query),
                "auth": self.auth_plan(request.require_auth, request.attach_auth, config),
                "side_effecting": request.method != HttpMethod::Get,
                "destructive": request_is_destructive(request.method, &request.path),
                "retried_after_refresh": retried_after_refresh,
            }),
            response: json!({
                "status": status.as_u16(),
                "success": true,
                "request_id": request_id,
                "fetched_at": chrono::Utc::now().to_rfc3339(),
            }),
        })
    }

    async fn call_workflow_paginated(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        request: WorkflowRequest,
        pagination: WorkflowPaginationOptions<'_>,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if request.method.openapi_key() != "get" {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--all is only supported for GET list workflows".to_string(),
            });
        }
        if pagination.limit == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--limit must be greater than zero".to_string(),
            });
        }
        if pagination.max_pages == 0 {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--max-pages must be greater than zero".to_string(),
            });
        }

        let mut items = Vec::new();
        let mut pages_fetched = 0_u64;
        let mut last_page_count = 0_usize;

        for offset in 0..pagination.max_pages {
            let page = pagination.start_page + offset;
            let mut page_request = request.clone();
            upsert_query(&mut page_request.query, "page", page.to_string());
            upsert_query(
                &mut page_request.query,
                "limit",
                pagination.limit.to_string(),
            );
            let data = self
                .call_workflow_result(config, cli_args, &page_request, request_log_path)
                .await?
                .body;
            let page_items =
                extract_paginated_items(&data, pagination.item_field).ok_or_else(|| {
                ApiCommandError::InvalidWorkflow {
                    message: format!(
                        "Could not find an array at `{}` or common pagination fields in response",
                        pagination.item_field
                    ),
                }
            })?;
            last_page_count = page_items.len();
            pages_fetched += 1;
            items.extend(page_items);

            if last_page_count < usize::try_from(pagination.limit).unwrap_or(usize::MAX) {
                break;
            }
        }

        let count = items.len();
        Ok(json!({
            "items": items,
            "count": count,
            "pages_fetched": pages_fetched,
            "start_page": pagination.start_page,
            "limit": pagination.limit,
            "max_pages": pagination.max_pages,
            "last_page_count": last_page_count,
        }))
    }

    async fn normalize_request_body(
        &self,
        config: &CliConfig,
        path: &str,
        body: &str,
        attach_auth: bool,
        require_auth: bool,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let mut json_body: Value = serde_json::from_str(body)?;
        if path == "/projects/saved"
            && let Some(project_ref) = json_body.get("project_id").and_then(Value::as_str)
            && project_ref.parse::<uuid::Uuid>().is_err()
        {
            let project_id = self
                .resolve_project_id(
                    config,
                    project_ref,
                    attach_auth,
                    require_auth,
                    request_log_path,
                )
                .await?;
            if let Some(object) = json_body.as_object_mut() {
                object.insert("project_id".to_string(), Value::String(project_id));
            }
        }
        Ok(json_body)
    }

    async fn normalize_project_path(
        &self,
        config: &CliConfig,
        path: &str,
        attach_auth: bool,
        require_auth: bool,
        request_log_path: &Path,
    ) -> Result<String, ApiCommandError> {
        let Some((prefix, project_ref, suffix)) = project_segment(path) else {
            return Ok(path.to_string());
        };
        if project_ref.parse::<uuid::Uuid>().is_ok() {
            return Ok(path.to_string());
        }
        let project_id = self
            .resolve_project_id(
                config,
                project_ref,
                attach_auth,
                require_auth,
                request_log_path,
            )
            .await?;
        Ok(format!("{prefix}{project_id}{suffix}"))
    }

    async fn resolve_project_id(
        &self,
        config: &CliConfig,
        project_ref: &str,
        attach_auth: bool,
        require_auth: bool,
        request_log_path: &Path,
    ) -> Result<String, ApiCommandError> {
        let path = format!("/projects/resolve/{project_ref}");
        let url = self.api_url(&path)?;
        let client = self.http_client(config, attach_auth, require_auth)?;
        let response = client.get(url).send().await?;
        let status = response.status();
        let request_id = request_id_from_headers(response.headers());
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.bytes().await?;
        let response = response_body_value(&content_type, &bytes);
        write_request_log(
            request_log_path,
            "workflow_project_resolution",
            "GET",
            &path,
            status.as_u16(),
            request_id.as_deref(),
            Some("get_projects_resolve_project_ref"),
        );
        if !status.is_success() {
            return Err(ApiCommandError::HttpStatus {
                method: "GET",
                path,
                status: status.as_u16(),
                request_id,
                body: Box::new(response),
            });
        }
        response
            .get("project_id")
            .or_else(|| response.get("projectId"))
            .or_else(|| response.get("id"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| {
                ApiCommandError::InvalidWorkflow {
                    message: format!("Could not resolve project reference `{project_ref}`"),
                }
            })
    }

    async fn ensure_request_auth(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        require_auth: bool,
    ) -> Result<(), ApiCommandError> {
        if self.allow_unauthenticated || !require_auth {
            return Ok(());
        }
        let Some(auth) = &config.auth else {
            return Err(ApiCommandError::NoAuthToken);
        };
        let now = chrono::Utc::now();
        let seconds_remaining = (auth.expires_at - now).num_seconds();
        if auth.expires_at <= now || seconds_remaining <= crate::config::AUTH_EXPIRES_SOON_SECONDS {
            refresh_stored_auth(config, &self.api_url, cli_args, false)
                .await
                .map_err(ApiCommandError::AuthRefresh)?;
        }
        Ok(())
    }

    async fn send_api_request(
        &self,
        config: &CliConfig,
        request: PreparedApiRequest<'_>,
    ) -> Result<reqwest::Response, ApiCommandError> {
        let client = self.http_client(config, request.attach_auth, request.attach_auth)?;
        let mut builder = client
            .request(request.method.reqwest(), request.url.clone())
            .headers(request.headers.clone());
        if !request.query.is_empty() {
            builder = builder.query(request.query);
        }
        if let Some(body) = request.body {
            builder = builder.json(body);
        }
        Ok(builder.send().await?)
    }

    async fn send_workflow_request(
        &self,
        config: &CliConfig,
        request: &WorkflowRequest,
        url: &url::Url,
        body: Option<&Value>,
    ) -> Result<reqwest::Response, ApiCommandError> {
        let requires_auth = request.require_auth && !self.allow_unauthenticated;
        let attach_auth = self.workflow_attach_auth(request, config);
        let client = self.http_client(config, attach_auth, requires_auth)?;
        let mut builder = client.request(request.method.reqwest(), url.clone());
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        if let Some(body) = body {
            builder = builder.json(body);
        }
        Ok(builder.send().await?)
    }

    fn workflow_attach_auth(&self, request: &WorkflowRequest, config: &CliConfig) -> bool {
        if self.allow_unauthenticated {
            return false;
        }
        if request.require_auth {
            return true;
        }
        request.attach_auth
            && config.auth.as_ref().is_some_and(|auth| {
                !auth.access_token.trim().is_empty() && auth.expires_at > chrono::Utc::now()
            })
    }

    fn http_client(
        &self,
        config: &CliConfig,
        attach_auth: bool,
        require_auth: bool,
    ) -> Result<reqwest::Client, ApiCommandError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("api-version"),
            HeaderValue::from_static("1"),
        );

        if attach_auth && let Some(auth) = &config.auth {
            if auth.expires_at <= chrono::Utc::now() {
                return Err(ApiCommandError::ExpiredAuthToken(auth.expires_at));
            }

            let value = format!("Bearer {}", auth.access_token);
            let value = HeaderValue::from_str(&value).map_err(|source| {
                ApiCommandError::InvalidHeaderValue {
                    name: "authorization".to_string(),
                    source,
                }
            })?;
            headers.insert(reqwest::header::AUTHORIZATION, value);
        } else if require_auth {
            return Err(ApiCommandError::NoAuthToken);
        }

        reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(ApiCommandError::Request)
    }

    async fn resolve_operation_id(
        &self,
        config: &CliConfig,
        method: HttpMethod,
        path: &str,
    ) -> Option<String> {
        let spec = self.fetch_openapi(config).await.ok()?;
        let operations = list_operations(&spec, None, Some(method)).ok()?;
        operations
            .into_iter()
            .find(|operation| openapi_path_matches(&operation.path, path))
            .map(|operation| operation.operation_id)
    }

    fn api_url(&self, path: &str) -> Result<url::Url, ApiCommandError> {
        if !path.starts_with('/') {
            return Err(ApiCommandError::InvalidPath(path.to_string()));
        }

        let mut url = self.api_url.clone();
        url.set_path(&format!("/api/v1{path}"));
        Ok(url)
    }
}

fn split_path_and_inline_query(
    input: &str,
) -> Result<(String, Vec<(String, String)>), ApiCommandError> {
    if !input.starts_with('/') {
        return Err(ApiCommandError::InvalidPath(input.to_string()));
    }
    let Some((path, query)) = input.split_once('?') else {
        return Ok((input.to_string(), Vec::new()));
    };
    if path.is_empty() || !path.starts_with('/') {
        return Err(ApiCommandError::InvalidPath(input.to_string()));
    }
    let query = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    Ok((path.to_string(), query))
}

pub(crate) fn request_id_from_headers(headers: &HeaderMap) -> Option<String> {
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

fn write_request_log(
    request_log_path: &Path,
    kind: &str,
    method: &str,
    path: &str,
    status: u16,
    request_id: Option<&str>,
    operation_id: Option<&str>,
) {
    #[cfg(not(test))]
    {
        let _ = crate::request_log::append_request_record_at(
            request_log_path,
            &json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "kind": kind,
                "method": method,
                "path": path,
                "status": status,
                "success": (200..=299).contains(&status),
                "request_id": request_id,
                "operation_id": operation_id,
            }),
        );
    }
    #[cfg(test)]
    let _ = (
        request_log_path,
        kind,
        method,
        path,
        status,
        request_id,
        operation_id,
    );
}

pub(crate) fn response_body_value(content_type: &str, bytes: &[u8]) -> Value {
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

fn ok_envelope(data: Value) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "next_actions": [
            "pcl api list",
            "pcl api inspect get_views_public_incidents",
            "pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated",
        ],
    }))
}

fn dry_run_envelope(data: Value) -> Value {
    let auth_required = data
        .pointer("/request/auth/required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let allow_unauthenticated = data
        .pointer("/request/auth/allow_unauthenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stored_token_valid = data
        .pointer("/request/auth/stored_token_valid")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let method = data
        .pointer("/request/method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut next_actions = if auth_required && !allow_unauthenticated && !stored_token_valid {
        vec![
            "pcl auth ensure --toon",
            "Authenticate before removing --dry-run",
        ]
    } else {
        vec![
            "Remove --dry-run to execute this request",
            "Use --toon for agent consumption or --json for strict JSON parsing",
        ]
    };
    if method_side_effecting(method) {
        next_actions.push("Use --body-template when constructing mutation bodies");
    }
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "next_actions": next_actions,
    }))
}

fn workflow_success_envelope(result: WorkflowCallResult, next_actions: Vec<String>) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": result.body,
        "request": result.request,
        "response": result.response,
        "next_actions": next_actions,
    }))
}

fn workflow_success_envelope_with_data(
    result: WorkflowCallResult,
    data: Value,
    next_actions: Vec<String>,
) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "request": result.request,
        "response": result.response,
        "next_actions": next_actions,
    }))
}

fn workflow_data_for_output_mode(workflow: &str, data: &Value, output_mode: OutputMode) -> Value {
    match (workflow_output_policy(workflow), output_mode) {
        (WorkflowOutputPolicy::MachineRawHumanCompactArtifacts, OutputMode::Human) => {
            compact_deployment_data(data)
        }
        _ => data.clone(),
    }
}

fn request_is_destructive(method: HttpMethod, path: &str) -> bool {
    method == HttpMethod::Delete
        || path.contains("/delete")
        || path.contains("/remove")
        || path.contains("/reject")
        || path.contains("/logout")
}

fn query_pairs_value(query: &[(String, String)]) -> Value {
    Value::Array(
        query
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}

fn upsert_query(query: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some((_, existing)) = query.iter_mut().find(|(key, _)| key == name) {
        *existing = value;
    } else {
        query.push((name.to_string(), value));
    }
}

fn extract_paginated_items(value: &Value, preferred_field: &str) -> Option<Vec<Value>> {
    if let Some(items) = array_at_path(value, preferred_field) {
        return Some(items.to_vec());
    }
    for path in [
        "items",
        "incidents",
        "results",
        "data.items",
        "data.incidents",
        "data.results",
        "data",
    ] {
        if let Some(items) = array_at_path(value, path) {
            return Some(items.to_vec());
        }
    }
    value.as_array().cloned()
}

fn array_at_path<'a>(value: &'a Value, path: &str) -> Option<&'a [Value]> {
    let mut current = value;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current = current.get(segment)?;
    }
    current.as_array().map(Vec::as_slice)
}

fn parse_key_values(
    kind: &'static str,
    entries: &[String],
) -> Result<Vec<(String, String)>, ApiCommandError> {
    entries
        .iter()
        .map(|entry| {
            let (key, value) = entry.split_once('=').ok_or_else(|| {
                ApiCommandError::InvalidKeyValue {
                    kind,
                    input: entry.clone(),
                }
            })?;
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

fn parse_headers(entries: &[String]) -> Result<HeaderMap, ApiCommandError> {
    let mut headers = HeaderMap::new();

    for entry in entries {
        let (name, value) = entry.split_once('=').ok_or_else(|| {
            ApiCommandError::InvalidKeyValue {
                kind: "header",
                input: entry.clone(),
            }
        })?;
        let header_name = HeaderName::from_str(name).map_err(|source| {
            ApiCommandError::InvalidHeaderName {
                name: name.to_string(),
                source,
            }
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|source| {
            ApiCommandError::InvalidHeaderValue {
                name: name.to_string(),
                source,
            }
        })?;
        headers.insert(header_name, header_value);
    }

    Ok(headers)
}

fn read_body(
    body: Option<&str>,
    body_file: Option<&PathBuf>,
) -> Result<Option<String>, ApiCommandError> {
    if let Some(body) = body {
        return Ok(Some(body.to_string()));
    }

    if let Some(path) = body_file {
        if path.as_os_str() == "-" {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(ApiCommandError::Stdin)?;
            return Ok(Some(body));
        }

        return fs::read_to_string(path).map(Some).map_err(|source| {
            ApiCommandError::BodyFile {
                path: path.clone(),
                source,
            }
        });
    }

    Ok(None)
}

fn write_json_output_file(path: &PathBuf, value: &Value) -> Result<(), ApiCommandError> {
    let body = serde_json::to_string_pretty(value)?;
    fs::write(path, body).map_err(|source| {
        ApiCommandError::OutputFile {
            path: path.clone(),
            source,
        }
    })
}

fn write_jsonl_items_output_file(path: &PathBuf, value: &Value) -> Result<(), ApiCommandError> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiCommandError::InvalidWorkflow {
                message: "--jsonl output requires paginated data with an items array".to_string(),
            }
        })?;
    let mut body = String::new();
    for item in items {
        body.push_str(&serde_json::to_string(item)?);
        body.push('\n');
    }
    fs::write(path, body).map_err(|source| {
        ApiCommandError::OutputFile {
            path: path.clone(),
            source,
        }
    })
}

#[cfg(test)]
mod tests;
