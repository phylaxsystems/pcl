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
    current_output_mode,
};
use reqwest::header::{
    HeaderMap,
    HeaderName,
    HeaderValue,
};
use serde::Serialize;
use serde_json::{
    Map,
    Value,
    json,
};
use std::{
    cell::Cell,
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::Read,
    path::{
        Path,
        PathBuf,
    },
    str::FromStr,
};

mod manifest;

pub use manifest::api_manifest;

pub const ENVELOPE_SCHEMA_VERSION: &str = "pcl.envelope.v1";

pub fn with_envelope_metadata(mut value: Value) -> Value {
    if let Value::Object(object) = &mut value {
        object
            .entry("schema_version")
            .or_insert_with(|| json!(ENVELOPE_SCHEMA_VERSION));
        object
            .entry("pcl_version")
            .or_insert_with(|| json!(env!("CARGO_PKG_VERSION")));
    }
    value
}

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
                    "pcl projects --mine".to_string(),
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
                    "pcl projects --mine".to_string(),
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
                    "pcl projects --mine".to_string(),
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
            "recoverable": self.recoverable(),
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
        help = "Base URL for the platform API"
    )]
    api_url: url::Url,

    #[arg(long, help = "Do not attach the stored bearer token to API requests")]
    allow_unauthenticated: bool,

    #[arg(
        long = "dry-run",
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
        after_help = "Examples:\n  pcl projects --mine\n  pcl projects\n  pcl projects --project-id <project-ref>\n  pcl projects --saved --user-id <user-id>\n  pcl projects --create --project-name demo --chain-id 1\n  pcl projects --project-id <project-ref> --update --field github_url=https://github.com/org/repo\n  pcl projects --project-id <project-ref> --save"
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
        after_help = "Examples:\n  pcl releases --project <project-ref>\n  pcl releases --project <project-ref> --release-id <release-id>\n  pcl releases --project <project-ref> --preview --body-file release.json\n  pcl releases --project <project-ref> --release-id <release-id> --backtest-progress\n  pcl releases --project <project-ref> --release-id <release-id> --check-id <check-id> --retry-check\n  pcl releases --project <project-ref> --release-id <release-id> --deploy-calldata --signer-address <signer-address>"
    )]
    Releases(ReleasesArgs),

    #[command(
        about = "Inspect deployments and confirm deployed assertions",
        after_help = "Examples:\n  pcl deployments --project <project-ref>\n  pcl deployments --project <project-ref> --confirm --body-template"
    )]
    Deployments(DeploymentsArgs),

    #[command(
        about = "Manage members, roles, and invitations",
        after_help = "Examples:\n  pcl access --project <project-ref> --members\n  pcl access --project <project-ref> --invite --body-template\n  pcl access --pending\n  pcl access --token <token> --preview"
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

#[derive(Debug, Serialize)]
struct OperationSummary {
    operation_id: String,
    method: &'static str,
    path: String,
    summary: Option<String>,
    tags: Vec<String>,
    auth: Value,
    workflow_alternatives: Vec<Value>,
    raw_api_use: Value,
    inspect_command: String,
    call_command: String,
    input_placeholders: Vec<String>,
    requires_input: bool,
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
    next_actions: Vec<String>,
}

impl WorkflowRequest {
    fn get(path: impl Into<String>, require_auth: bool, next_actions: Vec<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            query: Vec::new(),
            body: None,
            require_auth,
            next_actions,
        }
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

#[derive(clap::Args, Debug)]
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

#[derive(clap::Args, Debug)]
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

#[derive(clap::Args, Debug)]
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
    ProjectsCommand,
    ProjectsArgs,
    Projects,
    "List, inspect, create, update, save, or delete projects",
    "Examples:\n  pcl projects --mine\n  pcl projects\n  pcl projects --project-id <project-ref>\n  pcl projects --saved --user-id <user-id>\n  pcl projects --create --project-name demo --chain-id 1\n  pcl projects --project-id <project-ref> --update --field github_url=https://github.com/org/repo\n  pcl projects --project-id <project-ref> --save\n\nCompatibility alias:\n  pcl api projects ..."
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
    ReleasesCommand,
    ReleasesArgs,
    Releases,
    "List, inspect, create, preview, check, retry, deploy, or remove releases",
    "Examples:\n  pcl releases --project <project-ref>\n  pcl releases --project <project-ref> --release-id <release-id>\n  pcl releases --project <project-ref> --preview --body-file release.json\n  pcl releases --project <project-ref> --release-id <release-id> --backtest-progress\n  pcl releases --project <project-ref> --release-id <release-id> --check-id <check-id> --retry-check\n  pcl releases --project <project-ref> --release-id <release-id> --deploy-calldata --signer-address <signer-address>\n\nCompatibility alias:\n  pcl api releases ..."
);

top_level_workflow_command!(
    DeploymentsCommand,
    DeploymentsArgs,
    Deployments,
    "Inspect deployments and confirm deployed assertions",
    "Examples:\n  pcl deployments --project <project-ref>\n  pcl deployments --project <project-ref> --confirm --body-template\n\nCompatibility alias:\n  pcl api deployments ..."
);

top_level_workflow_command!(
    AccessCommand,
    AccessArgs,
    Access,
    "Manage members, roles, and invitations",
    "Examples:\n  pcl access --project <project-ref> --members\n  pcl access --project <project-ref> --invite --body-template\n  pcl access --pending\n  pcl access --token <token> --preview\n\nCompatibility alias:\n  pcl api access ..."
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
                    let output = template_envelope(account_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_workflow(config, cli_args, account_request(args)?, &request_log_path)
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
                    .run_workflow(
                        config,
                        cli_args,
                        contracts_request(args)?,
                        &request_log_path,
                    )
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
                    .run_workflow(config, cli_args, releases_request(args)?, &request_log_path)
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
                    .run_workflow(
                        config,
                        cli_args,
                        deployments_request(args)?,
                        &request_log_path,
                    )
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
                    .run_workflow(config, cli_args, access_request(args)?, &request_log_path)
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
                    .run_workflow(
                        config,
                        cli_args,
                        protocol_manager_request(args)?,
                        &request_log_path,
                    )
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
                    .run_workflow(
                        config,
                        cli_args,
                        transfers_request(args)?,
                        &request_log_path,
                    )
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Events(args) => {
                let output = self
                    .run_workflow(config, cli_args, events_request(args)?, &request_log_path)
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
        if self.dry_run {
            return Ok(dry_run_envelope(
                self.workflow_request_plan(&request, None, config),
            ));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = projects_next_actions(&result.body, request.next_actions);
        Ok(workflow_success_envelope(result, next_actions))
    }

    async fn run_assertions(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &AssertionsArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(assertions_body_template(args)));
        }
        let request = assertions_request(args)?;
        if self.dry_run {
            return Ok(dry_run_envelope(
                self.workflow_request_plan(&request, None, config),
            ));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = assertions_next_actions(&result.body, args, request.next_actions);
        Ok(workflow_success_envelope(result, next_actions))
    }

    async fn run_search(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &SearchArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let request = search_request(args)?;
        if self.dry_run {
            return Ok(dry_run_envelope(
                self.workflow_request_plan(&request, None, config),
            ));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let next_actions = search_next_actions(&result.body, request.next_actions);
        Ok(workflow_success_envelope(result, next_actions))
    }

    async fn run_workflow(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        request: WorkflowRequest,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        if self.dry_run {
            return Ok(dry_run_envelope(
                self.workflow_request_plan(&request, None, config),
            ));
        }
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        Ok(workflow_success_envelope(result, request.next_actions))
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
                "auth": self.auth_plan(request.require_auth, config),
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
                "auth": self.auth_plan(input.require_auth, config),
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

    fn auth_plan(&self, require_auth: bool, config: &CliConfig) -> Value {
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
            require_auth && !self.allow_unauthenticated && stored_token_valid;
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
        let response = request.send().await?.error_for_status()?;
        Ok(response.json().await?)
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
        let path = self.normalize_project_path(config, &request.path).await?;
        let url = self.api_url(&path)?;
        let requires_auth = request.require_auth && !self.allow_unauthenticated;
        self.ensure_request_auth(config, cli_args, request.require_auth)
            .await?;
        let json_body = if let Some(body) = &request.body {
            Some(self.normalize_request_body(config, &path, body).await?)
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
                "auth": self.auth_plan(request.require_auth, config),
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
    ) -> Result<Value, ApiCommandError> {
        let mut json_body: Value = serde_json::from_str(body)?;
        if path == "/projects/saved"
            && let Some(project_ref) = json_body.get("project_id").and_then(Value::as_str)
            && project_ref.parse::<uuid::Uuid>().is_err()
        {
            let project_id = self.resolve_project_id(config, project_ref).await?;
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
    ) -> Result<String, ApiCommandError> {
        let Some((prefix, project_ref, suffix)) = project_segment(path) else {
            return Ok(path.to_string());
        };
        if project_ref.parse::<uuid::Uuid>().is_ok() {
            return Ok(path.to_string());
        }
        let project_id = self.resolve_project_id(config, project_ref).await?;
        Ok(format!("{prefix}{project_id}{suffix}"))
    }

    async fn resolve_project_id(
        &self,
        config: &CliConfig,
        project_ref: &str,
    ) -> Result<String, ApiCommandError> {
        let url = self.api_url(&format!("/projects/resolve/{project_ref}"))?;
        let client = self.http_client(config, false, false)?;
        let response: Value = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
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
        let client = self.http_client(config, requires_auth, requires_auth)?;
        let mut builder = client.request(request.method.reqwest(), url.clone());
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        if let Some(body) = body {
            builder = builder.json(body);
        }
        Ok(builder.send().await?)
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

fn print_output(value: &Value, json_output: bool) -> Result<(), ApiCommandError> {
    print!("{}", envelope_output_string(value, json_output)?);
    Ok(())
}

pub fn envelope_output_string(
    value: &Value,
    json_output: bool,
) -> Result<String, serde_json::Error> {
    let value = with_envelope_metadata(value.clone());
    let output_mode = if json_output {
        OutputMode::Json
    } else {
        current_output_mode()
    };
    match output_mode {
        OutputMode::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&value)?)),
        OutputMode::Toon => Ok(toon_string(&value)),
        OutputMode::Human => Ok(human_string(&value)),
    }
}

/// Render an envelope for interactive humans.
pub fn human_string(value: &Value) -> String {
    let value = with_envelope_metadata(value.clone());
    let status = value.get("status").and_then(Value::as_str).unwrap_or("ok");
    let mut output = String::new();
    output.push_str(match status {
        "ok" => "OK",
        "error" => "Error",
        "action_required" => "Action required",
        "pending" => "Pending",
        other => other,
    });
    output.push('\n');

    if let Some(error) = value.get("error") {
        render_human_error(&mut output, error);
    } else if !render_human_special(&mut output, &value)
        && !render_human_collection(&mut output, &value)
        && let Some(data) = value.get("data")
    {
        render_human_summary(&mut output, data);
    }

    let human_actions = human_next_actions(&value);
    if !human_actions.is_empty() {
        output.push_str("\nNext:\n");
        for (index, action) in human_actions.iter().enumerate() {
            output.push_str("  ");
            output.push_str(&(index + 1).to_string());
            output.push_str(". ");
            output.push_str(action);
            output.push('\n');
        }
    }
    render_human_request_id(&mut output, &value);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn human_next_actions(envelope: &Value) -> Vec<String> {
    let status = envelope
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("ok");
    let is_empty_ok = status == "ok" && envelope_has_empty_results(envelope);
    let terms_accepted = envelope_terms_accepted(envelope);
    let preserve_agent_flags = envelope
        .get("data")
        .and_then(|data| data.get("consumption_order"))
        .is_some();
    let integration_test_unavailable = envelope
        .pointer("/data/test_available")
        .or_else(|| envelope.pointer("/data/data/test_available"))
        .and_then(Value::as_bool)
        == Some(false);
    envelope
        .get("next_actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|action| !is_dangerous_or_internal_action(action))
        .filter(|action| !(is_empty_ok && is_item_placeholder_action(action)))
        .filter(|action| !(terms_accepted && action.contains("account --accept-terms")))
        .filter(|action| !(integration_test_unavailable && action.contains(" --test")))
        .map(|action| {
            if preserve_agent_flags {
                action.to_string()
            } else {
                human_action_str(action)
            }
        })
        .filter(|action| !action.is_empty())
        .collect()
}

fn envelope_terms_accepted(envelope: &Value) -> bool {
    envelope
        .pointer("/data/terms_accepted")
        .or_else(|| envelope.pointer("/data/data/terms_accepted"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_dangerous_or_internal_action(action: &str) -> bool {
    action.contains(" config delete")
        || action.contains(" --delete")
        || action.contains(" --remove")
        || action.contains(" --revoke")
        || action.contains(" --logout")
        || action.starts_with("Read error.http.body")
        || action.starts_with("Use data.")
}

fn is_item_placeholder_action(action: &str) -> bool {
    [
        "<assertion-id>",
        "<incident-id>",
        "<release-id>",
        "<transfer-id>",
        "<adopter-id>",
        "<job-id>",
        "<project-ref>",
        "<contract-ref>",
        "<token>",
    ]
    .iter()
    .any(|placeholder| action.contains(placeholder))
}

fn envelope_has_empty_results(envelope: &Value) -> bool {
    let Some(data) = envelope.get("data") else {
        return false;
    };
    value_has_empty_results(data)
}

fn value_has_empty_results(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.is_empty(),
        Value::Object(object) => {
            if let Some(inner) = object.get("data")
                && value_has_empty_results(inner)
            {
                return true;
            }
            object.iter().any(|(key, value)| {
                !key.starts_with('_')
                    && (value.as_array().is_some_and(Vec::is_empty)
                        || value_has_empty_results(value))
            })
        }
        _ => false,
    }
}

struct HumanCollection<'a> {
    field: String,
    name: String,
    items: &'a [Value],
    pagination: Option<&'a Value>,
    meta: Option<&'a Value>,
}

fn render_human_error(output: &mut String, error: &Value) {
    output.push('\n');
    let code = error.get("code").and_then(Value::as_str);
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        output.push_str(&human_error_message(code, message));
        output.push('\n');
    } else if let Some(error) = error.as_str() {
        output.push_str(error);
        output.push('\n');
    } else {
        render_human_value(output, error, 0);
    }

    if let Some(reason) = api_error_reason(error) {
        output.push_str("API reason: ");
        output.push_str(&reason);
        output.push('\n');
    }
    if let Some(request_id) = error.get("request_id").and_then(Value::as_str) {
        output.push_str("Request ID: ");
        output.push_str(request_id);
        output.push('\n');
    }
}

fn human_error_message(code: Option<&str>, message: &str) -> String {
    if code.is_some_and(|value| value.starts_with("cli.")) {
        return clean_cli_error_message(message);
    }
    match code {
        Some("api.not_found") => {
            "Resource not found. Check the ID, slug, or API path and try again.".to_string()
        }
        Some("network.request_failed") => {
            "Network request failed. Check --api-url and your network connection, then retry."
                .to_string()
        }
        Some("api.server_error") => {
            "The platform returned a server error. Retry later or report the request ID."
                .to_string()
        }
        _ => message.to_string(),
    }
}

fn clean_cli_error_message(message: &str) -> String {
    let lines = message
        .lines()
        .take_while(|line| !line.starts_with("Usage:") && !line.starts_with("For more information"))
        .map(|line| line.strip_prefix("error: ").unwrap_or(line).trim_end())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.first() == Some(&"the following required arguments were not provided:")
        && let Some(argument) = lines.get(1)
    {
        return format!("Missing required argument: {}", argument.trim());
    }
    lines.join("\n")
}

fn api_error_reason(error: &Value) -> Option<String> {
    let body = error.pointer("/http/body")?;
    for key in ["message", "error", "detail", "reason"] {
        if let Some(value) = body.get(key).and_then(Value::as_str)
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    body.as_str().map(ToString::to_string)
}

fn render_human_special(output: &mut String, envelope: &Value) -> bool {
    let Some(data) = envelope.get("data") else {
        return false;
    };
    let display_data = data.get("data").unwrap_or(data);

    for render in [
        render_login_challenge as fn(&mut String, &Value) -> bool,
        render_request_plan,
        render_auth_status,
        render_identity_status,
        render_doctor,
    ] {
        if render(output, display_data) {
            return true;
        }
    }
    if render_project_home(output, data, display_data) {
        return true;
    }
    for render in [
        render_project_detail as fn(&mut String, &Value) -> bool,
        render_incident_detail,
        render_search_results,
        render_account_detail,
        render_deployment_state,
        render_transfer_state,
        render_integration_status,
        render_protocol_manager_status,
    ] {
        if render(output, display_data) {
            return true;
        }
    }
    if render_mutation_success(output, envelope, display_data) {
        return true;
    }
    for render in [
        render_api_manifest as fn(&mut String, &Value) -> bool,
        render_llms_guide,
        render_workflow_detail,
        render_schema_detail,
        render_operation_detail,
        render_api_coverage,
        render_raw_api_response,
        render_export_result,
        render_job_detail,
        render_path_or_toggle_result,
    ] {
        if render(output, display_data) {
            return true;
        }
    }
    if render_body_template(output, envelope, display_data) {
        return true;
    }

    false
}

fn render_login_challenge(output: &mut String, data: &Value) -> bool {
    if data.get("state").and_then(Value::as_str) != Some("login_required") {
        return false;
    }
    output.push_str("\nLogin required\n");
    if let Some(reason) = data.get("reason").and_then(Value::as_str) {
        writeln!(output, "Reason: {}", human_label(reason)).expect("write to string");
    }
    if let Some(url) = data.get("device_url").and_then(Value::as_str) {
        writeln!(output, "Open: {url}").expect("write to string");
    }
    if let Some(code) = data.get("code").and_then(Value::as_str) {
        writeln!(output, "Code: {code}").expect("write to string");
    }
    if let Some(expires_at) = data.get("expires_at").and_then(Value::as_str) {
        writeln!(output, "Expires: {}", format_timestamp(expires_at)).expect("write to string");
    }
    if let Some(command) = data.get("poll_command").and_then(Value::as_str) {
        writeln!(output, "Poll: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_request_plan(output: &mut String, data: &Value) -> bool {
    if data.get("dry_run").and_then(Value::as_bool) != Some(true) {
        return false;
    }

    output.push_str("\nDry run\n");
    if data.get("valid").and_then(Value::as_bool) == Some(false) {
        output.push_str("Request is not valid.\n");
        if let Some(error) = data.get("error") {
            render_human_error(output, error);
        }
        return true;
    }

    let request = data.get("request").unwrap_or(data);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("-");
    let path = request.get("path").and_then(Value::as_str).unwrap_or("-");
    writeln!(output, "{method} {path}").expect("write to string");
    if let Some(query) = request.get("query").and_then(Value::as_array)
        && !query.is_empty()
    {
        output.push_str("Query: ");
        output.push_str(&name_value_pairs(query));
        output.push('\n');
    }
    if let Some(auth) = request.get("auth") {
        let required = auth
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let attached = auth
            .get("will_attach_stored_token")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        writeln!(
            output,
            "Auth: {}{}",
            if required { "required" } else { "not required" },
            if attached {
                ", stored token will be attached"
            } else {
                ""
            }
        )
        .expect("write to string");
    }
    if let Some(body) = request.get("body")
        && !body.is_null()
    {
        output.push_str("Body: ");
        output.push_str(&human_compact_summary(body));
        output.push('\n');
    }
    if let Some(pagination) = data.get("pagination")
        && !pagination.is_null()
    {
        output.push_str("Pagination: ");
        output.push_str(&human_compact_summary(pagination));
        output.push('\n');
    }
    true
}

fn render_auth_status(output: &mut String, data: &Value) -> bool {
    if !data.get("authenticated").is_some_and(Value::is_boolean)
        || data.get("auth").is_some()
        || data.get("config_path").is_some()
    {
        return false;
    }

    output.push_str("\nAuthentication\n");
    let authenticated = data
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(
        output,
        "Status: {}",
        if authenticated {
            "authenticated"
        } else {
            "not logged in"
        }
    )
    .expect("write to string");
    if let Some(user) = data.get("user").and_then(Value::as_str) {
        writeln!(output, "User: {user}").expect("write to string");
    }
    if let Some(email) = data.get("email").and_then(Value::as_str)
        && data.get("user").and_then(Value::as_str) != Some(email)
    {
        writeln!(output, "Email: {email}").expect("write to string");
    }
    if let Some(wallet) = data.get("wallet_address").and_then(Value::as_str) {
        writeln!(output, "Wallet: {wallet}").expect("write to string");
    }
    if let Some(expires_at) = data.get("expires_at").and_then(Value::as_str) {
        writeln!(output, "Token expires: {}", format_timestamp(expires_at))
            .expect("write to string");
    }
    if let Some(seconds) = data.get("seconds_remaining").and_then(Value::as_i64) {
        writeln!(output, "Time remaining: {}", format_duration(seconds)).expect("write to string");
    }
    if data.get("refreshed").and_then(Value::as_bool) == Some(true) {
        output.push_str("Token refreshed.\n");
    }
    if let Some(request_id) = data.get("request_id").and_then(Value::as_str) {
        writeln!(output, "Request ID: {request_id}").expect("write to string");
    }
    true
}

fn render_identity_status(output: &mut String, data: &Value) -> bool {
    let Some(auth) = data.get("auth") else {
        return false;
    };
    if !auth.get("authenticated").is_some_and(Value::is_boolean) {
        return false;
    }
    output.push_str("\nIdentity\n");
    let authenticated = auth
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(
        output,
        "Status: {}",
        if authenticated {
            "authenticated"
        } else {
            "not logged in"
        }
    )
    .expect("write to string");
    if let Some(user) = auth.get("user").and_then(Value::as_str) {
        writeln!(output, "User: {user}").expect("write to string");
    }
    if let Some(user_id) = auth.get("user_id").and_then(Value::as_str) {
        writeln!(output, "User ID: {user_id}").expect("write to string");
    }
    if let Some(expires_at) = auth.get("expires_at").and_then(Value::as_str) {
        writeln!(output, "Token expires: {}", format_timestamp(expires_at))
            .expect("write to string");
    }
    if let Some(config_path) = data.get("config_path").and_then(Value::as_str) {
        writeln!(output, "Config: {config_path}").expect("write to string");
    }
    if data.get("offline").and_then(Value::as_bool) == Some(true) {
        output.push_str("Network checks skipped.\n");
    }
    true
}

fn render_doctor(output: &mut String, data: &Value) -> bool {
    let Some(checks) = data.get("checks").and_then(Value::as_array) else {
        return false;
    };
    output.push_str("\nDoctor\n");
    render_checks_table(output, checks);
    if let Some(api_url) = data.get("api_url").and_then(Value::as_str) {
        writeln!(output, "\nAPI: {api_url}").expect("write to string");
    }
    output.push_str("Default output: human. Agents should pass --toon; scripts can pass --json.\n");
    true
}

fn render_project_detail(output: &mut String, data: &Value) -> bool {
    if data.get("project_id").is_none() || data.get("project_name").is_none() {
        return false;
    }
    output.push_str("\nProject\n");
    write_string_field(output, "Name", data, "project_name");
    write_string_field(output, "ID", data, "project_id");
    write_string_field(output, "Slug", data, "slug");
    if let Some(private) = data.get("is_private").and_then(Value::as_bool) {
        writeln!(
            output,
            "Visibility: {}",
            if private { "private" } else { "public" }
        )
        .expect("write to string");
    }
    if let Some(dev) = data.get("is_dev").and_then(Value::as_bool) {
        writeln!(
            output,
            "Mode: {}",
            if dev { "development" } else { "production" }
        )
        .expect("write to string");
    }
    write_network_list_for_value(output, data);
    write_optional_string_field(output, "Description", data, "project_description");
    write_optional_string_field(output, "GitHub", data, "github_url");
    write_timestamp_field(output, "Created", data, "created_at");
    write_timestamp_field(output, "Updated", data, "updated_at");
    if let Some(manager) = data
        .get("protocol_manager_address")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        writeln!(output, "Protocol manager: {manager}").expect("write to string");
    } else {
        output.push_str("Protocol manager: not set\n");
    }
    write_count_field(
        output,
        "Submitted assertions",
        data,
        "submitted_assertion_ids",
    );
    write_u64_field(output, "Saved by", data, "saved_count", Some("users"));
    true
}

fn render_incident_detail(output: &mut String, data: &Value) -> bool {
    let Some(incident_id) = data.get("incident_id").and_then(Value::as_str) else {
        return false;
    };
    if data.get("invalidating_transactions").is_none() && data.get("transaction_count").is_none() {
        return false;
    }

    output.push_str("\nIncident\n");
    writeln!(output, "ID: {incident_id}").expect("write to string");
    write_optional_string_field(output, "Reference", data, "public_reference_id");
    write_u64_field(output, "Chain", data, "chain_id", None);
    write_timestamp_field(output, "Window start", data, "window_start");
    write_string_field(output, "Environment", data, "environment");

    if let Some(assertion) = data.get("assertion") {
        output.push_str("\nAssertion\n");
        write_optional_string_field(output, "Title", assertion, "title");
        write_optional_string_field(output, "ID", assertion, "assertion_id");
        if let Some(description) = assertion
            .get("description")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .filter(|value| !is_hex_blob(value))
        {
            writeln!(output, "Description: {}", truncate(description, 96))
                .expect("write to string");
        }
    } else {
        write_optional_string_field(output, "Assertion ID", data, "assertion_id");
    }

    if let Some(adopter) = data.get("assertion_adopter") {
        output.push_str("\nAssertion adopter\n");
        write_optional_string_field(output, "Name", adopter, "name");
        write_optional_string_field(output, "Address", adopter, "address");
        write_optional_string_field(output, "ID", adopter, "id");
    } else {
        write_optional_string_field(output, "Assertion adopter ID", data, "assertion_adopter_id");
    }

    output.push_str("\nTrace summary\n");
    if let Some(value) = data.get("transaction_count").and_then(Value::as_u64) {
        writeln!(
            output,
            "Invalidating transactions: {}",
            plural_count(value, "transaction")
        )
        .expect("write to string");
    }
    write_u64_field(output, "Traces completed", data, "traces_completed", None);
    write_u64_field(output, "Traces pending", data, "traces_pending", None);

    if let Some(transactions) = data
        .get("invalidating_transactions")
        .and_then(Value::as_array)
        .filter(|transactions| !transactions.is_empty())
    {
        let shown = transactions.len().min(5);
        writeln!(
            output,
            "\nInvalidating transactions (first {shown} of {})",
            transactions.len()
        )
        .expect("write to string");
        writeln!(
            output,
            "{} {} {} {} Trace",
            pad("#", 3),
            pad("Time", 16),
            pad("Tx hash", 20),
            pad("Result", 11)
        )
        .expect("write to string");
        for (index, tx) in transactions.iter().take(shown).enumerate() {
            let time = tx
                .get("incident_timestamp")
                .and_then(Value::as_str)
                .map_or_else(|| "-".to_string(), format_timestamp);
            let hash = first_string_field(tx, &["transaction_hash", "hash", "tx_hash"])
                .map_or_else(|| "-".to_string(), |value| truncate(&value, 20));
            let result = match tx.get("landed_on_chain").and_then(Value::as_bool) {
                Some(true) => "landed",
                Some(false) => "invalidated",
                None => "-",
            };
            let trace = tx
                .get("debug_traces")
                .and_then(Value::as_array)
                .and_then(|traces| traces.first())
                .and_then(|trace| trace.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("-");
            writeln!(
                output,
                "{} {} {} {} {}",
                pad(&(index + 1).to_string(), 3),
                pad(&time, 16),
                pad(&hash, 20),
                pad(result, 11),
                trace
            )
            .expect("write to string");
        }
    }

    true
}

fn render_project_home(output: &mut String, envelope_data: &Value, data: &Value) -> bool {
    let Some(member_projects) = data.get("member_projects").and_then(Value::as_array) else {
        return false;
    };
    let saved_projects = data
        .get("saved_projects")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let no_project_adopters = data
        .get("no_project_adopters")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    output.push_str("\nYour projects\n");
    writeln!(
        output,
        "Showing {} you belong to",
        plural_count(member_projects.len(), "project")
    )
    .expect("write to string");
    if let Some(meta) = envelope_data.get("_meta") {
        render_collection_meta(output, meta);
    }
    output.push('\n');

    if member_projects.is_empty() {
        output.push_str("No projects found for your account.\n");
    } else {
        render_projects_table(output, member_projects);
    }

    writeln!(
        output,
        "\nSaved projects: {}",
        plural_count(saved_projects.len(), "project")
    )
    .expect("write to string");
    if !saved_projects.is_empty() {
        render_projects_table(output, saved_projects);
    }
    writeln!(
        output,
        "Contracts without a project: {}",
        plural_count(no_project_adopters.len(), "contract")
    )
    .expect("write to string");
    true
}

fn render_search_results(output: &mut String, data: &Value) -> bool {
    let Some(projects) = data.get("projects").and_then(Value::as_array) else {
        return false;
    };
    let contracts = data
        .get("contracts")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let assertions = data
        .get("assertions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    output.push_str("\nSearch results\n");
    writeln!(output, "Projects: {}", projects.len()).expect("write to string");
    writeln!(output, "Contracts: {}", contracts.len()).expect("write to string");
    writeln!(output, "Assertions: {}", assertions.len()).expect("write to string");

    if projects.is_empty() && contracts.is_empty() && assertions.is_empty() {
        output.push_str("\nNo search results found.\n");
        return true;
    }

    if !projects.is_empty() {
        output.push_str("\nProjects\n");
        render_generic_table(output, projects);
    }
    if !contracts.is_empty() {
        output.push_str("\nContracts\n");
        render_search_contracts_table(output, contracts);
    }
    if !assertions.is_empty() {
        output.push_str("\nAssertions\n");
        render_generic_table(output, assertions);
    }
    true
}

fn render_search_contracts_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<32} {:<10} {:<22} Project",
        "Contract", "Network", "Address"
    )
    .expect("write to string");
    for item in items {
        let data = item.get("data").unwrap_or(item);
        let name = data
            .get("contract_name")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let network = data.get("network").and_then(Value::as_str).unwrap_or("-");
        let address = data.get("address").and_then(Value::as_str).unwrap_or("-");
        let project = data
            .get("related_project_slug")
            .or_else(|| data.get("related_project_id"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<32} {:<10} {:<22} {}",
            pad(name, 32),
            pad(network, 10),
            pad(address, 22),
            project
        )
        .expect("write to string");
    }
}

fn render_account_detail(output: &mut String, data: &Value) -> bool {
    if data.get("email").is_none() || data.get("authMethod").is_none() {
        return false;
    }
    output.push_str("\nAccount\n");
    write_string_field(output, "Email", data, "email");
    write_string_field(output, "User ID", data, "id");
    write_string_field(output, "Auth method", data, "authMethod");
    write_string_field(output, "Scope", data, "scope");
    write_bool_field(output, "Whitelisted", data, "whitelisted");
    write_bool_field(output, "Terms accepted", data, "terms_accepted");
    write_timestamp_field(output, "Terms accepted at", data, "terms_accepted_at");
    true
}

fn render_deployment_state(output: &mut String, data: &Value) -> bool {
    let Some(project) = data.get("project") else {
        return false;
    };
    if data.get("available_contracts").is_none()
        || data.get("submitted_assertions").is_none()
        || data.get("staging_assertions").is_none()
    {
        return false;
    }
    output.push_str("\nDeployments\n");
    if let Some(name) = project.get("project_name").and_then(Value::as_str) {
        writeln!(output, "Project: {name}").expect("write to string");
    }
    if let Some(id) = project.get("project_id").and_then(Value::as_str) {
        writeln!(output, "Project ID: {id}").expect("write to string");
    }
    write_network_list_for_value(output, project);
    write_count_field(output, "Available contracts", data, "available_contracts");
    write_count_field(output, "Submitted assertions", data, "submitted_assertions");
    write_count_field(output, "Staging assertions", data, "staging_assertions");
    if let Some(meta) = data.get("_meta") {
        render_collection_meta(output, meta);
    }
    true
}

fn render_transfer_state(output: &mut String, data: &Value) -> bool {
    let (Some(incoming), Some(outgoing)) = (data.get("incoming"), data.get("outgoing")) else {
        return false;
    };
    output.push_str("\nProtocol manager transfers\n");
    write_transfer_counts(output, "Incoming", incoming);
    write_transfer_counts(output, "Outgoing", outgoing);
    true
}

fn render_integration_status(output: &mut String, data: &Value) -> bool {
    if data.get("configured").is_none() || data.get("enabled").is_none() {
        return false;
    }
    output.push_str("\nIntegration\n");
    write_bool_field(output, "Configured", data, "configured");
    write_bool_field(output, "Enabled", data, "enabled");
    write_optional_string_field(output, "Webhook URL", data, "webhook_url");
    write_timestamp_field(output, "Last notification", data, "last_notification_at");
    write_u64_field(
        output,
        "Notifications sent",
        data,
        "notification_count",
        None,
    );
    write_bool_field(output, "Test available", data, "test_available");
    true
}

fn render_protocol_manager_status(output: &mut String, data: &Value) -> bool {
    if data.get("has_pending_transfer").is_none()
        || data.get("contracts_pending").is_none()
        || data.get("contracts_total").is_none()
    {
        return false;
    }
    output.push_str("\nProtocol manager\n");
    write_bool_field(output, "Pending transfer", data, "has_pending_transfer");
    write_optional_string_field(output, "Current manager", data, "current_manager_address");
    write_optional_string_field(output, "New manager", data, "new_manager_address");
    write_u64_field(output, "Contracts pending", data, "contracts_pending", None);
    write_u64_field(output, "Contracts total", data, "contracts_total", None);
    true
}

fn render_mutation_success(output: &mut String, envelope: &Value, data: &Value) -> bool {
    if data.get("success").and_then(Value::as_bool) != Some(true)
        || data
            .as_object()
            .is_some_and(|object| object.contains_key("message"))
    {
        return false;
    }
    let Some(request) = envelope.get("request") else {
        return false;
    };
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let path = request.get("path").and_then(Value::as_str).unwrap_or("");
    output.push('\n');
    output.push_str(mutation_success_message(method, path));
    output.push('\n');
    true
}

fn mutation_success_message(method: &str, path: &str) -> &'static str {
    match (method, path) {
        ("POST", "/projects/saved") => "Project saved",
        ("DELETE", "/projects/saved") => "Project removed from saved projects",
        _ if method == "DELETE"
            && path.starts_with("/projects/")
            && path.contains("/invitations/") =>
        {
            "Invitation revoked"
        }
        _ if method == "POST" && path.starts_with("/projects/") && path.ends_with("/resend") => {
            "Invitation resent"
        }
        _ if method == "PATCH" && path.starts_with("/projects/") && path.contains("/members/") => {
            "Member role updated"
        }
        _ if method == "DELETE" && path.starts_with("/projects/") && path.contains("/members/") => {
            "Member removed"
        }
        _ if method == "DELETE" && path.ends_with("/protocol-manager") => {
            "Protocol manager cleared"
        }
        _ if method == "POST" && path.ends_with("/confirm-transfer") => {
            "Protocol manager transfer confirmed"
        }
        _ if method == "DELETE"
            && path.starts_with("/projects/")
            && !path.contains("/integrations/")
            && !path.contains("/invitations/")
            && !path.contains("/members/")
            && !path.contains("/protocol-manager") =>
        {
            "Project deleted"
        }
        _ => "Request completed",
    }
}

fn render_body_template(output: &mut String, envelope: &Value, data: &Value) -> bool {
    if !is_body_template_envelope(envelope) {
        return false;
    }
    if let Some(variants) = data.get("body_variants").and_then(Value::as_array) {
        output.push_str("\nBody variants\n");
        for variant in variants {
            let name = variant
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("variant");
            writeln!(output, "- {name}").expect("write to string");
            if let Some(body) = variant.get("body") {
                render_human_value(output, body, 4);
            }
        }
        return true;
    }

    let Some(object) = data.as_object() else {
        return false;
    };
    if object.is_empty()
        || !object
            .values()
            .all(|value| is_scalar(value) || value.is_object() || value.is_array())
    {
        return false;
    }
    if !object.keys().any(|key| is_body_template_key(key)) {
        return false;
    }
    output.push_str("\nBody template\n");
    render_human_value(output, data, 2);
    true
}

fn is_body_template_envelope(envelope: &Value) -> bool {
    envelope
        .get("next_actions")
        .and_then(Value::as_array)
        .is_some_and(|actions| {
            actions.iter().filter_map(Value::as_str).any(|action| {
                action.starts_with("Pass the template")
                    || action.starts_with("Choose one entry from data.body_variants")
            })
        })
}

fn render_api_manifest(output: &mut String, data: &Value) -> bool {
    if data.get("name").and_then(Value::as_str) != Some("pcl") || data.get("commands").is_none() {
        return false;
    }
    output.push_str("\nPCL command surface\n");
    if let Some(description) = data.get("description").and_then(Value::as_str) {
        writeln!(output, "{description}").expect("write to string");
    }
    output.push_str("\nStart here:\n");
    for command in ["pcl --llms", "pcl workflows", "pcl schema list"] {
        writeln!(output, "  - {command}").expect("write to string");
    }
    if let Some(commands) = data.get("commands").and_then(Value::as_array) {
        writeln!(
            output,
            "\n{} workflow/API command groups available.",
            commands.len()
        )
        .expect("write to string");
    }
    true
}

fn render_llms_guide(output: &mut String, data: &Value) -> bool {
    if data.get("purpose").is_none() || data.get("consumption_order").is_none() {
        return false;
    }
    output.push_str("\nLLM guide\n");
    if let Some(purpose) = data.get("purpose").and_then(Value::as_str) {
        writeln!(output, "{purpose}").expect("write to string");
    }
    if let Some(order) = data.get("consumption_order").and_then(Value::as_array) {
        output.push_str("\nRecommended order:\n");
        for command in order.iter().filter_map(Value::as_str).take(8) {
            writeln!(output, "  - {command}").expect("write to string");
        }
    }
    true
}

fn render_workflow_detail(output: &mut String, data: &Value) -> bool {
    if data.get("steps").is_none() || data.get("name").is_none() {
        return false;
    }
    output.push('\n');
    if let Some(name) = data.get("name").and_then(Value::as_str) {
        writeln!(output, "Workflow: {name}").expect("write to string");
    }
    if let Some(description) = data.get("description").and_then(Value::as_str) {
        writeln!(output, "{description}").expect("write to string");
    }
    if let Some(steps) = data.get("steps").and_then(Value::as_array) {
        output.push_str("\nSteps:\n");
        for (index, step) in steps.iter().enumerate() {
            let command = step.get("command").and_then(Value::as_str).unwrap_or("-");
            let description = step.get("output").and_then(Value::as_str).unwrap_or("");
            writeln!(
                output,
                "  {}. {}{}",
                index + 1,
                humanize_command(command),
                if description.is_empty() {
                    String::new()
                } else {
                    format!(" -> {description}")
                }
            )
            .expect("write to string");
        }
    }
    true
}

fn render_schema_detail(output: &mut String, data: &Value) -> bool {
    if data.get("workflow").is_none()
        || !(data.get("actions").is_some() || data.get("action").is_some())
    {
        return false;
    }
    output.push('\n');
    if let Some(workflow) = data.get("workflow").and_then(Value::as_str) {
        writeln!(output, "Schema: {workflow}").expect("write to string");
    }
    if let Some(command) = data.get("command").and_then(Value::as_str) {
        writeln!(output, "Command: {}", humanize_command(command)).expect("write to string");
    }
    if let Some(actions) = data.get("actions").and_then(Value::as_array) {
        render_actions_table(output, actions);
    } else if let Some(action) = data.get("action") {
        render_action_detail(output, action);
    }
    true
}

fn render_operation_detail(output: &mut String, data: &Value) -> bool {
    if data.get("operation_id").is_none()
        || data.get("method").is_none()
        || data.get("path").is_none()
    {
        return false;
    }
    output.push_str("\nAPI operation\n");
    let method = data.get("method").and_then(Value::as_str).unwrap_or("-");
    let path = data.get("path").and_then(Value::as_str).unwrap_or("-");
    writeln!(output, "{method} {path}").expect("write to string");
    if let Some(operation_id) = data.get("operation_id").and_then(Value::as_str) {
        writeln!(output, "Operation: {operation_id}").expect("write to string");
    }
    if let Some(summary) = data.get("summary").and_then(Value::as_str) {
        writeln!(output, "Summary: {summary}").expect("write to string");
    }
    if let Some(policy) = data.pointer("/raw_api_use/policy").and_then(Value::as_str) {
        writeln!(output, "Raw API policy: {}", human_label(policy)).expect("write to string");
    }
    if let Some(alternatives) = data.get("workflow_alternatives").and_then(Value::as_array)
        && !alternatives.is_empty()
    {
        output.push_str("Prefer:\n");
        for alternative in alternatives {
            if let Some(example) = alternative.get("example").and_then(Value::as_str) {
                writeln!(output, "  - {}", humanize_command(example)).expect("write to string");
            }
        }
    }
    if let Some(command) = data.get("call_command").and_then(Value::as_str) {
        writeln!(output, "Raw call: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_api_coverage(output: &mut String, data: &Value) -> bool {
    let Some(total) = data.get("total_operations").and_then(Value::as_u64) else {
        return false;
    };
    output.push_str("\nAPI coverage\n");
    writeln!(output, "Operations: {total}").expect("write to string");
    for (label, field) in [
        ("No request-log hit", "no_hit_count"),
        ("Hit without 2xx", "no_2xx_count"),
        ("Write hit without 2xx", "write_no_2xx_count"),
        ("Unmatched records", "unmatched_record_count"),
    ] {
        if let Some(count) = data.get(field).and_then(Value::as_u64) {
            writeln!(output, "{label}: {count}").expect("write to string");
        }
    }
    if let Some(by_method) = data.get("by_method").and_then(Value::as_object) {
        output.push_str("\nBy method:\n");
        for (method, stats) in by_method {
            let total = stats.get("total").and_then(Value::as_u64).unwrap_or(0);
            let hit = stats.get("hit").and_then(Value::as_u64).unwrap_or(0);
            let ok = stats.get("ok").and_then(Value::as_u64).unwrap_or(0);
            writeln!(output, "  {method}: {ok}/{total} 2xx, {hit} hit").expect("write to string");
        }
    }
    true
}

fn render_raw_api_response(output: &mut String, data: &Value) -> bool {
    if data.get("request").is_none() || data.get("response").is_none() {
        return false;
    }
    let request = data.get("request").unwrap_or(&Value::Null);
    let response = data.get("response").unwrap_or(&Value::Null);
    output.push_str("\nAPI response\n");
    if let (Some(method), Some(path)) = (
        request.get("method").and_then(Value::as_str),
        request.get("path").and_then(Value::as_str),
    ) {
        writeln!(output, "{method} {path}").expect("write to string");
    }
    if let Some(status) = response.get("status").and_then(Value::as_u64) {
        writeln!(output, "HTTP {status}").expect("write to string");
    }
    if let Some(request_id) = response.get("request_id").and_then(Value::as_str) {
        writeln!(output, "Request ID: {request_id}").expect("write to string");
    }
    if let Some(body) = response.get("body") {
        if let Some(collection) = find_collection_in_value(body, "") {
            output.push('\n');
            output.push_str(&collection.name);
            output.push('\n');
            output.push_str(&collection_summary(&collection));
            output.push_str("\n\n");
            if collection.items.is_empty() {
                writeln!(output, "No {} found.", collection.name.to_ascii_lowercase())
                    .expect("write to string");
            } else {
                render_collection_items(output, &collection);
            }
        } else {
            output.push_str("Body: ");
            output.push_str(&human_compact_summary(body));
            output.push('\n');
        }
    }
    if let Some(path) = data.get("output_path").and_then(Value::as_str) {
        writeln!(output, "Wrote: {path}").expect("write to string");
    }
    true
}

fn render_export_result(output: &mut String, data: &Value) -> bool {
    if data.get("export").and_then(Value::as_str) != Some("incidents")
        && !(data.get("plan").is_some() && data.get("job_id").is_some())
    {
        return false;
    }
    output.push_str("\nIncident export\n");
    if let Some(job_id) = data.get("job_id").and_then(Value::as_str) {
        writeln!(output, "Job: {job_id}").expect("write to string");
    }
    let source = data.get("plan").unwrap_or(data);
    for (label, field) in [
        ("Output", "out"),
        ("Errors", "errors"),
        ("Checkpoint", "checkpoint"),
    ] {
        if let Some(path) = source.get(field).and_then(Value::as_str) {
            writeln!(output, "{label}: {path}").expect("write to string");
        }
    }
    for (label, field) in [
        ("Pages fetched", "pages_fetched"),
        ("Incidents written", "incidents_written"),
        ("Errors written", "errors_written"),
        ("Retries", "retries_attempted"),
    ] {
        if let Some(count) = data.get(field).and_then(Value::as_u64) {
            writeln!(output, "{label}: {count}").expect("write to string");
        }
    }
    if let Some(command) = data.get("resume_command").and_then(Value::as_str) {
        writeln!(output, "Resume: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_job_detail(output: &mut String, data: &Value) -> bool {
    let job = data.get("job").unwrap_or(data);
    if job.get("job_id").is_none() {
        return false;
    }
    output.push_str("\nJob\n");
    for (label, field) in [
        ("ID", "job_id"),
        ("Kind", "kind"),
        ("Status", "status"),
        ("Updated", "updated_at"),
    ] {
        if let Some(value) = job.get(field) {
            writeln!(output, "{label}: {}", human_cell(value)).expect("write to string");
        }
    }
    if let Some(stats) = job.get("stats") {
        output.push_str("Stats: ");
        output.push_str(&human_compact_summary(stats));
        output.push('\n');
    }
    if let Some(command) = data
        .get("resume_command")
        .or_else(|| job.get("resume_command"))
        .and_then(Value::as_str)
    {
        writeln!(output, "Resume: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_path_or_toggle_result(output: &mut String, data: &Value) -> bool {
    if data
        .as_object()
        .is_some_and(|object| object.values().any(Value::is_array))
    {
        return false;
    }
    let path_fields = [
        ("Config", "config_path"),
        ("Artifacts", "artifact_dir"),
        ("Request log", "request_log"),
        ("Jobs", "jobs_path"),
    ];
    let mut rendered = false;
    for (label, field) in path_fields {
        if let Some(path) = data.get(field).and_then(Value::as_str) {
            if !rendered {
                output.push('\n');
                rendered = true;
            }
            writeln!(output, "{label}: {path}").expect("write to string");
        }
    }
    for (label, field) in [("Created", "created"), ("Deleted", "deleted")] {
        if let Some(value) = data.get(field).and_then(Value::as_bool) {
            if !rendered {
                output.push('\n');
                rendered = true;
            }
            writeln!(output, "{label}: {}", yes_no(value)).expect("write to string");
        }
    }
    rendered
}

fn write_string_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(value) = data.get(field).and_then(Value::as_str) {
        writeln!(output, "{label}: {value}").expect("write to string");
    }
}

fn write_optional_string_field(output: &mut String, label: &str, data: &Value, field: &str) {
    match data.get(field) {
        Some(Value::String(value)) if !value.is_empty() => {
            writeln!(output, "{label}: {value}").expect("write to string");
        }
        Some(Value::Null) | None => {}
        Some(value) if is_scalar(value) => {
            writeln!(output, "{label}: {}", scalar_string(value)).expect("write to string");
        }
        Some(_) => {}
    }
}

fn write_timestamp_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(value) = data.get(field).and_then(Value::as_str) {
        writeln!(output, "{label}: {}", format_timestamp(value)).expect("write to string");
    }
}

fn write_bool_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(value) = data.get(field).and_then(Value::as_bool) {
        writeln!(output, "{label}: {}", yes_no(value)).expect("write to string");
    }
}

fn write_u64_field(
    output: &mut String,
    label: &str,
    data: &Value,
    field: &str,
    unit: Option<&str>,
) {
    if let Some(value) = data.get(field).and_then(Value::as_u64) {
        if let Some(unit) = unit {
            writeln!(output, "{label}: {value} {unit}").expect("write to string");
        } else {
            writeln!(output, "{label}: {value}").expect("write to string");
        }
    }
}

fn write_count_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(values) = data.get(field).and_then(Value::as_array) {
        writeln!(
            output,
            "{label}: {}",
            plural_count(values.len(), count_field_unit(label, field))
        )
        .expect("write to string");
    }
}

fn count_field_unit(label: &str, field: &str) -> &'static str {
    match (label, field) {
        ("Available contracts", _) => "contract",
        ("Submitted assertions", _) | ("Staging assertions", _) => "assertion",
        (_, "available_contracts") => "contract",
        (_, "submitted_assertions" | "staging_assertions" | "submitted_assertion_ids") => {
            "assertion"
        }
        _ => "item",
    }
}

fn write_network_list_for_value(output: &mut String, data: &Value) {
    let names = data
        .get("chain_names")
        .or_else(|| data.get("project_networks"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if names.is_empty() {
        return;
    }
    writeln!(output, "Networks: {}", names.join(", ")).expect("write to string");
}

fn write_transfer_counts(output: &mut String, label: &str, value: &Value) {
    let projects = value
        .get("project_transfers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let contracts = value
        .get("contract_transfers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    writeln!(
        output,
        "{label}: {}, {}",
        plural_count(projects, "project transfer"),
        plural_count(contracts, "contract transfer")
    )
    .expect("write to string");
}

fn render_human_collection(output: &mut String, envelope: &Value) -> bool {
    let Some(collection) = find_human_collection(envelope) else {
        return false;
    };

    output.push('\n');
    output.push_str(&collection.name);
    output.push('\n');
    output.push_str(&collection_summary(&collection));
    output.push('\n');
    if let Some(meta) = collection.meta {
        render_collection_meta(output, meta);
    }
    output.push('\n');

    if collection.items.is_empty() {
        writeln!(output, "No {} found.", collection.name.to_ascii_lowercase())
            .expect("write to string");
        return true;
    }

    render_collection_items(output, &collection);

    if let Some(pagination) = collection.pagination
        && pagination
            .get("hasMore")
            .or_else(|| pagination.get("has_more"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        let next_page = pagination
            .get("page")
            .and_then(Value::as_u64)
            .map_or(2, |page| page.saturating_add(1));
        let limit = pagination
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(collection.items.len() as u64);
        output.push('\n');
        writeln!(
            output,
            "More results available. Try --page {next_page} --limit {limit}."
        )
        .expect("write to string");
    }

    true
}

fn find_human_collection(envelope: &Value) -> Option<HumanCollection<'_>> {
    let data = envelope.get("data")?;
    let request_path = envelope
        .pointer("/request/path")
        .and_then(Value::as_str)
        .unwrap_or_default();

    find_collection_in_value(data, request_path)
}

fn find_collection_in_value<'a>(
    data: &'a Value,
    request_path: &str,
) -> Option<HumanCollection<'a>> {
    if let Some(inner) = data.get("data")
        && let Some(collection) = find_collection_in_value(inner, request_path)
    {
        return Some(HumanCollection {
            meta: data.get("_meta").or(collection.meta),
            ..collection
        });
    }

    if let Some(items) = data.as_array() {
        return Some(HumanCollection {
            field: infer_collection_field(request_path),
            name: infer_collection_name("items", request_path, items),
            items,
            pagination: None,
            meta: None,
        });
    }

    if let Some(items) = data.get("items").and_then(Value::as_array) {
        return Some(HumanCollection {
            field: "items".to_string(),
            name: infer_collection_name("items", request_path, items),
            items,
            pagination: data.get("pagination"),
            meta: data.get("_meta"),
        });
    }

    for field in [
        "incidents",
        "assertions",
        "contracts",
        "releases",
        "projects",
        "deployments",
        "events",
        "operations",
        "workflows",
        "schemas",
        "checks",
        "records",
        "jobs",
        "artifacts",
        "members",
        "invitations",
        "integrations",
        "transfers",
        "requests",
        "no_hit",
        "no_2xx",
        "write_no_2xx",
        "unmatched_records",
        "body_variants",
        "examples",
        "product_surfaces",
    ] {
        if let Some(items) = data.get(field).and_then(Value::as_array) {
            return Some(HumanCollection {
                field: field.to_string(),
                name: human_label(field),
                items,
                pagination: data.get("pagination"),
                meta: data.get("_meta"),
            });
        }
    }

    None
}

fn infer_collection_field(request_path: &str) -> String {
    if request_path.contains("assertion_adopters") {
        return "contracts".to_string();
    }
    for field in [
        "incidents",
        "projects",
        "assertions",
        "contracts",
        "releases",
        "deployments",
        "events",
        "members",
        "invitations",
        "transfers",
    ] {
        if request_path.contains(field) {
            return field.to_string();
        }
    }
    "items".to_string()
}

fn infer_collection_name(field: &str, request_path: &str, items: &[Value]) -> String {
    if request_path.contains("assertion_adopters") {
        return "Contracts".to_string();
    }
    for name in [
        "incidents",
        "assertions",
        "contracts",
        "releases",
        "projects",
        "deployments",
        "events",
        "operations",
        "workflows",
        "schemas",
        "records",
        "jobs",
        "artifacts",
        "requests",
    ] {
        if request_path.contains(name) {
            return human_label(name);
        }
    }
    if items.iter().any(has_incident_shape) {
        return "Incidents".to_string();
    }
    human_label(field)
}

fn collection_summary(collection: &HumanCollection<'_>) -> String {
    let shown = collection.items.len();
    if let Some(pagination) = collection.pagination {
        let total = pagination
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or(shown as u64);
        let page = pagination.get("page").and_then(Value::as_u64);
        let limit = pagination.get("limit").and_then(Value::as_u64);
        let item_name = collection_item_name(&collection.name, total);
        let mut summary = if total > shown as u64 {
            format!("Showing {shown} of {total} {item_name}")
        } else {
            format!("Showing {shown} {item_name}")
        };
        if let Some(page) = page {
            write!(summary, " on page {page}").expect("write to string");
        }
        if let Some(limit) = limit {
            write!(summary, " (limit {limit})").expect("write to string");
        }
        return summary;
    }
    let item_name = collection_item_name(&collection.name, shown as u64);
    format!("Showing {shown} {item_name}")
}

fn collection_item_name(name: &str, count: u64) -> String {
    let lower = name.to_ascii_lowercase();
    if count != 1 {
        return lower;
    }
    lower.strip_suffix("ies").map_or_else(
        || lower.strip_suffix("s").unwrap_or(&lower).to_string(),
        |stem| format!("{stem}y"),
    )
}

fn render_collection_items(output: &mut String, collection: &HumanCollection<'_>) {
    match collection.field.as_str() {
        "checks" => render_checks_table(output, collection.items),
        "operations" => render_operations_table(output, collection.items),
        "workflows" => render_workflows_table(output, collection.items),
        "schemas" => render_schemas_table(output, collection.items),
        "records" | "requests" | "unmatched_records" => {
            render_request_records_table(output, collection.items);
        }
        "jobs" => render_jobs_table(output, collection.items),
        "artifacts" => render_artifacts_table(output, collection.items),
        "members" => render_members_table(output, collection.items),
        "invitations" => render_invitations_table(output, collection.items),
        "projects" => render_projects_table(output, collection.items),
        "releases" => render_releases_table(output, collection.items),
        "events" => render_events_table(output, collection.items),
        "no_hit" | "no_2xx" | "write_no_2xx" => render_coverage_table(output, collection.items),
        "body_variants" => render_body_variant_table(output, collection.items),
        _ if is_incident_collection(collection) => render_incident_table(output, collection.items),
        _ => render_generic_table(output, collection.items),
    }
}

fn render_checks_table(output: &mut String, items: &[Value]) {
    writeln!(output, "{:<20} {:<10} Details", "Check", "Status").expect("write to string");
    for item in items {
        let name = item.get("name").and_then(Value::as_str).unwrap_or("-");
        let status = item.get("status").and_then(Value::as_str).unwrap_or("-");
        let details = item
            .get("details")
            .or_else(|| item.get("path"))
            .map_or_else(String::new, human_compact_summary);
        writeln!(
            output,
            "{:<20} {:<10} {}",
            pad(name, 20),
            pad(status, 10),
            details
        )
        .expect("write to string");
    }
}

fn render_operations_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<7} {:<45} {:<36} Policy",
        "Method", "Path", "Operation"
    )
    .expect("write to string");
    for item in items {
        let method = item.get("method").and_then(Value::as_str).unwrap_or("-");
        let path = item.get("path").and_then(Value::as_str).unwrap_or("-");
        let operation = item
            .get("operation_id")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let policy = item
            .pointer("/raw_api_use/policy")
            .and_then(Value::as_str)
            .map_or("-", |value| value);
        writeln!(
            output,
            "{:<7} {:<45} {:<36} {}",
            method,
            pad(path, 45),
            pad(operation, 36),
            human_label(policy)
        )
        .expect("write to string");
    }
}

fn render_workflows_table(output: &mut String, items: &[Value]) {
    writeln!(output, "{:<28} Steps  Description", "Workflow").expect("write to string");
    for item in items {
        let name = item.get("name").and_then(Value::as_str).unwrap_or("-");
        let steps = item
            .get("steps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        writeln!(
            output,
            "{:<28} {:<5} {}",
            pad(name, 28),
            steps,
            truncate(description, 72)
        )
        .expect("write to string");
    }
}

fn render_schemas_table(output: &mut String, items: &[Value]) {
    writeln!(output, "{:<24} {:<7} Command", "Workflow", "Actions").expect("write to string");
    for item in items {
        let workflow = item.get("workflow").and_then(Value::as_str).unwrap_or("-");
        let actions = item.get("actions").and_then(Value::as_u64).unwrap_or(0);
        let command = item.get("command").and_then(Value::as_str).unwrap_or("-");
        writeln!(
            output,
            "{:<24} {:<7} {}",
            pad(workflow, 24),
            actions,
            truncate(&humanize_command(command), 96)
        )
        .expect("write to string");
    }
}

fn render_request_records_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<16} {:<7} {:<45} {:<6} Request ID",
        "Time", "Method", "Path", "HTTP"
    )
    .expect("write to string");
    for item in items {
        let time = item
            .get("timestamp")
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp);
        let method = item.get("method").and_then(Value::as_str).unwrap_or("-");
        let path = item.get("path").and_then(Value::as_str).unwrap_or("-");
        let status = item
            .get("status")
            .and_then(Value::as_u64)
            .map_or_else(|| "-".to_string(), |value| value.to_string());
        let request_id = item
            .get("request_id")
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<16} {:<7} {:<45} {:<6} {}",
            pad(&time, 16),
            method,
            pad(path, 45),
            status,
            request_id
        )
        .expect("write to string");
    }
}

fn render_jobs_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<38} {:<16} {:<12} Updated",
        "Job", "Kind", "Status"
    )
    .expect("write to string");
    for item in items {
        let job_id = item.get("job_id").and_then(Value::as_str).unwrap_or("-");
        let kind = item.get("kind").and_then(Value::as_str).unwrap_or("-");
        let status = item.get("status").and_then(Value::as_str).unwrap_or("-");
        let updated = item
            .get("updated_at")
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp);
        writeln!(
            output,
            "{:<38} {:<16} {:<12} {}",
            pad(job_id, 38),
            pad(kind, 16),
            pad(status, 12),
            updated
        )
        .expect("write to string");
    }
}

fn render_artifacts_table(output: &mut String, items: &[Value]) {
    writeln!(output, "{:<58} {:>10} Modified", "Path", "Bytes").expect("write to string");
    for item in items {
        let path = item.get("path").and_then(Value::as_str).unwrap_or("-");
        let bytes = item
            .get("bytes")
            .and_then(Value::as_u64)
            .map_or_else(|| "-".to_string(), |value| value.to_string());
        let modified = item
            .get("modified")
            .and_then(Value::as_u64)
            .map_or_else(String::new, format_unix_timestamp);
        writeln!(output, "{:<58} {:>10} {}", pad(path, 58), bytes, modified)
            .expect("write to string");
    }
}

fn render_projects_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<28} {:<22} {:<20} {:<10} ID",
        "Project", "Slug", "Network", "Visibility"
    )
    .expect("write to string");
    for item in items {
        let name = item
            .get("project_name")
            .or_else(|| item.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let slug = item.get("slug").and_then(Value::as_str).unwrap_or("-");
        let network = first_project_network(item);
        let visibility = item
            .get("is_private")
            .and_then(Value::as_bool)
            .map_or("-", |private| if private { "private" } else { "public" });
        let id = item
            .get("project_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<28} {:<22} {:<20} {:<10} {}",
            pad(name, 28),
            pad(slug, 22),
            pad(&network, 20),
            visibility,
            id
        )
        .expect("write to string");
    }
}

fn first_project_network(item: &Value) -> String {
    item.get("chain_names")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .or_else(|| {
            item.get("project_networks")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
        })
        .map_or_else(|| "-".to_string(), human_scalar)
}

fn render_members_table(output: &mut String, items: &[Value]) {
    writeln!(output, "{:<34} {:<12} User ID", "Email", "Role").expect("write to string");
    for item in items {
        let email = item.get("email").and_then(Value::as_str).unwrap_or("-");
        let role = item.get("role").and_then(Value::as_str).unwrap_or("-");
        let user_id = item.get("user_id").and_then(Value::as_str).unwrap_or("-");
        writeln!(
            output,
            "{:<34} {:<12} {}",
            pad(email, 34),
            pad(role, 12),
            user_id
        )
        .expect("write to string");
    }
}

fn render_invitations_table(output: &mut String, items: &[Value]) {
    writeln!(output, "{:<34} {:<12} {:<16} ID", "Email", "Role", "Status")
        .expect("write to string");
    for item in items {
        let email = item
            .get("email")
            .or_else(|| item.get("identifier"))
            .or_else(|| item.get("invitee_identifier"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let role = item.get("role").and_then(Value::as_str).unwrap_or("-");
        let status = item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let id = item
            .get("id")
            .or_else(|| item.get("invitation_id"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<34} {:<12} {:<16} {}",
            pad(email, 34),
            pad(role, 12),
            pad(status, 16),
            id
        )
        .expect("write to string");
    }
}

fn render_releases_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<36} {:<14} {:<16} Created",
        "Release", "Environment", "Status"
    )
    .expect("write to string");
    for item in items {
        let id = item
            .get("release_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let environment = item
            .get("environment")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let status = item.get("status").and_then(Value::as_str).unwrap_or("-");
        let created = item
            .get("created_at")
            .or_else(|| item.get("createdAt"))
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp);
        writeln!(
            output,
            "{:<36} {:<14} {:<16} {}",
            pad(id, 36),
            pad(environment, 14),
            pad(status, 16),
            created
        )
        .expect("write to string");
    }
}

fn render_events_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<34} {:<14} {:<16} Type",
        "Event", "Environment", "Time"
    )
    .expect("write to string");
    for item in items {
        let id = item.get("id").and_then(Value::as_str).unwrap_or("-");
        let environment = item
            .get("environment")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let timestamp = item
            .get("timestamp")
            .or_else(|| item.get("created_at"))
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp);
        let kind = item
            .get("type")
            .or_else(|| item.get("event_type"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<34} {:<14} {:<16} {}",
            pad(id, 34),
            pad(environment, 14),
            pad(&timestamp, 16),
            kind
        )
        .expect("write to string");
    }
}

fn render_coverage_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<7} {:<45} {:<7} {:<7} Request ID",
        "Method", "Path", "Hits", "2xx"
    )
    .expect("write to string");
    for item in items.iter().take(20) {
        let method = item.get("method").and_then(Value::as_str).unwrap_or("-");
        let path = item.get("path").and_then(Value::as_str).unwrap_or("-");
        let hits = item.get("hits").and_then(Value::as_u64).unwrap_or(0);
        let ok = item.get("ok").and_then(Value::as_u64).unwrap_or(0);
        let request_id = item
            .get("latest_request_id")
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<7} {:<45} {:<7} {:<7} {}",
            method,
            pad(path, 45),
            hits,
            ok,
            request_id
        )
        .expect("write to string");
    }
    if items.len() > 20 {
        writeln!(output, "... {} more", items.len() - 20).expect("write to string");
    }
}

fn render_body_variant_table(output: &mut String, items: &[Value]) {
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("variant");
        writeln!(output, "- {name}").expect("write to string");
        if let Some(body) = item.get("body") {
            render_human_value(output, body, 4);
        }
    }
}

fn render_collection_meta(output: &mut String, meta: &Value) {
    let fetched_at = meta
        .get("fetchedAt")
        .or_else(|| meta.get("fetched_at"))
        .and_then(Value::as_str);
    let sources = meta.get("sources").and_then(Value::as_array);
    if fetched_at.is_none() && sources.is_none_or(Vec::is_empty) {
        return;
    }

    if let Some(fetched_at) = fetched_at {
        output.push_str("Updated: ");
        output.push_str(&format_timestamp(fetched_at));
        output.push('\n');
    }
    if let Some(sources) = sources {
        let source_names = sources
            .iter()
            .filter_map(Value::as_str)
            .map(human_source_name)
            .collect::<Vec<_>>()
            .join(", ");
        if !source_names.is_empty() {
            output.push_str("Source: ");
            output.push_str(&source_names);
            output.push('\n');
        }
    }
}

fn human_source_name(source: &str) -> String {
    match source {
        "offchain" => "Phylax platform index".to_string(),
        "onchain" => "on-chain data".to_string(),
        "cache" => "cache".to_string(),
        other => human_label(other),
    }
}

fn is_incident_collection(collection: &HumanCollection<'_>) -> bool {
    collection.name == "Incidents" || collection.items.iter().any(has_incident_shape)
}

fn has_incident_shape(value: &Value) -> bool {
    value.get("referenceId").is_some()
        || value.get("reference_id").is_some()
        || (value.get("timestamp").is_some()
            && value.get("network").is_some()
            && value.get("title").is_some())
}

fn render_incident_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<3} {:<16} {:<24} {:<29} ID",
        "#", "Time", "Network", "Title"
    )
    .expect("write to string");
    for (index, item) in items.iter().enumerate() {
        let timestamp = item
            .get("timestamp")
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp);
        let network = format_network(item.get("network"));
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled");
        let id = item.get("id").and_then(Value::as_str).unwrap_or("-");
        writeln!(
            output,
            "{:<3} {:<16} {:<24} {:<29} {}",
            index + 1,
            pad(&timestamp, 16),
            pad(&network, 24),
            pad(title, 29),
            id
        )
        .expect("write to string");
    }
}

fn render_generic_table(output: &mut String, items: &[Value]) {
    let columns = generic_columns(items);
    if columns.is_empty() {
        render_human_value(output, &Value::Array(items.to_vec()), 0);
        return;
    }

    write!(output, "{:<3}", "#").expect("write to string");
    for column in &columns {
        write!(output, " {:<22}", human_label(column)).expect("write to string");
    }
    output.push('\n');

    for (index, item) in items.iter().enumerate() {
        write!(output, "{:<3}", index + 1).expect("write to string");
        for column in &columns {
            let value = item.get(column).map_or_else(String::new, human_cell);
            write!(output, " {:<22}", pad(&value, 22)).expect("write to string");
        }
        output.push('\n');
    }
}

fn generic_columns(items: &[Value]) -> Vec<String> {
    let mut columns = Vec::new();
    for preferred in [
        "name",
        "title",
        "id",
        "status",
        "environment",
        "network",
        "timestamp",
        "createdAt",
        "updatedAt",
    ] {
        if items.iter().any(|item| item.get(preferred).is_some()) {
            columns.push(preferred.to_string());
        }
        if columns.len() == 4 {
            return columns;
        }
    }

    if columns.is_empty()
        && let Some(object) = items.first().and_then(Value::as_object)
    {
        columns.extend(object.keys().take(4).cloned());
    }
    columns
}

fn human_cell(value: &Value) -> String {
    match value {
        Value::Object(object) if object.contains_key("name") => {
            object
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| compact_json(value), ToString::to_string)
        }
        Value::Object(_) | Value::Array(_) => compact_json(value),
        _ => human_scalar(value),
    }
}

fn human_action_str(value: &str) -> String {
    if value.trim_start().starts_with("pcl ") {
        humanize_command(value)
    } else if matches!(
        value,
        "Use --toon for agent consumption or --json for strict JSON parsing"
            | "Use --json for strict JSON parsing"
    ) {
        String::new()
    } else if value == "Use --body-template when constructing mutation bodies" {
        "Use --body-template to start from an example request body".to_string()
    } else {
        value.to_string()
    }
}

fn humanize_command(command: &str) -> String {
    command
        .replace(" --format toon", "")
        .replace(" --toon", "")
        .replace("--toon ", "")
}

fn is_body_template_key(key: &str) -> bool {
    matches!(
        key,
        "project_name"
            | "project_description"
            | "profile_image_url"
            | "github_url"
            | "chain_id"
            | "is_private"
            | "is_dev"
            | "project_id"
            | "identifier"
            | "identifier_type"
            | "role"
            | "provider"
            | "webhook_url"
            | "routing_key"
            | "enabled"
            | "address"
            | "signature"
            | "nonce"
            | "tx_hash"
            | "contract_name"
            | "assertions"
            | "assertionsDir"
            | "contracts"
            | "environment"
            | "mode"
            | "new_manager_address"
            | "ponder_transfer_id"
            | "reason"
            | "notify"
    )
}

fn name_value_pairs(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| {
            let name = value.get("name").and_then(Value::as_str).unwrap_or("?");
            let rendered = value
                .get("value")
                .map_or_else(|| "none".to_string(), scalar_string);
            format!("{name}={rendered}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_actions_table(output: &mut String, actions: &[Value]) {
    writeln!(
        output,
        "{:<24} {:<7} {:<8} Path",
        "Action", "Auth", "Method"
    )
    .expect("write to string");
    for action in actions {
        let name = action.get("name").and_then(Value::as_str).unwrap_or("-");
        let auth = action
            .get("auth")
            .and_then(Value::as_bool)
            .map_or("-", |value| if value { "yes" } else { "no" });
        let method = action.get("method").and_then(Value::as_str).unwrap_or("-");
        let path = action.get("path").and_then(Value::as_str).unwrap_or("-");
        writeln!(
            output,
            "{:<24} {:<7} {:<8} {}",
            pad(name, 24),
            auth,
            method,
            path
        )
        .expect("write to string");
    }
}

fn render_action_detail(output: &mut String, action: &Value) {
    let name = action.get("name").and_then(Value::as_str).unwrap_or("-");
    writeln!(output, "Action: {name}").expect("write to string");
    if let (Some(method), Some(path)) = (
        action.get("method").and_then(Value::as_str),
        action.get("path").and_then(Value::as_str),
    ) {
        writeln!(output, "Request: {method} {path}").expect("write to string");
    }
    if let Some(auth) = action.get("auth").and_then(Value::as_bool) {
        writeln!(
            output,
            "Auth: {}",
            if auth { "required" } else { "not required" }
        )
        .expect("write to string");
    }
    if let Some(example) = action.get("example").and_then(Value::as_str) {
        writeln!(output, "Example: {}", humanize_command(example)).expect("write to string");
    }
    if let Some(flags) = action.get("required_flags").and_then(Value::as_array)
        && !flags.is_empty()
    {
        writeln!(output, "Required flags: {}", string_list(flags)).expect("write to string");
    }
    if let Some(flags) = action.get("optional_flags").and_then(Value::as_array)
        && !flags.is_empty()
    {
        writeln!(output, "Optional flags: {}", string_list(flags)).expect("write to string");
    }
}

fn string_list(values: &[Value]) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn format_duration(seconds: i64) -> String {
    if seconds < 0 {
        return "expired".to_string();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn render_human_summary(output: &mut String, data: &Value) {
    let display_data = data.get("data").unwrap_or(data);
    output.push('\n');
    if let Some(object) = display_data.as_object() {
        for (key, value) in object {
            if key.starts_with('_') {
                continue;
            }
            output.push_str(&human_label(key));
            output.push_str(": ");
            if is_scalar(value) {
                output.push_str(&human_scalar(value));
                output.push('\n');
            } else {
                output.push_str(&human_compact_summary(value));
                output.push('\n');
            }
        }
    } else {
        render_human_value(output, display_data, 0);
    }
}

fn render_human_request_id(output: &mut String, envelope: &Value) {
    let request_id = envelope
        .pointer("/response/request_id")
        .and_then(Value::as_str);
    let status = envelope.pointer("/response/status").and_then(Value::as_u64);
    if request_id.is_none() && status.is_none() {
        return;
    }

    output.push('\n');
    if let Some(request_id) = request_id {
        output.push_str("Request ID: ");
        output.push_str(request_id);
        if let Some(status) = status {
            write!(output, " (HTTP {status})").expect("write to string");
        }
        output.push('\n');
    } else if let Some(status) = status {
        writeln!(output, "HTTP status: {status}").expect("write to string");
    }
}

fn human_compact_summary(value: &Value) -> String {
    match value {
        Value::Array(values) => plural_count(values.len(), "item"),
        Value::Object(object) => {
            if object.is_empty() {
                return "empty object".to_string();
            }
            object
                .iter()
                .filter(|(key, _)| !key.starts_with('_'))
                .take(3)
                .map(|(key, value)| {
                    if is_scalar(value) {
                        format!("{}={}", human_label(key), human_scalar(value))
                    } else {
                        format!("{}={}", human_label(key), compact_json(value))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        _ => human_scalar(value),
    }
}

fn format_network(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    if let Some(name) = value.as_str() {
        return name.to_string();
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Unknown network");
    if let Some(chain_id) = value.get("chainId").and_then(Value::as_u64) {
        return format!("{name} ({chain_id})");
    }
    if let Some(chain_id) = value.get("chain_id").and_then(Value::as_u64) {
        return format!("{name} ({chain_id})");
    }
    name.to_string()
}

fn format_timestamp(value: &str) -> String {
    if value.len() >= 16 && value.as_bytes().get(10) == Some(&b'T') {
        return value[..16].replace('T', " ");
    }
    value.to_string()
}

fn format_unix_timestamp(value: u64) -> String {
    let Ok(seconds) = i64::try_from(value) else {
        return value.to_string();
    };
    chrono::DateTime::from_timestamp(seconds, 0).map_or_else(
        || value.to_string(),
        |timestamp| timestamp.format("%Y-%m-%d %H:%M").to_string(),
    )
}

fn human_label(value: &str) -> String {
    let words = split_label_words(value);
    let mut rendered = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let lower = word.to_ascii_lowercase();
        let text = match lower.as_str() {
            "id" => "ID".to_string(),
            "api" => "API".to_string(),
            "http" => "HTTP".to_string(),
            "url" => "URL".to_string(),
            "json" => "JSON".to_string(),
            "cli" => "CLI".to_string(),
            "pcl" => "PCL".to_string(),
            "uuid" => "UUID".to_string(),
            "tx" => "tx".to_string(),
            "github" => "GitHub".to_string(),
            "authmethod" => "auth method".to_string(),
            other if index == 0 => capitalize(other),
            other => other.to_string(),
        };
        rendered.push(text);
    }
    rendered.join(" ")
}

fn split_label_words(value: &str) -> Vec<String> {
    let normalized = value.replace(['_', '-'], " ");
    let mut words = Vec::new();
    for raw in normalized.split_whitespace() {
        let mut current = String::new();
        let chars = raw.chars().collect::<Vec<_>>();
        for (index, ch) in chars.iter().enumerate() {
            if index > 0
                && ch.is_uppercase()
                && chars
                    .get(index.saturating_sub(1))
                    .is_some_and(|previous| previous.is_lowercase() || previous.is_ascii_digit())
            {
                words.push(current);
                current = String::new();
            }
            current.push(*ch);
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn plural_count(count: impl std::fmt::Display, item: &str) -> String {
    let count = count.to_string();
    if count == "1" {
        format!("1 {item}")
    } else {
        format!("{count} {item}s")
    }
}

fn human_scalar(value: &Value) -> String {
    match value {
        Value::Bool(value) => yes_no(*value).to_string(),
        Value::String(value) => {
            if value.len() >= 16 && value.as_bytes().get(10) == Some(&b'T') {
                format_timestamp(value)
            } else {
                value.clone()
            }
        }
        _ => scalar_string(value),
    }
}

fn pad(value: &str, width: usize) -> String {
    let value = truncate(value, width);
    format!("{value:<width$}")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let prefix: String = value.chars().take(max_chars - 3).collect();
    format!("{prefix}...")
}

fn is_hex_blob(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("0x") else {
        return false;
    };
    hex.len() > 64 && hex.chars().all(|character| character.is_ascii_hexdigit())
}

fn render_human_value(output: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                write_indent(output, indent);
                output.push_str(key);
                output.push_str(": ");
                if is_scalar(value) {
                    output.push_str(&scalar_string(value));
                    output.push('\n');
                } else {
                    output.push('\n');
                    render_human_value(output, value, indent + 2);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                write_indent(output, indent);
                output.push_str("- ");
                if is_scalar(value) {
                    output.push_str(&scalar_string(value));
                    output.push('\n');
                } else {
                    output.push('\n');
                    render_human_value(output, value, indent + 2);
                }
            }
        }
        _ => {
            write_indent(output, indent);
            output.push_str(&scalar_string(value));
            output.push('\n');
        }
    }
}

fn write_indent(output: &mut String, indent: usize) {
    for _ in 0..indent {
        output.push(' ');
    }
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn scalar_string(value: &Value) -> String {
    match value {
        Value::Null => "none".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => compact_json(value),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
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
    let next_actions = if auth_required && !allow_unauthenticated && !stored_token_valid {
        vec![
            "pcl auth ensure --toon",
            "Authenticate before removing --dry-run",
            "Use --body-template when constructing mutation bodies",
        ]
    } else {
        let mut actions = vec![
            "Remove --dry-run to execute this request",
            "Use --toon for agent consumption or --json for strict JSON parsing",
        ];
        let method = data
            .pointer("/request/method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method_side_effecting(method) {
            actions.push("Use --body-template when constructing mutation bodies");
        }
        actions
    };
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

fn request_is_destructive(method: HttpMethod, path: &str) -> bool {
    method == HttpMethod::Delete
        || path.contains("/delete")
        || path.contains("/remove")
        || path.contains("/reject")
        || path.contains("/logout")
}

fn search_request(args: &SearchArgs) -> Result<WorkflowRequest, ApiCommandError> {
    if args.health {
        return Ok(WorkflowRequest::get(
            "/health",
            false,
            vec!["pcl search --system-status".to_string()],
        ));
    }
    if args.system_status {
        return Ok(WorkflowRequest::get(
            "/system-status",
            false,
            vec!["pcl search --stats".to_string()],
        ));
    }
    if args.stats {
        return Ok(WorkflowRequest::get(
            "/stats",
            false,
            vec!["pcl projects --limit 10".to_string()],
        ));
    }
    if args.whitelist {
        return Ok(WorkflowRequest::get(
            "/whitelist",
            true,
            vec!["pcl projects --mine".to_string()],
        ));
    }
    if args.verified_contract {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let chain_id = args.chain_id.ok_or_else(|| {
            ApiCommandError::InvalidWorkflowWithActions {
                message: "--verified-contract requires --chain-id".to_string(),
                next_actions: vec![
                    "pcl search --verified-contract --address <address> --chain-id <chain-id>"
                        .to_string(),
                    "pcl search --help".to_string(),
                ],
            }
        })?;
        let mut request = WorkflowRequest::get(
            "/web/verified-contract",
            false,
            vec!["pcl contracts --project <project-ref>".to_string()],
        );
        push_query_string_value(&mut request.query, "address", address);
        push_query(&mut request.query, "chainId", Some(chain_id));
        return Ok(request);
    }

    let query = args
        .query
        .as_deref()
        .or(args.term.as_deref())
        .filter(|query| !query.trim().is_empty())
        .ok_or_else(|| {
            ApiCommandError::InvalidWorkflowWithActions {
                message: "Search query is required unless you choose a specific search action"
                    .to_string(),
                next_actions: vec![
                    "pcl search <term>".to_string(),
                    "pcl search --query <term>".to_string(),
                    "pcl search --stats".to_string(),
                    "pcl search --help".to_string(),
                ],
            }
        })?;

    let mut request = WorkflowRequest::get(
        "/search",
        false,
        vec![
            "pcl projects --project <project-ref>".to_string(),
            "pcl contracts --project <project-ref>".to_string(),
        ],
    );
    push_query_string_value(&mut request.query, "query", query.to_string());
    Ok(request)
}

fn account_request(args: &AccountArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.accept_terms {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/accept-terms",
            true,
            body.or_else(|| Some(json!({}).to_string())),
            vec!["pcl account".to_string(), "pcl projects --mine".to_string()],
        ));
    }
    if args.logout {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/logout",
            true,
            body.or_else(|| Some(json!({}).to_string())),
            vec!["pcl auth logout".to_string()],
        ));
    }
    Ok(WorkflowRequest::get(
        "/web/auth/me",
        true,
        vec![
            "pcl account --accept-terms".to_string(),
            "pcl projects --mine".to_string(),
        ],
    ))
}

fn contracts_request(args: &ContractsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/assertion_adopters",
            true,
            body,
            vec!["pcl contracts --unassigned --manager <manager-address>".to_string()],
        ));
    }
    if args.assign_project {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/assertion_adopters/assign-project",
            true,
            body,
            vec!["pcl contracts --project <project-ref>".to_string()],
        ));
    }
    if args.unassigned {
        let manager = required_arg(args.manager.as_deref(), "--manager")?;
        let mut request = WorkflowRequest::get(
            "/assertion_adopters/no-project",
            true,
            vec!["pcl contracts --assign-project --body-template".to_string()],
        );
        push_query_string_value(&mut request.query, "manager", manager);
        return Ok(request);
    }
    if args.remove_calldata {
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        if args.assertion_ids.is_empty() {
            return Err(ApiCommandError::InvalidWorkflow {
                message: "--assertion-id is required for --remove-calldata".to_string(),
            });
        }
        let mut request = WorkflowRequest::get(
            format!("/assertion_adopters/{address}/remove-assertions-calldata"),
            true,
            vec!["pcl releases --project <project-ref>".to_string()],
        );
        push_query_string(&mut request.query, "network", args.network.as_deref());
        push_query_string(
            &mut request.query,
            "environment",
            args.environment.as_deref(),
        );
        for assertion_id in &args.assertion_ids {
            push_query_string_value(&mut request.query, "assertion_ids", assertion_id.clone());
        }
        return Ok(request);
    }
    if args.remove {
        let project = required_arg(args.project.as_deref(), "--project")?;
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/{address}"),
            true,
            body,
            vec![format!("pcl contracts --project {project}")],
        ));
    }
    if let Some(project) = &args.project {
        if let Some(adopter_id) = &args.adopter_id {
            return Ok(WorkflowRequest::get(
                format!("/views/projects/{project}/contracts/{adopter_id}"),
                true,
                vec![format!("pcl contracts --project {project}")],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/views/projects/{project}/contracts"),
            true,
            vec![format!(
                "pcl contracts --project {project} --adopter-id <adopter-id>"
            )],
        ));
    }

    Ok(WorkflowRequest::get(
        "/assertion_adopters",
        true,
        vec!["pcl contracts --unassigned --manager <manager-address>".to_string()],
    ))
}

fn releases_request(args: &ReleasesArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "releases", "--project")?;
    if args.preview {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases/preview"),
            true,
            body,
            vec![format!(
                "pcl releases --project {project} --create --body-file release.json"
            )],
        ));
    }
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases"),
            true,
            body,
            vec![format!("pcl releases --project {project}")],
        ));
    }
    if args.deploy
        || args.remove
        || args.deploy_calldata
        || args.remove_calldata
        || args.backtest_progress
        || args.retry_check
    {
        let release_id = required_arg(args.release_id.as_deref(), "--release-id")?;
        if args.backtest_progress {
            return Ok(WorkflowRequest::get(
                format!("/projects/{project}/releases/{release_id}/backtest-progress"),
                true,
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id}"
                )],
            ));
        }
        if args.retry_check {
            let check_id = required_arg(args.check_id.as_deref(), "--check-id")?;
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/checks/{check_id}/retry"),
                true,
                body.or_else(|| Some(empty_json_body())),
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id} --backtest-progress"
                )],
            ));
        }
        if args.deploy {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/deploy"),
                true,
                body,
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id}"
                )],
            ));
        }
        if args.remove {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/remove"),
                true,
                body,
                vec![format!("pcl releases --project {project}")],
            ));
        }
        if args.deploy_calldata {
            let signer_address = required_arg(args.signer_address.as_deref(), "--signer-address")?;
            let mut request = WorkflowRequest::get(
                format!("/projects/{project}/releases/{release_id}/deploy-calldata"),
                true,
                vec![format!(
                    "pcl releases --project {project} --release-id {release_id} --deploy"
                )],
            );
            push_query_string_value(&mut request.query, "signerAddress", signer_address);
            return Ok(request);
        }
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases/{release_id}/remove-calldata"),
            true,
            vec![format!(
                "pcl releases --project {project} --release-id {release_id} --remove"
            )],
        ));
    }
    let Some(release_id) = &args.release_id else {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases"),
            true,
            vec![format!(
                "pcl releases --project {project} --release-id <release-id>"
            )],
        ));
    };
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/releases/{release_id}"),
        true,
        vec![
            format!(
                "pcl releases --project {project} --release-id {release_id} --deploy-calldata --signer-address <signer-address>"
            ),
            format!("pcl releases --project {project} --release-id {release_id} --remove-calldata"),
        ],
    ))
}

fn deployments_request(args: &DeploymentsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "deployments", "--project")?;
    if args.confirm {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/confirm-deployment"),
            true,
            body,
            vec![format!("pcl deployments --project {project}")],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/views/projects/{project}/deployments"),
        true,
        vec![format!("pcl releases --project {project}")],
    ))
}

fn access_request(args: &AccessArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.pending {
        return Ok(WorkflowRequest::get(
            "/invitations/pending",
            true,
            vec!["pcl access --token <token> --accept".to_string()],
        ));
    }
    if args.accept || args.preview {
        let token = required_arg(args.token.as_deref(), "--token")?;
        if args.accept {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/invitations/{token}/accept"),
                true,
                body.or_else(|| Some(empty_json_body())),
                vec!["pcl projects --mine".to_string()],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl access --token {token} --accept")],
        ));
    }
    if let Some(token) = &args.token {
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl access --token {token} --accept")],
        ));
    }
    let project = required_project_arg(args.project.as_deref(), "access", "--project")?;
    if args.my_role {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/my-role"),
            true,
            vec![format!("pcl access --project {project} --members")],
        ));
    }
    if args.invite {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/invitations"),
            true,
            body,
            vec![format!("pcl access --project {project} --invitations")],
        ));
    }
    if args.resend || args.revoke {
        let invitation_id = required_arg(args.invitation_id.as_deref(), "--invitation-id")?;
        if args.resend {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/invitations/{invitation_id}/resend"),
                true,
                body.or_else(|| Some(empty_json_body())),
                vec![format!("pcl access --project {project} --invitations")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/invitations/{invitation_id}"),
            true,
            body,
            vec![format!("pcl access --project {project} --invitations")],
        ));
    }
    if args.update_role || args.remove {
        let member_user_id = required_arg(args.member_user_id.as_deref(), "--member-user-id")?;
        if args.update_role {
            return Ok(workflow_with_body(
                HttpMethod::Patch,
                format!("/projects/{project}/members/{member_user_id}"),
                true,
                body,
                vec![format!("pcl access --project {project} --members")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/members/{member_user_id}"),
            true,
            body,
            vec![format!("pcl access --project {project} --members")],
        ));
    }
    if args.invitations {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/invitations"),
            true,
            vec![format!(
                "pcl access --project {project} --invite --body-template"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/members"),
        true,
        vec![
            format!("pcl access --project {project} --my-role"),
            format!("pcl access --project {project} --invitations"),
        ],
    ))
}

fn integrations_request(args: &IntegrationsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "integrations", "--project")?;
    let Some(provider) = args.provider else {
        return Err(ApiCommandError::InvalidWorkflowWithActions {
            message: "--provider is required".to_string(),
            next_actions: vec![
                "pcl integrations --project <project-id> --provider slack".to_string(),
                "pcl integrations --project <project-id> --provider pagerduty".to_string(),
                "pcl integrations --help".to_string(),
            ],
        });
    };
    let provider = provider.path();
    let base = format!("/projects/{project}/integrations/{provider}");
    if args.configure {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    if args.test {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("{base}/test"),
            true,
            body.or_else(|| Some(empty_json_body())),
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    if args.delete {
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            base,
            true,
            body,
            vec![format!(
                "pcl integrations --project {project} --provider {provider}"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        base,
        true,
        vec![
            format!("pcl integrations --project {project} --provider {provider} --test"),
            format!(
                "pcl integrations --project {project} --provider {provider} --configure --body-template"
            ),
        ],
    ))
}

fn protocol_manager_request(
    args: &ProtocolManagerArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    let project = required_project_arg(args.project.as_deref(), "protocol-manager", "--project")?;
    let base = format!("/projects/{project}/protocol-manager");
    if args.nonce {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let mut request = WorkflowRequest::get(
            format!("{base}/nonce"),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --set --body-template"
            )],
        );
        push_query_string_value(&mut request.query, "address", address);
        push_query(&mut request.query, "chain_id", args.chain_id);
        return Ok(request);
    }
    if args.set {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --pending-transfer"
            )],
        ));
    }
    if args.clear {
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            base,
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --nonce --address <manager-address>"
            )],
        ));
    }
    if args.transfer_calldata {
        let new_manager = required_arg(args.new_manager.as_deref(), "--new-manager")?;
        let mut request = WorkflowRequest::get(
            format!("{base}/transfer-calldata"),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --set --body-template"
            )],
        );
        push_query_string_value(&mut request.query, "new_manager", new_manager);
        return Ok(request);
    }
    if args.accept_calldata {
        return Ok(WorkflowRequest::get(
            format!("{base}/accept-calldata"),
            true,
            vec![format!(
                "pcl protocol-manager --project {project} --confirm-transfer --body-template"
            )],
        ));
    }
    if args.confirm_transfer {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("{base}/confirm-transfer"),
            true,
            body,
            vec![format!(
                "pcl protocol-manager --project {project} --pending-transfer"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("{base}/pending-transfer"),
        true,
        vec![
            format!("pcl protocol-manager --project {project} --nonce --address <manager-address>"),
            format!(
                "pcl protocol-manager --project {project} --transfer-calldata --new-manager <manager-address>"
            ),
        ],
    ))
}

fn transfers_request(args: &TransfersArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), args.body_file.as_ref(), &args.field)?;
    if args.reject {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/transfers/reject",
            true,
            body,
            vec!["pcl transfers --pending".to_string()],
        ));
    }
    if let Some(transfer_id) = &args.transfer_id {
        return Ok(WorkflowRequest::get(
            format!("/views/transfers/{transfer_id}"),
            true,
            vec!["pcl transfers --pending".to_string()],
        ));
    }
    Ok(WorkflowRequest::get(
        "/views/transfers/pending",
        true,
        vec!["pcl transfers --transfer-id <transfer-id>".to_string()],
    ))
}

fn events_request(args: &EventsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let project = required_project_arg(args.project.as_deref(), "events", "--project")?;
    let mut request = if args.audit_log {
        WorkflowRequest::get(
            format!("/views/projects/{project}/audit-log"),
            true,
            vec![format!("pcl events --project {project}")],
        )
    } else {
        WorkflowRequest::get(
            format!("/views/projects/{project}/events"),
            true,
            vec![format!("pcl events --project {project} --audit-log")],
        )
    };
    push_query(&mut request.query, "page", args.page);
    push_query(&mut request.query, "limit", args.limit);
    push_query_string(
        &mut request.query,
        "environment",
        args.environment.as_deref(),
    );
    Ok(request)
}

fn workflow_with_body(
    method: HttpMethod,
    path: impl Into<String>,
    require_auth: bool,
    body: Option<String>,
    next_actions: Vec<String>,
) -> WorkflowRequest {
    WorkflowRequest {
        method,
        path: path.into(),
        query: Vec::new(),
        body,
        require_auth,
        next_actions,
    }
}

fn empty_json_body() -> String {
    json!({}).to_string()
}

fn request_body(
    body: Option<&str>,
    body_file: Option<&PathBuf>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(body, body_file)?;
    body_with_fields(body, fields)
}

fn project_request_body(args: &ProjectsArgs) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(args.body.as_deref(), args.body_file.as_ref())?;
    let mut object = match body {
        Some(body) => serde_json::from_str::<Value>(&body)?,
        None => Value::Object(Map::new()),
    };
    let Value::Object(map) = &mut object else {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "project body must be a JSON object".to_string(),
        });
    };

    insert_optional(
        map,
        "project_name",
        args.project_name.clone().map(Value::String),
    );
    insert_optional(
        map,
        "project_description",
        args.project_description.clone().map(Value::String),
    );
    insert_optional(
        map,
        "profile_image_url",
        args.profile_image_url.clone().map(Value::String),
    );
    insert_optional(
        map,
        "github_url",
        args.github_url.clone().map(Value::String),
    );
    insert_optional(map, "chain_id", args.chain_id.map(|value| json!(value)));
    insert_optional(map, "is_private", args.is_private.map(|value| json!(value)));
    insert_optional(map, "is_dev", args.is_dev.map(|value| json!(value)));
    apply_fields(map, &args.field)?;

    if map.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(map.clone()).to_string()))
    }
}

fn body_with_fields(
    body: Option<String>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    if fields.is_empty() {
        return Ok(body);
    }
    let mut value = match body {
        Some(body) => serde_json::from_str::<Value>(&body)?,
        None => Value::Object(Map::new()),
    };
    let Value::Object(map) = &mut value else {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--field requires the request body to be a JSON object".to_string(),
        });
    };
    apply_fields(map, fields)?;
    Ok(Some(Value::Object(map.clone()).to_string()))
}

fn apply_fields(map: &mut Map<String, Value>, fields: &[String]) -> Result<(), ApiCommandError> {
    for field in fields {
        let (key, value) = field.split_once('=').ok_or_else(|| {
            ApiCommandError::InvalidKeyValue {
                kind: "field",
                input: field.clone(),
            }
        })?;
        map.insert(key.to_string(), parse_field_value(value));
    }
    Ok(())
}

fn parse_field_value(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()))
}

fn insert_optional(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

fn template_envelope(data: Value) -> Value {
    let next_actions = if data
        .get("body_variants")
        .and_then(Value::as_array)
        .is_some_and(|variants| !variants.is_empty())
    {
        vec![
            "Choose one entry from data.body_variants and pass only its body with --body-file <path>",
            "Or pass fields from the chosen variant body with --field key=value",
        ]
    } else {
        vec![
            "Pass the template with --body-file <path>",
            "Or pass individual fields with --field key=value",
        ]
    };
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "next_actions": next_actions,
    }))
}

fn project_body_template(args: &ProjectsArgs) -> Value {
    if args.update {
        return body_template("project_update");
    }
    if args.save || args.unsave {
        return body_template("project_saved");
    }
    if args.delete || args.resolve || args.widget || args.mine || args.saved {
        return body_template("empty_object");
    }
    body_template("project_create")
}

fn assertions_body_template(_args: &AssertionsArgs) -> Value {
    body_template("empty_object")
}

fn account_body_template(_args: &AccountArgs) -> Value {
    body_template("empty_object")
}

fn contracts_body_template(args: &ContractsArgs) -> Value {
    if args.assign_project {
        return body_template("contracts_assign_project");
    }
    if args.unassigned || args.remove || args.remove_calldata || args.adopter_id.is_some() {
        return body_template("empty_object");
    }
    body_template("contracts")
}

fn release_body_template(args: &ReleasesArgs) -> Value {
    if args.deploy {
        return body_template("release_deploy");
    }
    if args.remove {
        return body_template("release_remove");
    }
    if args.deploy_calldata
        || args.remove_calldata
        || args.backtest_progress
        || args.retry_check
        || args.release_id.is_some()
    {
        return body_template("empty_object");
    }
    body_template("release")
}

fn deployment_body_template(args: &DeploymentsArgs) -> Value {
    if !args.confirm {
        return body_template("empty_object");
    }
    body_template("deployment_confirmation")
}

fn access_body_template(args: &AccessArgs) -> Value {
    if args.update_role {
        return body_template("role_update");
    }
    if args.invite {
        return body_template("access_invite");
    }
    if args.accept
        || args.resend
        || args.revoke
        || args.remove
        || args.members
        || args.invitations
        || args.pending
        || args.preview
        || args.my_role
    {
        return body_template("empty_object");
    }
    body_template("access_invite")
}

fn integration_body_template(args: &IntegrationsArgs) -> Value {
    if args.test || args.delete {
        return body_template("empty_object");
    }
    if let Some(provider) = args.provider {
        return body_template(provider.path());
    }
    json!({
        "body_variants": [
            {
                "name": "slack",
                "body": body_template("slack")
            },
            {
                "name": "pagerduty",
                "body": body_template("pagerduty")
            }
        ]
    })
}

fn protocol_manager_body_template(args: &ProtocolManagerArgs) -> Value {
    if args.set {
        return body_template("protocol_manager_set");
    }
    if args.confirm_transfer {
        return body_template("protocol_manager_confirm");
    }
    if args.clear
        || args.nonce
        || args.transfer_calldata
        || args.accept_calldata
        || args.pending_transfer
    {
        return body_template("empty_object");
    }
    body_template("protocol_manager_set")
}

fn transfer_body_template(args: &TransfersArgs) -> Value {
    if !args.reject {
        return body_template("empty_object");
    }
    body_template("transfer_reject")
}

fn body_template(kind: &str) -> Value {
    match kind {
        "project_create" => {
            json!({
                "project_name": "<name>",
                "chain_id": 1,
                "project_description": "<description>",
                "profile_image_url": "https://example.com/project.png",
                "is_private": false
            })
        }
        "project_update" => {
            json!({
                "project_name": "<name>",
                "project_description": "<description>",
                "github_url": "https://github.com/org/repo",
                "profile_image_url": "https://example.com/project.png",
                "is_dev": false,
                "is_private": false,
                "assertion_adopters": []
            })
        }
        "project_saved" => json!({ "project_id": "<project-uuid>" }),
        "release" => {
            json!({
                "environment": "staging",
                "assertionsDir": "assertions",
                "contracts": {
                    "<contract-key>": {
                        "address": "0x...",
                        "name": "<contract-name>",
                        "assertions": [
                            {
                                "file": "Assertion.sol",
                                "args": [],
                                "bytecode": "0x...",
                                "flattenedSource": "<source>",
                                "compilerVersion": "0.8.28",
                                "contractName": "<assertion-contract>",
                                "evmVersion": "paris",
                                "optimizerRuns": 200,
                                "optimizerEnabled": true,
                                "metadataBytecodeHash": "none",
                                "libraries": {}
                            }
                        ]
                    }
                },
                "compilerArgs": []
            })
        }
        "access_invite" => {
            json!({
                "identifier": "user@example.com",
                "identifier_type": "email",
                "role": "viewer"
            })
        }
        "role_update" => json!({ "role": "viewer" }),
        "release_deploy" => {
            json!({
                "chainId": 1,
                "txHash": "0x..."
            })
        }
        "release_remove" => {
            json!({
                "chainId": 1,
                "txHash": "0x..."
            })
        }
        "deployment_confirmation" => {
            json!({
                "tx_hash": "0x...",
                "chainId": 1,
                "environment": "staging",
                "assertions": [
                    {
                        "assertion_id": "0x...",
                        "assertion_adopters": [
                            {
                                "id": "<adopter-id>"
                            }
                        ]
                    }
                ]
            })
        }
        "slack" => {
            json!({
                "webhook_url": "https://hooks.slack.com/services/...",
                "enabled": true
            })
        }
        "pagerduty" => {
            json!({
                "routing_key": "<pagerduty-routing-key>",
                "enabled": true
            })
        }
        "protocol_manager_set" => {
            json!({
                "address": "0x...",
                "signature": "0x...",
                "nonce": "<nonce>"
            })
        }
        "protocol_manager_confirm" => {
            json!({
                "body_variants": [
                    {
                        "name": "direct",
                        "body": {
                            "mode": "direct",
                            "new_manager_address": "0x..."
                        }
                    },
                    {
                        "name": "onchain",
                        "body": {
                            "mode": "onchain",
                            "new_manager_address": "0x...",
                            "chain_id": 1,
                            "tx_hash": "0x..."
                        }
                    }
                ]
            })
        }
        "transfer_reject" => {
            json!({
                "ponder_transfer_id": "<transfer-id>"
            })
        }
        "contracts" => {
            json!({
                "network": "1",
                "address": "0x...",
                "contract_name": "<contract-name>",
                "project_id": "<project-uuid>"
            })
        }
        "contracts_assign_project" => {
            json!({
                "project_id": "<project-uuid>",
                "assertion_adopter_ids": ["<adopter-id>"]
            })
        }
        "empty_object" => json!({}),
        _ => json!({}),
    }
}

fn required_arg(value: Option<&str>, name: &str) -> Result<String, ApiCommandError> {
    value.map(ToString::to_string).ok_or_else(|| {
        ApiCommandError::InvalidWorkflow {
            message: format!("{name} is required"),
        }
    })
}

fn required_arg_with_actions(
    value: Option<&str>,
    name: &str,
    next_actions: Vec<String>,
) -> Result<String, ApiCommandError> {
    value.map(ToString::to_string).ok_or_else(|| {
        ApiCommandError::InvalidWorkflowWithActions {
            message: format!("{name} is required"),
            next_actions,
        }
    })
}

fn required_project_arg(
    value: Option<&str>,
    command: &str,
    flag: &str,
) -> Result<String, ApiCommandError> {
    required_arg_with_actions(
        value,
        flag,
        vec![
            "pcl projects --mine".to_string(),
            format!("pcl {command} {flag} <project-id>"),
            format!("pcl {command} --help"),
        ],
    )
}

fn push_query_string_value(query: &mut Vec<(String, String)>, name: &str, value: String) {
    query.push((name.to_string(), value));
}

fn project_segment(path: &str) -> Option<(&'static str, &str, &str)> {
    if let Some(rest) = path.strip_prefix("/projects/") {
        let (segment, suffix) = split_first_segment(rest);
        if matches!(segment, "saved" | "resolve") {
            return None;
        }
        return Some(("/projects/", segment, suffix));
    }
    if let Some(rest) = path.strip_prefix("/views/projects/") {
        let (segment, suffix) = split_first_segment(rest);
        if segment == "home" {
            return None;
        }
        return Some(("/views/projects/", segment, suffix));
    }
    None
}

fn split_first_segment(path: &str) -> (&str, &str) {
    path.split_once('/').map_or((path, ""), |(segment, _rest)| {
        (segment, &path[segment.len()..])
    })
}

fn incidents_request(args: &IncidentsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    if args.all && (args.incident_id.is_some() || args.stats || args.retry_trace) {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--all is only supported for incident list workflows".to_string(),
        });
    }
    if args.stats && args.project_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--stats requires --project-id".to_string(),
        });
    }
    if args.tx_id.is_some() && args.incident_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--tx-id requires --incident-id".to_string(),
        });
    }
    if args.retry_trace && args.tx_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--retry-trace requires --incident-id and --tx-id".to_string(),
        });
    }

    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);

    if let Some(incident_id) = &args.incident_id {
        if args.retry_trace {
            let tx_id = required_arg(args.tx_id.as_deref(), "--tx-id")?;
            return Ok(WorkflowRequest {
                method: HttpMethod::Post,
                path: format!("/incidents/{incident_id}/transactions/{tx_id}/trace/retry"),
                query,
                body: Some("{}".to_string()),
                require_auth: true,
                next_actions: vec![format!(
                    "pcl incidents --incident-id {incident_id} --tx-id {tx_id}"
                )],
            });
        }
        let path = if let Some(tx_id) = &args.tx_id {
            format!("/views/incidents/{incident_id}/transactions/{tx_id}/trace")
        } else {
            format!("/views/incidents/{incident_id}")
        };
        let next_actions = vec![
            "pcl incidents --limit 5".to_string(),
            format!("pcl api inspect get {}", path),
        ];
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path,
            query,
            body: None,
            require_auth: true,
            next_actions,
        });
    }

    if let Some(project_id) = &args.project_id {
        if args.stats {
            let path = format!("/projects/{project_id}/incidents/stats");
            return Ok(WorkflowRequest {
                method: HttpMethod::Get,
                path,
                query,
                body: None,
                require_auth: true,
                next_actions: vec![format!(
                    "pcl incidents --project-id {project_id} --limit 10"
                )],
            });
        }
        push_query_string(&mut query, "assertionId", args.assertion_id.as_deref());
        push_query_string(
            &mut query,
            "assertionAdopterId",
            args.assertion_adopter_id.as_deref(),
        );
        push_query_string(&mut query, "environment", args.environment.as_deref());
        push_query_string(&mut query, "fromDate", args.from_date.as_deref());
        push_query_string(&mut query, "toDate", args.to_date.as_deref());
        let path = format!("/views/projects/{project_id}/incidents");
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path,
            query,
            body: None,
            require_auth: true,
            next_actions: vec![
                format!("pcl assertions --project-id {project_id}"),
                "pcl incidents --limit 5".to_string(),
            ],
        });
    }

    push_query(&mut query, "network", args.network);
    push_query_string(&mut query, "sort", args.sort.as_deref());
    push_query_string(&mut query, "devMode", args.dev_mode.as_deref());
    Ok(WorkflowRequest {
        method: HttpMethod::Get,
        path: "/views/public/incidents".to_string(),
        query,
        body: None,
        require_auth: false,
        next_actions: vec![
            "pcl incidents --project-id <project-id> --limit 10".to_string(),
            "pcl projects --limit 10".to_string(),
        ],
    })
}

fn incidents_next_actions(
    data: &Value,
    args: &IncidentsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if let Some(incident_id) = &args.incident_id {
        if args.tx_id.is_none()
            && let Some(tx_id) = data
                .get("data")
                .and_then(|data| data.get("invalidating_transactions"))
                .and_then(Value::as_array)
                .and_then(|transactions| transactions.first())
                .and_then(|transaction| {
                    first_string_field(transaction, &["transaction_hash", "id", "tx_id"])
                })
        {
            return vec![
                format!("pcl incidents --incident-id {incident_id} --tx-id {tx_id}"),
                "pcl incidents --limit 5".to_string(),
            ];
        }
        return fallback;
    }
    first_string_field(data, &["id", "incidentId", "incident_id"]).map_or(fallback, |incident_id| {
        vec![
            format!("pcl incidents --incident-id {incident_id}"),
            "pcl projects --limit 10".to_string(),
        ]
    })
}

fn projects_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    if let Some(project_id) = data.get("project_id").and_then(Value::as_str) {
        return vec![
            format!("pcl assertions --project-id {project_id}"),
            format!("pcl incidents --project-id {project_id} --limit 10"),
        ];
    }
    first_string_field(data, &["project_id", "projectId", "id"]).map_or(fallback, |project_id| {
        vec![
            format!("pcl projects --project-id {project_id}"),
            format!("pcl assertions --project-id {project_id}"),
            format!("pcl incidents --project-id {project_id} --limit 10"),
        ]
    })
}

fn assertions_next_actions(
    data: &Value,
    args: &AssertionsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    let Some(project_id) = &args.project_id else {
        return first_string_field(
            data,
            &["assertion_adopter_address", "adopter_address", "address"],
        )
        .map_or(fallback, |address| {
            vec![format!("pcl assertions --adopter-address {address}")]
        });
    };

    first_string_field(data, &["assertion_id", "assertionId", "id"]).map_or(
        fallback,
        |assertion_id| {
            vec![
                format!("pcl assertions --project-id {project_id} --assertion-id {assertion_id}",),
                format!("pcl incidents --project-id {project_id} --assertion-id {assertion_id}",),
            ]
        },
    )
}

fn search_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    if let Some(project_id) = data
        .get("projects")
        .and_then(Value::as_array)
        .and_then(|projects| projects.first())
        .and_then(|project| first_string_field(project, &["project_id", "projectId", "id", "slug"]))
    {
        return vec![
            format!("pcl projects --project-id {project_id}"),
            format!("pcl contracts --project {project_id}"),
        ];
    }
    if let Some(project_id) = data
        .get("contracts")
        .and_then(Value::as_array)
        .and_then(|contracts| contracts.first())
        .and_then(|contract| {
            contract.get("data").map_or_else(
                || first_string_field(contract, &["related_project_id", "related_project_slug"]),
                |inner| first_string_field(inner, &["related_project_id", "related_project_slug"]),
            )
        })
    {
        return vec![
            format!("pcl projects --project-id {project_id}"),
            format!("pcl contracts --project {project_id}"),
        ];
    }
    fallback
}

fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(object) => {
            for key in keys {
                if let Some(value) = object.get(*key).and_then(Value::as_str) {
                    return Some(value.to_string());
                }
            }
            object
                .values()
                .find_map(|value| first_string_field(value, keys))
        }
        Value::Array(values) => {
            values
                .iter()
                .find_map(|value| first_string_field(value, keys))
        }
        _ => None,
    }
}

fn projects_request(args: &ProjectsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);
    push_query_string(&mut query, "search", args.search.as_deref());
    let body = project_request_body(args)?;

    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/projects",
            true,
            body,
            vec!["pcl projects --mine".to_string()],
        ));
    }

    if args.mine {
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: "/views/projects/home".to_string(),
            query,
            body: None,
            require_auth: true,
            next_actions: vec![
                "pcl account".to_string(),
                "pcl projects --saved --user-id <user-id>".to_string(),
            ],
        });
    }
    if args.saved {
        let user_id = required_arg(args.user_id.as_deref(), "--user-id")?;
        push_query_string_value(&mut query, "user_id", user_id);
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: "/projects/saved".to_string(),
            query,
            body: None,
            require_auth: true,
            next_actions: vec!["pcl projects --mine".to_string()],
        });
    }
    if args.project_id.is_none()
        && (args.update || args.delete || args.save || args.unsave || args.resolve || args.widget)
    {
        required_project_arg(args.project_id.as_deref(), "projects", "--project-id")?;
    }
    if let Some(project_id) = &args.project_id {
        if args.resolve {
            return Ok(WorkflowRequest {
                method: HttpMethod::Get,
                path: format!("/projects/resolve/{project_id}"),
                query,
                body: None,
                require_auth: false,
                next_actions: vec![format!("pcl projects --project-id {project_id}")],
            });
        }
        if args.widget {
            return Ok(WorkflowRequest::get(
                format!("/projects/{project_id}/widget"),
                true,
                vec![format!("pcl projects --project-id {project_id}")],
            ));
        }
        if args.save || args.unsave {
            return Ok(workflow_with_body(
                if args.save {
                    HttpMethod::Post
                } else {
                    HttpMethod::Delete
                },
                "/projects/saved",
                true,
                Some(json!({ "project_id": project_id }).to_string()),
                vec![
                    format!("pcl projects --project-id {project_id}"),
                    "pcl projects --mine".to_string(),
                ],
            ));
        }
        if args.update {
            return Ok(workflow_with_body(
                HttpMethod::Put,
                format!("/projects/{project_id}"),
                true,
                body,
                vec![format!("pcl projects --project-id {project_id}")],
            ));
        }
        if args.delete {
            return Ok(workflow_with_body(
                HttpMethod::Delete,
                format!("/projects/{project_id}"),
                true,
                body,
                vec!["pcl projects --mine".to_string()],
            ));
        }
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: format!("/projects/{project_id}"),
            query,
            body: None,
            require_auth: true,
            next_actions: vec![
                format!("pcl assertions --project-id {project_id}"),
                format!("pcl incidents --project-id {project_id} --limit 10"),
            ],
        });
    }

    Ok(WorkflowRequest {
        method: HttpMethod::Get,
        path: "/views/projects".to_string(),
        query,
        body: None,
        require_auth: false,
        next_actions: vec![
            "pcl projects --project-id <project-id>".to_string(),
            "pcl incidents --limit 5".to_string(),
        ],
    })
}

fn assertions_request(args: &AssertionsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    if args.submit || args.submitted {
        return Err(ApiCommandError::InvalidWorkflow {
            message:
                "Submitted assertions have been removed from the API; use releases and registered assertions instead"
                    .to_string(),
        });
    }

    if let Some(adopter_address) = &args.adopter_address {
        let mut request = WorkflowRequest::get(
            "/assertions",
            false,
            vec!["pcl contracts --project <project-ref>".to_string()],
        );
        push_query_string_value(
            &mut request.query,
            "adopter_address",
            adopter_address.clone(),
        );
        push_query_string(&mut request.query, "network", args.network.as_deref());
        push_query_string(
            &mut request.query,
            "environment",
            args.environment.as_deref(),
        );
        push_query(
            &mut request.query,
            "include_onchain_only",
            args.include_onchain_only,
        );
        return Ok(request);
    }

    let project_id =
        required_project_arg(args.project_id.as_deref(), "assertions", "--project-id")?;
    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);
    push_query_string(&mut query, "assertionAdopterId", args.adopter_id.as_deref());
    push_query_string(&mut query, "environment", args.environment.as_deref());

    if args.registered {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/registered-assertions"),
            true,
            vec![format!("pcl assertions --project-id {project_id}")],
        ));
    }
    if args.remove_info {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/remove-assertions-info"),
            true,
            vec![format!(
                "pcl assertions --project-id {project_id} --remove-calldata"
            )],
        ));
    }
    if args.remove_calldata {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/remove-assertions-calldata"),
            true,
            vec![format!("pcl releases --project {project_id}")],
        ));
    }

    if let Some(assertion_id) = &args.assertion_id {
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: format!("/views/projects/{project_id}/assertions/{assertion_id}"),
            query,
            body: None,
            require_auth: true,
            next_actions: vec![format!(
                "pcl incidents --project-id {project_id} --assertion-id {assertion_id}",
            )],
        });
    }

    Ok(WorkflowRequest {
        method: HttpMethod::Get,
        path: format!("/views/projects/{project_id}/assertions"),
        query,
        body: None,
        require_auth: true,
        next_actions: vec![
            format!("pcl incidents --project-id {project_id} --limit 10"),
            format!("pcl assertions --project-id {project_id} --assertion-id <assertion-id>"),
        ],
    })
}

fn push_query<T: ToString>(query: &mut Vec<(String, String)>, name: &str, value: Option<T>) {
    if let Some(value) = value {
        query.push((name.to_string(), value.to_string()));
    }
}

fn push_query_string(query: &mut Vec<(String, String)>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        query.push((name.to_string(), value.to_string()));
    }
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

/// Render a JSON value as the CLI's compact TOON-style text output.
pub fn toon_string(value: &Value) -> String {
    let mut output = toon_format::encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    });
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
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

#[derive(Clone, Debug)]
struct OperationCoverage {
    operation_id: String,
    method: String,
    path: String,
    hit: u64,
    ok: u64,
    statuses: BTreeMap<String, u64>,
    latest_request_id: Option<String>,
    latest_status: Option<u64>,
    latest_timestamp: Option<String>,
    latest_kind: Option<String>,
}

impl OperationCoverage {
    fn new(operation: &OperationSummary) -> Self {
        Self {
            operation_id: operation.operation_id.clone(),
            method: operation.method.to_string(),
            path: operation.path.clone(),
            hit: 0,
            ok: 0,
            statuses: BTreeMap::new(),
            latest_request_id: None,
            latest_status: None,
            latest_timestamp: None,
            latest_kind: None,
        }
    }

    fn record_hit(&mut self, record: &Value) {
        let status = record.get("status").and_then(Value::as_u64);
        self.hit += 1;
        if status.is_some_and(|status| (200..=299).contains(&status)) {
            self.ok += 1;
        }
        if let Some(status) = status {
            *self.statuses.entry(status.to_string()).or_insert(0) += 1;
        }
        self.latest_status = status;
        self.latest_request_id = record
            .get("request_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.latest_timestamp = record
            .get("timestamp")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        self.latest_kind = record
            .get("kind")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }

    fn success_2xx(&self) -> bool {
        self.ok > 0
    }

    fn side_effecting(&self) -> bool {
        method_side_effecting(&self.method)
    }

    fn to_value(&self) -> Value {
        json!({
            "operation_id": self.operation_id,
            "method": self.method,
            "path": self.path,
            "hit": self.hit > 0,
            "hits": self.hit,
            "success_2xx": self.success_2xx(),
            "ok": self.ok,
            "statuses": self.statuses,
            "side_effecting": self.side_effecting(),
            "latest_request_id": self.latest_request_id,
            "latest_status": self.latest_status,
            "latest_timestamp": self.latest_timestamp,
            "latest_kind": self.latest_kind,
        })
    }
}

fn api_coverage(
    spec: &Value,
    request_log_path: &Path,
    record_limit: usize,
    api_url: &str,
) -> Result<Value, ApiCommandError> {
    let operations = list_operations(spec, None, None)?;
    let records = crate::request_log::read_request_records_at(request_log_path, record_limit)
        .map_err(|source| {
            ApiCommandError::RequestLog {
                path: request_log_path.to_path_buf(),
                source,
            }
        })?;
    let mut coverage = operations
        .iter()
        .map(OperationCoverage::new)
        .collect::<Vec<_>>();
    let mut unmatched_records = Vec::new();

    for record in &records {
        let Some(index) = match_request_record_to_operation(&operations, record) else {
            unmatched_records.push(record.clone());
            continue;
        };
        coverage[index].record_hit(record);
    }

    let mut by_method: BTreeMap<String, BTreeMap<&'static str, u64>> = BTreeMap::new();
    for entry in &coverage {
        let method = by_method.entry(entry.method.clone()).or_default();
        *method.entry("total").or_insert(0) += 1;
        if entry.hit > 0 {
            *method.entry("hit").or_insert(0) += 1;
        }
        if entry.success_2xx() {
            *method.entry("ok").or_insert(0) += 1;
        }
    }

    let no_hit = coverage
        .iter()
        .filter(|entry| entry.hit == 0)
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();
    let no_2xx = coverage
        .iter()
        .filter(|entry| entry.hit > 0 && !entry.success_2xx())
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();
    let write_no_2xx = coverage
        .iter()
        .filter(|entry| entry.side_effecting() && entry.hit > 0 && !entry.success_2xx())
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();
    let operations_value = coverage
        .iter()
        .map(OperationCoverage::to_value)
        .collect::<Vec<_>>();

    Ok(json!({
        "generated_at": chrono::Utc::now().to_rfc3339(),
        "api_url": api_url,
        "request_log": request_log_path,
        "records_considered": records.len(),
        "record_limit": record_limit,
        "total_operations": operations.len(),
        "by_method": by_method,
        "no_hit_count": no_hit.len(),
        "no_2xx_count": no_2xx.len(),
        "write_no_2xx_count": write_no_2xx.len(),
        "unmatched_record_count": unmatched_records.len(),
        "no_hit": no_hit,
        "no_2xx": no_2xx,
        "write_no_2xx": write_no_2xx,
        "unmatched_records": unmatched_records,
        "operations": operations_value,
    }))
}

fn write_api_coverage_markdown(path: &PathBuf, coverage: &Value) -> Result<(), ApiCommandError> {
    let markdown = api_coverage_markdown(coverage);
    fs::write(path, markdown).map_err(|source| {
        ApiCommandError::OutputFile {
            path: path.clone(),
            source,
        }
    })
}

fn api_coverage_markdown(coverage: &Value) -> String {
    let mut body = String::new();
    writeln!(body, "# PCL API Coverage").expect("writing to String cannot fail");
    writeln!(body).expect("writing to String cannot fail");
    writeln!(
        body,
        "- Generated: {}",
        coverage
            .get("generated_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- API URL: {}",
        coverage
            .get("api_url")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Request log: {}",
        coverage
            .get("request_log")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Operations: {}",
        coverage
            .get("total_operations")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Records considered: {}",
        coverage
            .get("records_considered")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- No-hit operations: {}",
        coverage
            .get("no_hit_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(
        body,
        "- Hit but no 2xx operations: {}",
        coverage
            .get("no_2xx_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )
    .expect("writing to String cannot fail");
    writeln!(body).expect("writing to String cannot fail");

    append_coverage_table(&mut body, "No-Hit Operations", coverage.get("no_hit"));
    append_coverage_table(
        &mut body,
        "Hit But No 2xx Operations",
        coverage.get("no_2xx"),
    );
    append_coverage_table(
        &mut body,
        "Side-Effecting Operations Hit But No 2xx",
        coverage.get("write_no_2xx"),
    );
    body
}

fn append_coverage_table(body: &mut String, title: &str, value: Option<&Value>) {
    writeln!(body, "## {title}").expect("writing to String cannot fail");
    writeln!(body).expect("writing to String cannot fail");
    let Some(entries) = value.and_then(Value::as_array) else {
        writeln!(body, "None.").expect("writing to String cannot fail");
        writeln!(body).expect("writing to String cannot fail");
        return;
    };
    if entries.is_empty() {
        writeln!(body, "None.").expect("writing to String cannot fail");
        writeln!(body).expect("writing to String cannot fail");
        return;
    }
    writeln!(
        body,
        "| Operation | Method | Path | Hits | Statuses | Latest Request |"
    )
    .expect("writing to String cannot fail");
    writeln!(body, "| --- | --- | --- | ---: | --- | --- |")
        .expect("writing to String cannot fail");
    for entry in entries {
        writeln!(
            body,
            "| `{}` | `{}` | `{}` | {} | `{}` | `{}` |",
            entry
                .get("operation_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            entry
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            entry.get("hits").and_then(Value::as_u64).unwrap_or(0),
            entry
                .get("statuses")
                .map_or_else(|| "{}".to_string(), Value::to_string),
            entry
                .get("latest_request_id")
                .and_then(Value::as_str)
                .unwrap_or("")
        )
        .expect("writing to String cannot fail");
    }
    writeln!(body).expect("writing to String cannot fail");
}

fn match_request_record_to_operation(
    operations: &[OperationSummary],
    record: &Value,
) -> Option<usize> {
    if let Some(operation_id) = record.get("operation_id").and_then(Value::as_str)
        && let Some(index) = operations
            .iter()
            .position(|operation| operation.operation_id == operation_id)
    {
        return Some(index);
    }

    let method = record.get("method").and_then(Value::as_str)?;
    let path = record.get("path").and_then(Value::as_str)?;
    let path = path.split_once('?').map_or(path, |(path, _)| path);

    operations.iter().position(|operation| {
        operation.method.eq_ignore_ascii_case(method) && openapi_path_matches(&operation.path, path)
    })
}

fn openapi_path_matches(openapi_path: &str, observed_path: &str) -> bool {
    if openapi_path == observed_path {
        return true;
    }
    let openapi_segments = openapi_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let observed_segments = observed_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if openapi_segments.len() != observed_segments.len() {
        return false;
    }
    openapi_segments
        .iter()
        .zip(observed_segments)
        .all(|(expected, observed)| {
            (expected.starts_with('{') && expected.ends_with('}')) || *expected == observed
        })
}

fn list_operations(
    spec: &Value,
    filter: Option<&str>,
    method_filter: Option<HttpMethod>,
) -> Result<Vec<OperationSummary>, ApiCommandError> {
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or(ApiCommandError::MissingPaths)?;
    let filter = filter.map(str::to_lowercase);
    let mut operations = Vec::new();

    for (path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };

        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            if method_filter.is_some_and(|wanted| wanted.openapi_key() != method.openapi_key()) {
                continue;
            }

            let Some(operation) = path_item.get(method.openapi_key()) else {
                continue;
            };

            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .map_or_else(|| synthetic_operation_id(method, path), ToString::to_string);
            let summary = operation
                .get("summary")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let tags = operation
                .get("tags")
                .and_then(Value::as_array)
                .map(|tags| {
                    tags.iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if let Some(filter) = &filter {
                let haystack = format!(
                    "{} {} {} {}",
                    operation_id,
                    path,
                    summary.as_deref().unwrap_or_default(),
                    tags.join(" ")
                )
                .to_lowercase();
                if !haystack.contains(filter) {
                    continue;
                }
            }

            let input_placeholders = operation_input_placeholders(path, operation);
            let requires_input = !input_placeholders.is_empty();
            let workflow_alternatives = workflow_alternatives(method, path);
            let raw_api_use =
                raw_api_use(method, path, operation, !workflow_alternatives.is_empty());
            operations.push(OperationSummary {
                inspect_command: format!("pcl api inspect {operation_id}"),
                call_command: example_call(method, path, operation),
                input_placeholders,
                requires_input,
                auth: operation_auth_metadata(method, path, operation),
                workflow_alternatives,
                raw_api_use,
                operation_id,
                method: method.as_str(),
                path: path.clone(),
                summary,
                tags,
            });
        }
    }

    operations.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.method.cmp(b.method))
            .then_with(|| a.operation_id.cmp(&b.operation_id))
    });

    Ok(operations)
}

fn inspect_operation(
    spec: &Value,
    operation: &str,
    path: Option<&str>,
    full: bool,
) -> Result<Value, ApiCommandError> {
    let paths = spec
        .get("paths")
        .and_then(Value::as_object)
        .ok_or(ApiCommandError::MissingPaths)?;

    let operation_method = match operation.to_lowercase().as_str() {
        "get" => Some(HttpMethod::Get),
        "post" => Some(HttpMethod::Post),
        "put" => Some(HttpMethod::Put),
        "patch" => Some(HttpMethod::Patch),
        "delete" => Some(HttpMethod::Delete),
        _ => None,
    };

    if let (Some(method), Some(path)) = (operation_method, path) {
        let operation = paths
            .get(path)
            .and_then(|path_item| path_item.get(method.openapi_key()))
            .ok_or_else(|| {
                ApiCommandError::OperationNotFound(format!("{} {}", method.as_str(), path))
            })?;
        let operation_id = operation
            .get("operationId")
            .and_then(Value::as_str)
            .map_or_else(|| synthetic_operation_id(method, path), ToString::to_string);
        return Ok(operation_manifest(
            operation_id,
            method,
            path,
            operation,
            full,
        ));
    }

    for (candidate_path, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };

        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
        ] {
            let Some(candidate) = path_item.get(method.openapi_key()) else {
                continue;
            };
            let candidate_id = candidate
                .get("operationId")
                .and_then(Value::as_str)
                .map_or_else(
                    || synthetic_operation_id(method, candidate_path),
                    ToString::to_string,
                );
            if candidate_id == operation {
                return Ok(operation_manifest(
                    candidate_id,
                    method,
                    candidate_path,
                    candidate,
                    full,
                ));
            }
        }
    }

    Err(ApiCommandError::OperationNotFound(operation.to_string()))
}

fn operation_manifest(
    operation_id: String,
    method: HttpMethod,
    path: &str,
    operation: &Value,
    full: bool,
) -> Value {
    let workflow_alternatives = workflow_alternatives(method, path);
    let raw_api_use = raw_api_use(method, path, operation, !workflow_alternatives.is_empty());
    let mut manifest = json!({
        "operation_id": operation_id,
        "method": method.as_str(),
        "path": path,
        "summary": operation.get("summary").and_then(Value::as_str),
        "description": operation.get("description").and_then(Value::as_str),
        "auth": operation_auth_metadata(method, path, operation),
        "workflow_alternatives": workflow_alternatives,
        "raw_api_use": raw_api_use,
        "parameters": operation_parameters(operation),
        "path_params": named_parameters(operation, "path", false),
        "required_query": named_parameters(operation, "query", true),
        "request_body": request_body_manifest(operation),
        "body_fields": body_fields(operation),
        "body_variants": body_variants(operation),
        "required_body_fields": required_body_fields(operation),
        "body_template": openapi_body_template(operation),
        "input_placeholders": operation_input_placeholders(path, operation),
        "response_statuses": response_statuses(operation),
        "example_call": example_call(method, path, operation),
    });

    if full && let Some(object) = manifest.as_object_mut() {
        object.insert("operation".to_string(), operation.clone());
    }

    manifest
}

fn workflow_alternatives(method: HttpMethod, path: &str) -> Vec<Value> {
    let mut alternatives = manifest_workflow_alternatives(method, path);
    alternatives.extend(special_workflow_alternatives(method, path));
    alternatives
}

fn manifest_workflow_alternatives(method: HttpMethod, path: &str) -> Vec<Value> {
    let Some(commands) = api_manifest()
        .get("commands")
        .and_then(Value::as_array)
        .cloned()
    else {
        return Vec::new();
    };

    let mut alternatives = Vec::new();
    for command in commands {
        let Some(command_text) = command.get("command").and_then(Value::as_str) else {
            continue;
        };
        if command_text.starts_with("pcl api ") {
            continue;
        }
        let workflow = command_text
            .split_whitespace()
            .nth(1)
            .unwrap_or(command_text)
            .to_string();
        let Some(actions) = command.get("actions").and_then(Value::as_array) else {
            continue;
        };

        for action in actions {
            if !manifest_action_matches_operation(action, method, path) {
                continue;
            }
            let action_name = action.get("name").and_then(Value::as_str);
            let example = workflow_example_for_operation(&workflow, action.get("example"), path);
            alternatives.push(json!({
                "workflow": workflow,
                "action": action_name,
                "command": command_text,
                "example": example,
                "required_flags": action.get("required_flags").cloned().unwrap_or(Value::Null),
                "body_template": action.get("body_template").cloned().unwrap_or(Value::Null),
            }));
        }
    }

    alternatives
}

fn workflow_example_for_operation(
    workflow: &str,
    example: Option<&Value>,
    operation_path: &str,
) -> Option<String> {
    let example = example.and_then(Value::as_str)?;
    if workflow == "integrations" {
        if operation_path.contains("/integrations/pagerduty") {
            return Some(example.replace("--provider slack", "--provider pagerduty"));
        }
        if operation_path.contains("/integrations/slack") {
            return Some(example.replace("--provider pagerduty", "--provider slack"));
        }
    }
    Some(example.to_string())
}

fn manifest_action_matches_operation(action: &Value, method: HttpMethod, path: &str) -> bool {
    action
        .get("method")
        .and_then(Value::as_str)
        .is_some_and(|action_method| action_method.eq_ignore_ascii_case(method.as_str()))
        && action
            .get("path")
            .and_then(Value::as_str)
            .is_some_and(|action_path| path_patterns_overlap(action_path, path))
}

fn path_patterns_overlap(left: &str, right: &str) -> bool {
    openapi_path_matches(left, right) || openapi_path_matches(right, left)
}

fn special_workflow_alternatives(method: HttpMethod, path: &str) -> Vec<Value> {
    let normalized_path = normalize_path_placeholders(path);
    match (method, normalized_path.as_str()) {
        (HttpMethod::Get, "/cli/auth/code") => {
            single_special_workflow(
                "auth",
                "login_challenge",
                "pcl auth login --no-wait --force --toon",
                "Device-login challenge is exposed as a structured auth command.",
            )
        }
        (HttpMethod::Get, "/cli/auth/status") => {
            single_special_workflow(
                "auth",
                "poll",
                "pcl auth poll --session-id <session-id> --device-secret <secret> --expires-at <rfc3339> --toon",
                "Polling is handled by the auth command returned in data.poll_command.",
            )
        }
        (HttpMethod::Post, "/cli/auth/verify") => {
            single_special_workflow(
                "auth",
                "verify",
                "pcl auth login --force --toon",
                "The login command owns verification and stores the resulting credentials.",
            )
        }
        (HttpMethod::Post, "/auth/refresh") => {
            single_special_workflow(
                "auth",
                "refresh",
                "pcl auth refresh --toon",
                "Refresh rotation is exposed as a structured auth command.",
            )
        }
        (HttpMethod::Get, "/openapi") => {
            single_special_workflow(
                "api",
                "manifest",
                "pcl api manifest --toon",
                "Use the CLI manifest/list/inspect surfaces for discovery instead of raw OpenAPI retrieval.",
            )
        }
        (HttpMethod::Get, "/projects") => {
            single_special_workflow(
                "projects",
                "explorer",
                "pcl projects --limit 10",
                "Project exploration uses the normalized project view endpoint.",
            )
        }
        (HttpMethod::Get, "/public/incidents") => {
            single_special_workflow(
                "incidents",
                "list_public",
                "pcl incidents --limit 5",
                "Public incident listing uses the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/projects/{}/incidents") => {
            single_special_workflow(
                "incidents",
                "list_project",
                "pcl incidents --project <project-ref> --limit 50",
                "Project incident listing uses the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/incidents/{}") => {
            single_special_workflow(
                "incidents",
                "detail",
                "pcl incidents --incident-id <incident-id>",
                "Incident detail uses the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/incidents/{}/transactions/{}/trace") => {
            single_special_workflow(
                "incidents",
                "trace",
                "pcl incidents --incident-id <incident-id> --tx-id <tx-id>",
                "Incident traces use the normalized incident view endpoint.",
            )
        }
        (HttpMethod::Get, "/projects/{}/submitted-assertions") => {
            vec![
                special_workflow(
                    "releases",
                    "list",
                    "pcl releases --project <project-ref>",
                    "Submitted assertions were superseded by release and registered-assertion workflows.",
                ),
                special_workflow(
                    "assertions",
                    "registered",
                    "pcl assertions --project <project-ref> --registered",
                    "Submitted assertions were superseded by release and registered-assertion workflows.",
                ),
            ]
        }
        (HttpMethod::Post, "/projects/{}/submitted-assertions") => {
            single_special_workflow(
                "releases",
                "create",
                "pcl apply --json",
                "Submitting assertions is now represented by creating a release through pcl apply or pcl releases.",
            )
        }
        _ => Vec::new(),
    }
}

fn single_special_workflow(workflow: &str, action: &str, example: &str, note: &str) -> Vec<Value> {
    vec![special_workflow(workflow, action, example, note)]
}

fn special_workflow(workflow: &str, action: &str, example: &str, note: &str) -> Value {
    json!({
        "workflow": workflow,
        "action": action,
        "example": example,
        "note": note,
    })
}

fn normalize_path_placeholders(path: &str) -> String {
    path.split('/')
        .map(|segment| {
            if segment.starts_with('{') && segment.ends_with('}') {
                "{}"
            } else {
                segment
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn raw_api_use(
    method: HttpMethod,
    path: &str,
    operation: &Value,
    has_workflow_alternative: bool,
) -> Value {
    if has_workflow_alternative {
        return json!({
            "policy": "prefer_workflow",
            "reason": "A first-class CLI workflow exists. Use raw api call only for debugging, OpenAPI parity checks, or reproducing low-level API behavior.",
        });
    }
    if service_api_key_raw_call_path(method, path) {
        return json!({
            "policy": "internal_service",
            "reason": "Service callback endpoint; normal CLI users and agents should not call it directly.",
        });
    }
    if requires_browser_session_token(path, operation) {
        return json!({
            "policy": "browser_session_bridge",
            "reason": "Browser/Privy session bridge; use auth/account commands for CLI authentication state.",
        });
    }

    json!({
        "policy": "debug_escape_hatch",
        "reason": "No first-class workflow is advertised; inspect first and preserve request IDs when using raw calls.",
    })
}

fn operation_parameters(operation: &Value) -> Vec<Value> {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .map(|parameters| {
            parameters
                .iter()
                .map(|parameter| {
                    json!({
                        "name": parameter.get("name").and_then(Value::as_str),
                        "in": parameter.get("in").and_then(Value::as_str),
                        "required": parameter.get("required").and_then(Value::as_bool).unwrap_or(false),
                        "schema": parameter.get("schema").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn named_parameters(operation: &Value, location: &str, required_only: bool) -> Vec<String> {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .map(|parameters| {
            parameters
                .iter()
                .filter(|parameter| parameter.get("in").and_then(Value::as_str) == Some(location))
                .filter(|parameter| {
                    !required_only
                        || parameter
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn request_body_manifest(operation: &Value) -> Value {
    let Some(body) = operation.get("requestBody") else {
        return Value::Null;
    };
    json!({
        "required": body.get("required").and_then(Value::as_bool).unwrap_or(false),
        "content_types": body
            .get("content")
            .and_then(Value::as_object)
            .map(|content| content.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default(),
        "schema_type": body
            .pointer("/content/application~1json/schema")
            .map_or_else(|| "unknown".to_string(), compact_schema_type),
    })
}

fn body_schema(operation: &Value) -> Option<&Value> {
    operation.pointer("/requestBody/content/application~1json/schema")
}

fn required_body_fields(operation: &Value) -> Vec<String> {
    body_schema(operation)
        .map(required_fields_for_schema)
        .unwrap_or_default()
}

fn required_fields_for_schema(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn body_fields(operation: &Value) -> Vec<Value> {
    body_schema(operation)
        .map(body_fields_for_schema)
        .unwrap_or_default()
}

fn body_fields_for_schema(schema: &Value) -> Vec<Value> {
    let required = required_fields_for_schema(schema);
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| {
                    json!({
                        "name": name,
                        "required": required.iter().any(|required| required == name),
                        "type": compact_schema_type(schema),
                        "enum": schema.get("enum").cloned().unwrap_or(Value::Null),
                        "const": schema.get("const").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn body_variants(operation: &Value) -> Vec<Value> {
    let Some(schema) = body_schema(operation) else {
        return Vec::new();
    };
    let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    variants
        .iter()
        .enumerate()
        .map(|(index, variant)| {
            json!({
                "name": schema_variant_name(variant, index),
                "schema_type": compact_schema_type(variant),
                "required_body_fields": required_fields_for_schema(variant),
                "body_fields": body_fields_for_schema(variant),
                "body_template": template_from_schema(variant),
            })
        })
        .collect()
}

fn schema_variant_name(schema: &Value, index: usize) -> String {
    schema
        .pointer("/properties/mode/const")
        .or_else(|| schema.pointer("/properties/mode/enum/0"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("variant_{}", index + 1), ToString::to_string)
}

fn compact_schema_type(schema: &Value) -> String {
    if let Some(schema_type) = schema.get("type").and_then(Value::as_str) {
        return schema_type.to_string();
    }
    if schema.get("oneOf").is_some() {
        return "oneOf".to_string();
    }
    if schema.get("anyOf").is_some() {
        return "anyOf".to_string();
    }
    "unknown".to_string()
}

fn openapi_body_template(operation: &Value) -> Value {
    let Some(schema) = body_schema(operation) else {
        return Value::Null;
    };
    template_from_schema(schema)
}

fn template_from_schema(schema: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut object = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (name, property) in properties {
                    object.insert(name.clone(), template_from_schema(property));
                }
            }
            Value::Object(object)
        }
        Some("array") => {
            Value::Array(vec![
                schema
                    .get("items")
                    .map_or(Value::String("<item>".to_string()), template_from_schema),
            ])
        }
        Some("integer") | Some("number") => json!(0),
        Some("boolean") => json!(false),
        Some("string") => {
            if let Some(value) = schema.get("const") {
                return value.clone();
            }
            schema
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_else(|| Value::String("<string>".to_string()))
        }
        _ => {
            if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
                return options
                    .first()
                    .map_or(Value::String("<value>".to_string()), template_from_schema);
            }
            Value::String("<value>".to_string())
        }
    }
}

fn response_statuses(operation: &Value) -> Vec<Value> {
    operation
        .get("responses")
        .and_then(Value::as_object)
        .map(|responses| {
            responses
                .iter()
                .map(|(status, response)| {
                    json!({
                        "status": status,
                        "description": response.get("description").and_then(Value::as_str),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn example_call(method: HttpMethod, path: &str, operation: &Value) -> String {
    let path = example_path(path, operation);
    let mut command = format!(
        "pcl api call {} {}",
        method.openapi_key(),
        shell_quote_path(&path)
    );
    if should_allow_unauthenticated_raw_call(method, &path, operation) {
        command.push_str(" --allow-unauthenticated");
    }
    if service_api_key_raw_call_path(method, &path) {
        command.push_str(" --header 'x-api-key=<x-api-key>'");
    }
    for parameter in required_header_parameters(operation) {
        if service_api_key_raw_call_path(method, &path)
            && parameter.eq_ignore_ascii_case("x-api-key")
        {
            continue;
        }
        write!(
            command,
            " --header {}",
            shell_quote(&format!(
                "{parameter}={}",
                header_placeholder(&parameter, &path, operation)
            ))
        )
        .expect("writing to String cannot fail");
    }
    for parameter in required_query_parameters(operation) {
        write!(
            command,
            " --query {}",
            shell_quote(&format!("{parameter}=<{parameter}>"))
        )
        .expect("writing to String cannot fail");
    }
    if operation.get("requestBody").is_some() {
        let body = openapi_body_template(operation);
        if body.is_null() {
            command.push_str(" --body '{}'");
        } else {
            let body = serde_json::to_string(&body).unwrap_or_else(|_| "{...}".to_string());
            write!(command, " --body {}", shell_quote(&body))
                .expect("writing to String cannot fail");
        }
    }
    command
}

fn operation_auth_metadata(method: HttpMethod, path: &str, operation: &Value) -> Value {
    let required_headers = required_header_parameters(operation);
    let browser_token_required = requires_browser_session_token(path, operation);
    let service_api_key_required = service_api_key_raw_call_path(method, path);
    let stored_cli_auth =
        !should_allow_unauthenticated_raw_call(method, path, operation) && !browser_token_required;
    let mut notes = Vec::new();
    if browser_token_required {
        notes.push(
            "Requires a browser/Privy session bearer token supplied with --header authorization=Bearer <privy-token>.",
        );
    }
    if stored_cli_auth {
        notes.push(
            "PCL attaches the stored CLI bearer token unless --allow-unauthenticated is set.",
        );
    }
    if service_api_key_required {
        notes.push("Requires a service API key supplied with --header x-api-key=<x-api-key>.");
    }
    json!({
        "stored_cli_auth": stored_cli_auth,
        "allow_unauthenticated_example": should_allow_unauthenticated_raw_call(method, path, operation),
        "browser_session_token_required": browser_token_required,
        "service_api_key_required": service_api_key_required,
        "required_headers": required_headers,
        "notes": notes,
    })
}

fn requires_browser_session_token(path: &str, operation: &Value) -> bool {
    path == "/web/auth/bootstrap-session" && has_required_authorization_parameter(operation)
}

fn header_placeholder(parameter: &str, path: &str, operation: &Value) -> String {
    if parameter.eq_ignore_ascii_case("authorization") {
        if requires_browser_session_token(path, operation) {
            return "Bearer <privy-token>".to_string();
        }
        return "Bearer <token>".to_string();
    }
    format!("<{parameter}>")
}

fn should_allow_unauthenticated_raw_call(
    method: HttpMethod,
    path: &str,
    operation: &Value,
) -> bool {
    service_api_key_raw_call_path(method, path)
        || (public_raw_call_path(method, path) && !has_required_authorization_parameter(operation))
}

fn service_api_key_raw_call_path(method: HttpMethod, path: &str) -> bool {
    method == HttpMethod::Post
        && (path.starts_with("/enforcer/")
            || path.starts_with("/indexer/")
            || path.starts_with("/tracer/")
            || path.starts_with("/backtesting/"))
}

fn public_raw_call_path(method: HttpMethod, path: &str) -> bool {
    match method {
        HttpMethod::Get => {
            path == "/health"
                || path == "/cli/auth/code"
                || path == "/cli/auth/status"
                || path == "/openapi"
                || path == "/projects"
                || path == "/public/incidents"
                || path == "/stats"
                || path == "/system-status"
                || path == "/search"
                || path == "/assertions"
                || path == "/views/projects"
                || path.starts_with("/views/public/")
                || path.starts_with("/projects/resolve/")
                || path.starts_with("/web/verified-contract")
                || (path.starts_with("/invitations/") && path.ends_with("/preview"))
        }
        HttpMethod::Post => path == "/auth/refresh" || path == "/cli/auth/verify",
        HttpMethod::Put | HttpMethod::Patch | HttpMethod::Delete => false,
    }
}

fn has_required_authorization_parameter(operation: &Value) -> bool {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .is_some_and(|parameters| {
            parameters.iter().any(|parameter| {
                parameter.get("in").and_then(Value::as_str) == Some("header")
                    && parameter
                        .get("required")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    && parameter
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case("authorization"))
            })
        })
}

fn example_path(path: &str, operation: &Value) -> String {
    let mut path = path.to_string();
    for parameter in named_parameters(operation, "path", false) {
        path = path.replace(&format!("{{{parameter}}}"), &format!("<{parameter}>"));
    }
    path
}

fn shell_quote_path(path: &str) -> String {
    if path.contains('<') || path.contains('>') {
        shell_quote(path)
    } else {
        path.to_string()
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn operation_input_placeholders(path: &str, operation: &Value) -> Vec<String> {
    let mut placeholders = named_parameters(operation, "path", false)
        .into_iter()
        .map(|parameter| format!("path:{parameter}"))
        .collect::<Vec<_>>();
    placeholders.extend(
        required_header_parameters(operation)
            .into_iter()
            .map(|parameter| format!("header:{parameter}")),
    );
    placeholders.extend(
        required_query_parameters(operation)
            .into_iter()
            .map(|parameter| format!("query:{parameter}")),
    );
    if operation.get("requestBody").is_some() {
        placeholders.push("body".to_string());
    }
    if placeholders.is_empty() && path.contains('{') {
        placeholders.push("path".to_string());
    }
    placeholders
}

fn required_header_parameters(operation: &Value) -> Vec<String> {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .map(|parameters| {
            parameters
                .iter()
                .filter(|parameter| {
                    parameter.get("in").and_then(Value::as_str) == Some("header")
                        && parameter
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn required_query_parameters(operation: &Value) -> Vec<String> {
    operation
        .get("parameters")
        .and_then(Value::as_array)
        .map(|parameters| {
            parameters
                .iter()
                .filter(|parameter| {
                    parameter.get("in").and_then(Value::as_str) == Some("query")
                        && parameter
                            .get("required")
                            .and_then(Value::as_bool)
                            .unwrap_or(false)
                })
                .filter_map(|parameter| parameter.get("name").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn next_actions_for_operations(operations: &[OperationSummary]) -> Vec<String> {
    operations.first().map_or_else(
        || vec!["pcl api list".to_string(), "pcl api manifest".to_string()],
        |operation| {
            if let Some(example) = operation
                .workflow_alternatives
                .first()
                .and_then(|alternative| alternative.get("example"))
                .and_then(Value::as_str)
            {
                return vec![
                    example.to_string(),
                    format!("{} --toon", operation.inspect_command),
                ];
            }
            if operation.requires_input {
                vec![
                    format!("{} --toon", operation.inspect_command),
                    "Inspect the operation, then fill the placeholders in the example call"
                        .to_string(),
                ]
            } else {
                vec![
                    operation.inspect_command.clone(),
                    operation.call_command.clone(),
                ]
            }
        },
    )
}

fn command_next_actions(inspected: &Value) -> Vec<String> {
    if let Some(example) = inspected
        .get("workflow_alternatives")
        .and_then(Value::as_array)
        .and_then(|alternatives| alternatives.first())
        .and_then(|alternative| alternative.get("example"))
        .and_then(Value::as_str)
    {
        return vec![example.to_string()];
    }
    inspected
        .get("example_call")
        .and_then(Value::as_str)
        .map_or_else(
            || vec!["pcl api list".to_string()],
            |command| vec![command.to_string()],
        )
}

fn synthetic_operation_id(method: HttpMethod, path: &str) -> String {
    let mut id = method.openapi_key().to_string();
    let mut previous_was_separator = false;

    for ch in path.chars() {
        if ch.is_ascii_alphanumeric() {
            if previous_was_separator && !id.ends_with('_') {
                id.push('_');
            }
            id.push(ch.to_ascii_lowercase());
            previous_was_separator = false;
        } else {
            previous_was_separator = true;
        }
    }

    id.trim_end_matches('_').to_string()
}

#[cfg(test)]
mod tests;
