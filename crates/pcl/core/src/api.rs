use crate::{
    DEFAULT_PLATFORM_URL,
    config::CliConfig,
};
use clap::{
    ArgGroup,
    ValueEnum,
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
    fs,
    io::Read,
    path::PathBuf,
    str::FromStr,
};

#[derive(Debug, thiserror::Error)]
pub enum ApiCommandError {
    #[error("Run `pcl auth login` first, or pass `--allow-unauthenticated`")]
    NoAuthToken,

    #[error(
        "Stored auth token expired at {0}. Run `pcl auth login` again, or pass `--allow-unauthenticated` for public endpoints."
    )]
    ExpiredAuthToken(chrono::DateTime<chrono::Utc>),

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

    #[error("Failed to read request body from stdin: {0}")]
    Stdin(std::io::Error),

    #[error("Invalid JSON body: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("OpenAPI spec does not contain a paths object")]
    MissingPaths,

    #[error("No API operation matched `{0}`")]
    OperationNotFound(String),

    #[error("{message}")]
    InvalidWorkflow { message: String },
}

impl ApiCommandError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAuthToken => "auth.no_token",
            Self::ExpiredAuthToken(_) => "auth.expired_token",
            Self::InvalidKeyValue { .. } => "input.invalid_key_value",
            Self::InvalidHeaderName { .. } => "input.invalid_header_name",
            Self::InvalidHeaderValue { .. } => "input.invalid_header_value",
            Self::InvalidPath(_) => "input.invalid_path",
            Self::Url(_) => "input.invalid_url",
            Self::BodyFile { .. } => "input.body_file_read_failed",
            Self::Stdin(_) => "input.stdin_read_failed",
            Self::Json(_) => "input.invalid_json",
            Self::Request(source) => {
                match source.status().map(|status| status.as_u16()) {
                    Some(401) => "auth.unauthorized",
                    Some(403) => "auth.forbidden",
                    _ => "network.request_failed",
                }
            }
            Self::MissingPaths => "openapi.missing_paths",
            Self::OperationNotFound(_) => "openapi.operation_not_found",
            Self::InvalidWorkflow { .. } => "workflow.invalid_arguments",
        }
    }

    pub fn recoverable(&self) -> bool {
        !matches!(self, Self::MissingPaths)
    }

    pub fn next_actions(&self) -> Vec<String> {
        match self {
            Self::NoAuthToken | Self::ExpiredAuthToken(_) => {
                vec![
                    "pcl auth login".to_string(),
                    "pcl api list --allow-unauthenticated --json".to_string(),
                ]
            }
            Self::InvalidPath(_) => {
                vec![
                    "pcl api list --json".to_string(),
                    "pcl api call get /views/public/incidents --allow-unauthenticated --json"
                        .to_string(),
                ]
            }
            Self::InvalidKeyValue { kind, .. } => {
                vec![format!(
                    "Use --{kind} key=value, for example: pcl api call get /views/public/incidents --{kind} limit=5 --json"
                )]
            }
            Self::InvalidHeaderName { .. } | Self::InvalidHeaderValue { .. } => {
                vec![
                    "Use --header name=value, for example: --header x-cl-dev-mode=true".to_string(),
                ]
            }
            Self::Json(_) => {
                vec![
                    "Pass valid JSON with --body '{\"key\":\"value\"}'".to_string(),
                    "Use --body-file request.json for larger payloads".to_string(),
                ]
            }
            Self::OperationNotFound(_) => {
                vec![
                    "pcl api list --json".to_string(),
                    "pcl api inspect get /views/public/incidents --json".to_string(),
                ]
            }
            Self::InvalidWorkflow { .. } => {
                vec![
                    "pcl api manifest".to_string(),
                    "pcl api incidents --limit 5".to_string(),
                    "pcl api assertions --project-id <project-id>".to_string(),
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
            Self::Request(_) | Self::Url(_) => vec!["Check --api-url and retry".to_string()],
            Self::BodyFile { .. } => {
                vec!["Check --body-file path or pass --body directly".to_string()]
            }
            Self::Stdin(_) => vec!["Pipe a JSON body into --body-file -".to_string()],
            Self::MissingPaths => {
                vec!["Check that /api/v1/openapi returns an OpenAPI document".to_string()]
            }
        }
    }

    pub fn json_envelope(&self) -> Value {
        json!({
            "status": "error",
            "error": {
                "code": self.code(),
                "message": self.to_string(),
                "recoverable": self.recoverable(),
            },
            "next_actions": self.next_actions(),
        })
    }
}

#[derive(clap::Parser, Debug)]
#[command(
    about = "Discover and call the platform API",
    long_about = "Discover and call the Credible Layer platform API. Commands return structured JSON with next actions so agents can inspect the API surface and execute any endpoint."
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
}

#[derive(clap::Subcommand, Debug)]
enum ApiCommand {
    #[command(
        about = "List or inspect incidents",
        after_help = "Examples:\n  pcl api incidents --limit 5\n  pcl api incidents --project-id <project-id> --environment production\n  pcl api incidents --incident-id <incident-id>\n  pcl api incidents --incident-id <incident-id> --tx-id <tx-id>\n  pcl api incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace"
    )]
    Incidents(IncidentsArgs),

    #[command(
        about = "List, inspect, create, update, save, or delete projects",
        after_help = "Examples:\n  pcl api projects\n  pcl api projects --project-id <project-ref>\n  pcl api projects --create --project-name demo --chain-id 1\n  pcl api projects --project-id <project-ref> --update --field github_url=https://github.com/org/repo\n  pcl api projects --project-id <project-ref> --save"
    )]
    Projects(ProjectsArgs),

    #[command(
        about = "List, inspect, submit, and manage project assertions",
        after_help = "Examples:\n  pcl api assertions --project-id <project-ref>\n  pcl api assertions --project-id <project-ref> --submitted\n  pcl api assertions --project-id <project-ref> --submit --body-file submitted-assertions.json\n  pcl api assertions --project-id <project-ref> --remove-info"
    )]
    Assertions(AssertionsArgs),

    #[command(
        about = "Search and inspect platform-wide metadata",
        after_help = "Examples:\n  pcl api search --query settler\n  pcl api search --stats\n  pcl api search --system-status\n  pcl api search --verified-contract --address 0x... --chain-id 1"
    )]
    Search(SearchArgs),

    #[command(
        about = "Inspect and manage current account onboarding state",
        after_help = "Examples:\n  pcl api account\n  pcl api account --accept-terms\n  pcl api account --logout"
    )]
    Account(AccountArgs),

    #[command(
        about = "List or manage project contracts and assertion adopters",
        after_help = "Examples:\n  pcl api contracts --project <project-ref>\n  pcl api contracts --project <project-ref> --adopter-id <adopter-id>\n  pcl api contracts --unassigned\n  pcl api contracts --create --body '{...}'"
    )]
    Contracts(ContractsArgs),

    #[command(
        about = "List, inspect, create, preview, deploy, or remove releases",
        after_help = "Examples:\n  pcl api releases --project <project-ref>\n  pcl api releases --project <project-ref> --release-id <release-id>\n  pcl api releases --project <project-ref> --preview --body-file release.json\n  pcl api releases --project <project-ref> --release-id <release-id> --deploy-calldata"
    )]
    Releases(ReleasesArgs),

    #[command(
        about = "Inspect deployments and confirm deployed assertions",
        after_help = "Examples:\n  pcl api deployments --project <project-ref>\n  pcl api deployments --project <project-ref> --confirm --body '{...}'"
    )]
    Deployments(DeploymentsArgs),

    #[command(
        about = "Manage members, roles, and invitations",
        after_help = "Examples:\n  pcl api access --project <project-ref> --members\n  pcl api access --project <project-ref> --invite --body '{...}'\n  pcl api access --pending\n  pcl api access --token <token> --preview"
    )]
    Access(AccessArgs),

    #[command(
        about = "Manage Slack and PagerDuty integrations",
        after_help = "Examples:\n  pcl api integrations --project <project-ref> --provider slack\n  pcl api integrations --project <project-ref> --provider pagerduty --configure --body '{...}'\n  pcl api integrations --project <project-ref> --provider slack --test"
    )]
    Integrations(IntegrationsArgs),

    #[command(
        about = "Manage project protocol manager settings",
        after_help = "Examples:\n  pcl api protocol-manager --project <project-ref> --nonce\n  pcl api protocol-manager --project <project-ref> --transfer-calldata\n  pcl api protocol-manager --project <project-ref> --set --body '{...}'"
    )]
    ProtocolManager(ProtocolManagerArgs),

    #[command(
        about = "Inspect or reject protocol manager transfers",
        after_help = "Examples:\n  pcl api transfers --pending\n  pcl api transfers --transfer-id <transfer-id>\n  pcl api transfers --reject --body '{...}'"
    )]
    Transfers(TransfersArgs),

    #[command(
        about = "Inspect project events and audit logs",
        after_help = "Examples:\n  pcl api events --project <project-ref>\n  pcl api events --project <project-ref> --audit-log"
    )]
    Events(EventsArgs),

    #[command(
        about = "Print an agent-readable command manifest",
        after_help = "Examples:\n  pcl api manifest\n  pcl api manifest --json"
    )]
    Manifest,

    #[command(
        about = "List OpenAPI operations",
        after_help = "Examples:\n  pcl api list --json\n  pcl api list --filter incidents --json\n  pcl api list --method get --json"
    )]
    List {
        #[arg(long, help = "Filter operation id, summary, tags, or path")]
        filter: Option<String>,
        #[arg(long, value_enum, help = "Filter by HTTP method")]
        method: Option<HttpMethod>,
    },

    #[command(
        about = "Inspect one OpenAPI operation",
        after_help = "Examples:\n  pcl api inspect get_views_projects_project_id_incidents --json\n  pcl api inspect get /views/public/incidents --json"
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
        about = "Call any platform API endpoint",
        after_help = "Examples:\n  pcl api call get /views/public/incidents --query limit=5 --json\n  pcl api call get /views/projects/<uuid>/incidents --query environment=production --json\n  pcl api call post /web/auth/logout --body '{}' --json"
    )]
    Call {
        #[arg(value_enum, help = "HTTP method")]
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
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
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

#[derive(Debug, Serialize)]
struct OperationSummary {
    operation_id: String,
    method: &'static str,
    path: String,
    summary: Option<String>,
    tags: Vec<String>,
    inspect_command: String,
    call_command: String,
}

struct ApiRequestInput<'a> {
    method: HttpMethod,
    path: &'a str,
    query: &'a [String],
    header: &'a [String],
    body: Option<&'a str>,
    body_file: &'a Option<PathBuf>,
    require_auth: bool,
}

#[derive(Debug)]
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
        alias = "project",
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
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("project_action")
        .args(["home", "saved", "create", "update", "delete", "save", "unsave", "resolve", "widget"])
        .multiple(false)
))]
struct ProjectsArgs {
    #[arg(
        long,
        alias = "project",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project_id: Option<String>,
    #[arg(long, help = "Return authenticated projects home view")]
    home: bool,
    #[arg(long, help = "Return saved projects")]
    saved: bool,
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
        alias = "project",
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
    #[arg(long, help = "Return submitted assertions for --project-id")]
    submitted: bool,
    #[arg(long, help = "Return registered assertions for --project-id")]
    registered: bool,
    #[arg(long, help = "Submit assertions to --project-id")]
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
        alias = "project-id",
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
        .args(["create", "preview", "deploy", "remove", "deploy_calldata", "remove_calldata"])
        .multiple(false)
))]
struct ReleasesArgs {
    #[arg(
        long,
        alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: String,
    #[arg(long, alias = "release_id", help = "Release ID")]
    release_id: Option<String>,
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
        alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: String,
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
        alias = "project-id",
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
        alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: String,
    #[arg(long, value_enum, help = "Integration provider")]
    provider: IntegrationProvider,
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
        alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: String,
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
        alias = "project-id",
        alias = "project_id",
        help = "Project UUID or slug"
    )]
    project: String,
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

impl ApiArgs {
    pub async fn run(&self, config: &CliConfig, json_output: bool) -> Result<(), ApiCommandError> {
        match &self.command {
            ApiCommand::Incidents(args) => {
                let output = self.run_incidents(config, args).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Projects(args) => {
                let output = self.run_projects(config, args).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Assertions(args) => {
                let output = self.run_assertions(config, args).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Search(args) => {
                let output = self.run_workflow(config, search_request(args)?).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Account(args) => {
                if args.body_template {
                    let output = template_envelope(account_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self.run_workflow(config, account_request(args)?).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Contracts(args) => {
                if args.body_template {
                    let output = template_envelope(contracts_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self.run_workflow(config, contracts_request(args)?).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Releases(args) => {
                if args.body_template {
                    let output = template_envelope(release_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self.run_workflow(config, releases_request(args)?).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Deployments(args) => {
                if args.body_template {
                    let output = template_envelope(deployment_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_workflow(config, deployments_request(args)?)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Access(args) => {
                if args.body_template {
                    let output = template_envelope(access_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self.run_workflow(config, access_request(args)?).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Integrations(args) => {
                if args.body_template {
                    let output = template_envelope(integration_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self
                    .run_workflow(config, integrations_request(args)?)
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
                    .run_workflow(config, protocol_manager_request(args)?)
                    .await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Transfers(args) => {
                if args.body_template {
                    let output = template_envelope(transfer_body_template(args));
                    print_output(&output, json_output)?;
                    return Ok(());
                }
                let output = self.run_workflow(config, transfers_request(args)?).await?;
                print_output(&output, json_output)?;
            }
            ApiCommand::Events(args) => {
                let output = self.run_workflow(config, events_request(args)).await?;
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
            ApiCommand::Call {
                method,
                path,
                query,
                header,
                body,
                body_file,
            } => {
                let response = self
                    .call_api(
                        config,
                        ApiRequestInput {
                            method: *method,
                            path,
                            query,
                            header,
                            body: body.as_deref(),
                            body_file,
                            require_auth: !self.allow_unauthenticated,
                        },
                    )
                    .await?;
                let output = json!({
                    "status": "ok",
                    "data": response,
                    "next_actions": [
                        "pcl api list --json",
                        "pcl api manifest --json"
                    ],
                });
                print_output(&output, json_output)?;
            }
        }

        Ok(())
    }

    async fn run_incidents(
        &self,
        config: &CliConfig,
        args: &IncidentsArgs,
    ) -> Result<Value, ApiCommandError> {
        let request = incidents_request(args)?;
        let data = self.call_workflow(config, &request).await?;
        let next_actions = incidents_next_actions(&data, args, request.next_actions);
        Ok(json!({
            "status": "ok",
            "data": data,
            "next_actions": next_actions,
        }))
    }

    async fn run_projects(
        &self,
        config: &CliConfig,
        args: &ProjectsArgs,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(project_body_template(args)));
        }
        let request = projects_request(args)?;
        let data = self.call_workflow(config, &request).await?;
        let next_actions = projects_next_actions(&data, request.next_actions);
        Ok(json!({
            "status": "ok",
            "data": data,
            "next_actions": next_actions,
        }))
    }

    async fn run_assertions(
        &self,
        config: &CliConfig,
        args: &AssertionsArgs,
    ) -> Result<Value, ApiCommandError> {
        if args.body_template {
            return Ok(template_envelope(assertions_body_template(args)));
        }
        let request = assertions_request(args)?;
        let data = self.call_workflow(config, &request).await?;
        let next_actions = assertions_next_actions(&data, args, request.next_actions);
        Ok(json!({
            "status": "ok",
            "data": data,
            "next_actions": next_actions,
        }))
    }

    async fn run_workflow(
        &self,
        config: &CliConfig,
        request: WorkflowRequest,
    ) -> Result<Value, ApiCommandError> {
        let data = self.call_workflow(config, &request).await?;
        Ok(json!({
            "status": "ok",
            "data": data,
            "next_actions": request.next_actions,
        }))
    }

    async fn fetch_openapi(&self, config: &CliConfig) -> Result<Value, ApiCommandError> {
        let url = self.api_url("/openapi")?;
        let request = self.http_client(config, false, false)?.get(url);
        let response = request.send().await?.error_for_status()?;
        Ok(response.json().await?)
    }

    async fn call_api(
        &self,
        config: &CliConfig,
        input: ApiRequestInput<'_>,
    ) -> Result<Value, ApiCommandError> {
        let url = self.api_url(input.path)?;
        let query = parse_key_values("query", input.query)?;
        let headers = parse_headers(input.header)?;
        let body = read_body(input.body, input.body_file)?;

        let client = self.http_client(
            config,
            !self.allow_unauthenticated,
            input.require_auth && !self.allow_unauthenticated,
        )?;
        let mut request = client.request(input.method.reqwest(), url).headers(headers);

        if !query.is_empty() {
            request = request.query(&query);
        }

        if let Some(body) = body {
            let json_body: Value = serde_json::from_str(&body)?;
            request = request.json(&json_body);
        }

        let response = request.send().await?;
        let status = response.status();
        let headers = response
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
        let body = if content_type.contains("application/json") {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                json!({
                    "parse_error": "response declared JSON but could not be parsed",
                    "raw": String::from_utf8_lossy(&bytes),
                })
            })
        } else {
            json!(String::from_utf8_lossy(&bytes).to_string())
        };

        Ok(json!({
            "request": {
                "method": input.method.as_str(),
                "path": input.path,
            },
            "response": {
                "status": status.as_u16(),
                "success": status.is_success(),
                "headers": headers,
                "body": body,
            }
        }))
    }

    async fn call_workflow(
        &self,
        config: &CliConfig,
        request: &WorkflowRequest,
    ) -> Result<Value, ApiCommandError> {
        let path = self.normalize_project_path(config, &request.path).await?;
        let url = self.api_url(&path)?;
        let client = self.http_client(
            config,
            !self.allow_unauthenticated,
            request.require_auth && !self.allow_unauthenticated,
        )?;
        let mut builder = client.request(request.method.reqwest(), url);
        if !request.query.is_empty() {
            builder = builder.query(&request.query);
        }
        if let Some(body) = &request.body {
            let json_body = self.normalize_request_body(config, &path, body).await?;
            builder = builder.json(&json_body);
        }
        let response = builder.send().await?.error_for_status()?;
        Ok(response.json().await?)
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
            if require_auth && auth.expires_at <= chrono::Utc::now() {
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

    fn api_url(&self, path: &str) -> Result<url::Url, ApiCommandError> {
        if !path.starts_with('/') {
            return Err(ApiCommandError::InvalidPath(path.to_string()));
        }

        let mut url = self.api_url.clone();
        url.set_path(&format!("/api/v1{path}"));
        Ok(url)
    }
}

fn print_output(value: &Value, json_output: bool) -> Result<(), ApiCommandError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        if let Some(data) = value.get("data") {
            print_toon(data, 0);
        }
        if let Some(next_actions) = value.get("next_actions") {
            print_toon_field("next_actions", next_actions, 0);
        }
    }
    Ok(())
}

fn ok_envelope(data: Value) -> Value {
    json!({
        "status": "ok",
        "data": data,
        "next_actions": [
            "pcl api list",
            "pcl api inspect get_views_public_incidents",
            "pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated",
        ],
    })
}

fn api_manifest() -> Value {
    json!({
        "name": "pcl api",
        "description": "Use workflow-shaped commands for every UI/API workflow, with raw OpenAPI commands as an escape hatch.",
        "default_output": "toon",
        "json_output": "Add --json to emit {status,data,next_actions}.",
        "body_input": {
            "preferred": "Use typed flags when available, then --field key=value, then --body-file for nested payloads.",
            "template_flag": "--body-template",
            "field_flag": "--field key=value parses JSON scalars/objects/arrays when VALUE is valid JSON, otherwise a string"
        },
        "auth": {
            "default": "Stored bearer token is attached to API calls.",
            "public_endpoints": "Workflow commands use public view endpoints without requiring login when possible.",
            "login_command": "pcl auth login",
        },
        "commands": [
            {
                "command": "pcl api incidents [--project-id <id>] [--incident-id <id>] [--limit <n>]",
                "description": "List public incidents, project incidents, incident detail, or incident trace.",
                "output": "incident data from /views/public/incidents, /views/projects/{projectId}/incidents, or /views/incidents/{incidentId}",
                "actions": [
                    {"name": "list_public", "method": "GET", "path": "/views/public/incidents", "example": "pcl api incidents --limit 5"},
                    {"name": "list_project", "method": "GET", "path": "/views/projects/{projectId}/incidents", "example": "pcl api incidents --project <project-ref> --limit 10"},
                    {"name": "detail", "method": "GET", "path": "/views/incidents/{incidentId}", "example": "pcl api incidents --incident-id <incident-id>"},
                    {"name": "trace", "method": "GET", "path": "/views/incidents/{incidentId}/transactions/{txId}/trace", "example": "pcl api incidents --incident-id <incident-id> --tx-id <tx-id>"},
                    {"name": "retry_trace", "method": "POST", "path": "/incidents/{incident_id}/transactions/{tx_id}/trace/retry", "example": "pcl api incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace"}
                ]
            },
            {
                "command": "pcl api projects [--project <ref>] [--create|--update|--delete|--save|--unsave|--resolve|--widget]",
                "description": "List, inspect, create, update, save, unsave, resolve, widget, and delete projects.",
                "output": "project explorer, project detail, projects home, saved projects, widget, or mutation result",
                "actions": [
                    {"name": "explorer", "method": "GET", "path": "/views/projects", "example": "pcl api projects --limit 10"},
                    {"name": "home", "method": "GET", "path": "/views/projects/home", "example": "pcl api projects --home"},
                    {"name": "saved", "method": "GET", "path": "/projects/saved", "example": "pcl api projects --saved"},
                    {"name": "detail", "method": "GET", "path": "/projects/{project_id}", "example": "pcl api projects --project <project-ref>"},
                    {"name": "create", "method": "POST", "path": "/projects", "required_body_fields": ["project_name", "chain_id"], "example": "pcl api projects --create --project-name demo --chain-id 1"},
                    {"name": "update", "method": "PUT", "path": "/projects/{project_id}", "example": "pcl api projects --project <project-ref> --update --field github_url=https://github.com/org/repo"},
                    {"name": "delete", "method": "DELETE", "path": "/projects/{project_id}", "example": "pcl api projects --project <project-ref> --delete"},
                    {"name": "save", "method": "POST", "path": "/projects/saved", "example": "pcl api projects --project <project-ref> --save"},
                    {"name": "unsave", "method": "DELETE", "path": "/projects/saved", "example": "pcl api projects --project <project-ref> --unsave"},
                    {"name": "resolve", "method": "GET", "path": "/projects/resolve/{project_ref}", "example": "pcl api projects --project <project-ref> --resolve"},
                    {"name": "widget", "method": "GET", "path": "/projects/{project_id}/widget", "example": "pcl api projects --project <project-ref> --widget"}
                ]
            },
            {
                "command": "pcl api assertions --project <ref> [--assertion-id <id>|--submitted|--registered|--submit|--remove-info|--remove-calldata]",
                "description": "List, inspect, submit, and manage project assertion lifecycle state.",
                "output": "assertion index/detail, submitted assertions, registered assertions, removal info/calldata, or submit result",
                "actions": [
                    {"name": "index", "method": "GET", "path": "/views/projects/{projectId}/assertions", "example": "pcl api assertions --project <project-ref>"},
                    {"name": "detail", "method": "GET", "path": "/views/projects/{projectId}/assertions/{assertionId}", "example": "pcl api assertions --project <project-ref> --assertion-id <assertion-id>"},
                    {"name": "adopter_lookup", "method": "GET", "path": "/assertions", "required_flags": ["--adopter-address"], "optional_flags": ["--network", "--environment", "--include-onchain-only"], "example": "pcl api assertions --adopter-address 0x... --network 1"},
                    {"name": "submitted", "method": "GET", "path": "/projects/{project_id}/submitted-assertions", "example": "pcl api assertions --project <project-ref> --submitted"},
                    {"name": "submit", "method": "POST", "path": "/projects/{project_id}/submitted-assertions", "required_body_fields": ["assertions"], "example": "pcl api assertions --project <project-ref> --submit --body-file submitted-assertions.json"},
                    {"name": "registered", "method": "GET", "path": "/projects/{project_id}/registered-assertions", "example": "pcl api assertions --project <project-ref> --registered"},
                    {"name": "remove_info", "method": "GET", "path": "/projects/{project_id}/remove-assertions-info", "example": "pcl api assertions --project <project-ref> --remove-info"},
                    {"name": "remove_calldata", "method": "GET", "path": "/projects/{project_id}/remove-assertions-calldata", "example": "pcl api assertions --project <project-ref> --remove-calldata"}
                ]
            },
            {
                "command": "pcl api search [--query <term>] [--stats] [--system-status] [--verified-contract --address <addr> --chain-id <id>]",
                "description": "Search projects/contracts and inspect platform metadata.",
                "output": "search results, stats, system status, health, whitelist, or verified contract data",
                "actions": [
                    {"name": "query", "auth": false, "method": "GET", "path": "/search", "optional_flags": ["--query"], "example": "pcl api search --query settler"},
                    {"name": "stats", "auth": false, "method": "GET", "path": "/stats", "example": "pcl api search --stats"},
                    {"name": "system_status", "auth": false, "method": "GET", "path": "/system-status", "example": "pcl api search --system-status"},
                    {"name": "health", "auth": false, "method": "GET", "path": "/health", "example": "pcl api search --health"},
                    {"name": "whitelist", "auth": true, "method": "GET", "path": "/whitelist", "example": "pcl api search --whitelist"},
                    {"name": "verified_contract", "auth": false, "method": "GET", "path": "/web/verified-contract", "required_flags": ["--address", "--chain-id"], "example": "pcl api search --verified-contract --address 0x... --chain-id 1"}
                ]
            },
            {
                "command": "pcl api account [--me|--accept-terms|--logout]",
                "description": "Inspect authenticated web user state and perform onboarding actions.",
                "output": "current user account state, terms acceptance result, or logout result",
                "actions": [
                    {"name": "me", "auth": true, "method": "GET", "path": "/web/auth/me", "example": "pcl api account"},
                    {"name": "accept_terms", "auth": true, "method": "POST", "path": "/web/auth/accept-terms", "body_template": "empty_object", "example": "pcl api account --accept-terms"},
                    {"name": "logout", "auth": true, "method": "POST", "path": "/web/auth/logout", "body_template": "empty_object", "example": "pcl api account --logout"}
                ]
            },
            {
                "command": "pcl api contracts [--project <ref>] [--adopter-id <id>] [--unassigned] [--create --body '{...}']",
                "description": "List and manage project contracts and assertion adopters.",
                "output": "contract views, adopter records, assignment results, or remove calldata",
                "actions": [
                    {"name": "list_all", "auth": true, "method": "GET", "path": "/assertion_adopters", "example": "pcl api contracts"},
                    {"name": "list_project", "auth": true, "method": "GET", "path": "/views/projects/{project}/contracts", "required_flags": ["--project"], "example": "pcl api contracts --project <project-ref>"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/views/projects/{project}/contracts/{adopter_id}", "required_flags": ["--project", "--adopter-id"], "example": "pcl api contracts --project <project-ref> --adopter-id <adopter-id>"},
                    {"name": "unassigned", "auth": true, "method": "GET", "path": "/assertion_adopters/no-project", "example": "pcl api contracts --unassigned"},
                    {"name": "create", "auth": true, "method": "POST", "path": "/assertion_adopters", "body_template": "contracts", "example": "pcl api contracts --create --body-template"},
                    {"name": "assign_project", "auth": true, "method": "POST", "path": "/assertion_adopters/assign-project", "body_template": "contracts_assign_project", "example": "pcl api contracts --assign-project --body-template"},
                    {"name": "remove", "auth": true, "method": "DELETE", "path": "/projects/{project}/{aa_address}", "required_flags": ["--project", "--aa-address"], "example": "pcl api contracts --project <project-ref> --aa-address 0x... --remove"},
                    {"name": "remove_calldata", "auth": true, "method": "GET", "path": "/assertion_adopters/{aa_address}/remove-assertions-calldata", "required_flags": ["--aa-address"], "example": "pcl api contracts --aa-address 0x... --remove-calldata"}
                ]
            },
            {
                "command": "pcl api releases --project <ref> [--release-id <id>] [--preview|--create|--deploy|--remove|--deploy-calldata|--remove-calldata]",
                "description": "List, inspect, create, preview, deploy, and remove releases.",
                "output": "release data, diffs, deployment confirmations, or calldata",
                "actions": [
                    {"name": "list", "auth": true, "method": "GET", "path": "/projects/{project}/releases", "required_flags": ["--project"], "example": "pcl api releases --project <project-ref>"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}", "required_flags": ["--project", "--release-id"], "example": "pcl api releases --project <project-ref> --release-id <release-id>"},
                    {"name": "preview", "auth": true, "method": "POST", "path": "/projects/{project}/releases/preview", "required_flags": ["--project"], "body_template": "release", "example": "pcl api releases --project <project-ref> --preview --body-file release.json"},
                    {"name": "create", "auth": true, "method": "POST", "path": "/projects/{project}/releases", "required_flags": ["--project"], "body_template": "release", "example": "pcl api releases --project <project-ref> --create --body-file release.json"},
                    {"name": "deploy_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}/deploy-calldata", "required_flags": ["--project", "--release-id"], "example": "pcl api releases --project <project-ref> --release-id <release-id> --deploy-calldata"},
                    {"name": "deploy", "auth": true, "method": "POST", "path": "/projects/{project}/releases/{release_id}/deploy", "required_flags": ["--project", "--release-id"], "body_template": "release_deploy", "example": "pcl api releases --project <project-ref> --release-id <release-id> --deploy --body-template"},
                    {"name": "remove_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/releases/{release_id}/remove-calldata", "required_flags": ["--project", "--release-id"], "example": "pcl api releases --project <project-ref> --release-id <release-id> --remove-calldata"},
                    {"name": "remove", "auth": true, "method": "POST", "path": "/projects/{project}/releases/{release_id}/remove", "required_flags": ["--project", "--release-id"], "body_template": "release_remove", "example": "pcl api releases --project <project-ref> --release-id <release-id> --remove --body-template"}
                ]
            },
            {
                "command": "pcl api deployments --project <ref> [--confirm --body '{...}']",
                "description": "Inspect deployment state and confirm deployed assertions.",
                "output": "deployment view or confirmation result",
                "actions": [
                    {"name": "list", "auth": true, "method": "GET", "path": "/views/projects/{project}/deployments", "required_flags": ["--project"], "example": "pcl api deployments --project <project-ref>"},
                    {"name": "confirm", "auth": true, "method": "POST", "path": "/projects/{project}/confirm-deployment", "required_flags": ["--project"], "body_template": "deployment_confirmation", "example": "pcl api deployments --project <project-ref> --confirm --body-template"}
                ]
            },
            {
                "command": "pcl api access [--project <ref>] [--members|--invitations|--invite|--pending|--token <token>]",
                "description": "Manage project members, roles, and invitations.",
                "output": "member lists, invitation lists, role data, or mutation results",
                "actions": [
                    {"name": "members", "auth": true, "method": "GET", "path": "/projects/{project}/members", "required_flags": ["--project"], "example": "pcl api access --project <project-ref> --members"},
                    {"name": "my_role", "auth": true, "method": "GET", "path": "/projects/{project}/my-role", "required_flags": ["--project"], "example": "pcl api access --project <project-ref> --my-role"},
                    {"name": "invitations", "auth": true, "method": "GET", "path": "/projects/{project}/invitations", "required_flags": ["--project"], "example": "pcl api access --project <project-ref> --invitations"},
                    {"name": "invite", "auth": true, "method": "POST", "path": "/projects/{project}/invitations", "required_flags": ["--project"], "body_template": "access_invite", "example": "pcl api access --project <project-ref> --invite --body-template"},
                    {"name": "resend", "auth": true, "method": "POST", "path": "/projects/{project}/invitations/{invitation_id}/resend", "required_flags": ["--project", "--invitation-id"], "body_template": "empty_object", "example": "pcl api access --project <project-ref> --invitation-id <id> --resend"},
                    {"name": "revoke", "auth": true, "method": "DELETE", "path": "/projects/{project}/invitations/{invitation_id}", "required_flags": ["--project", "--invitation-id"], "body_template": "empty_object", "example": "pcl api access --project <project-ref> --invitation-id <id> --revoke"},
                    {"name": "update_role", "auth": true, "method": "PATCH", "path": "/projects/{project}/members/{member_user_id}", "required_flags": ["--project", "--member-user-id"], "body_template": "role_update", "example": "pcl api access --project <project-ref> --member-user-id <user-id> --update-role --body-template"},
                    {"name": "remove", "auth": true, "method": "DELETE", "path": "/projects/{project}/members/{member_user_id}", "required_flags": ["--project", "--member-user-id"], "body_template": "empty_object", "example": "pcl api access --project <project-ref> --member-user-id <user-id> --remove"},
                    {"name": "pending", "auth": true, "method": "GET", "path": "/invitations/pending", "example": "pcl api access --pending"},
                    {"name": "preview", "auth": false, "method": "GET", "path": "/invitations/{token}/preview", "required_flags": ["--token"], "example": "pcl api access --token <token> --preview"},
                    {"name": "accept", "auth": true, "method": "POST", "path": "/invitations/{token}/accept", "required_flags": ["--token"], "body_template": "empty_object", "example": "pcl api access --token <token> --accept"}
                ]
            },
            {
                "command": "pcl api integrations --project <ref> --provider <slack|pagerduty> [--configure|--test|--delete]",
                "description": "Manage Slack and PagerDuty integrations.",
                "output": "integration status or mutation/test results",
                "actions": [
                    {"name": "get", "auth": true, "method": "GET", "path": "/projects/{project}/integrations/{provider}", "required_flags": ["--project", "--provider"], "example": "pcl api integrations --project <project-ref> --provider slack"},
                    {"name": "configure", "auth": true, "method": "POST", "path": "/projects/{project}/integrations/{provider}", "required_flags": ["--project", "--provider"], "body_template": "slack|pagerduty", "example": "pcl api integrations --project <project-ref> --provider slack --configure --body-template"},
                    {"name": "test", "auth": true, "method": "POST", "path": "/projects/{project}/integrations/{provider}/test", "required_flags": ["--project", "--provider"], "body_template": "slack|pagerduty", "example": "pcl api integrations --project <project-ref> --provider slack --test"},
                    {"name": "delete", "auth": true, "method": "DELETE", "path": "/projects/{project}/integrations/{provider}", "required_flags": ["--project", "--provider"], "example": "pcl api integrations --project <project-ref> --provider slack --delete"}
                ]
            },
            {
                "command": "pcl api protocol-manager --project <ref> [--nonce|--set|--clear|--transfer-calldata|--accept-calldata|--pending-transfer|--confirm-transfer]",
                "description": "Manage protocol manager transfers and calldata.",
                "output": "manager state, nonce, calldata, pending transfer, or mutation result",
                "actions": [
                    {"name": "pending_transfer", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/pending-transfer", "required_flags": ["--project"], "example": "pcl api protocol-manager --project <project-ref> --pending-transfer"},
                    {"name": "nonce", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/nonce", "required_flags": ["--project"], "example": "pcl api protocol-manager --project <project-ref> --nonce"},
                    {"name": "set", "auth": true, "method": "POST", "path": "/projects/{project}/protocol-manager", "required_flags": ["--project"], "body_template": "protocol_manager_set", "example": "pcl api protocol-manager --project <project-ref> --set --body-template"},
                    {"name": "clear", "auth": true, "method": "DELETE", "path": "/projects/{project}/protocol-manager", "required_flags": ["--project"], "body_template": "empty_object", "example": "pcl api protocol-manager --project <project-ref> --clear"},
                    {"name": "transfer_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/transfer-calldata", "required_flags": ["--project", "--new-manager"], "query": {"new_manager": "<address>"}, "example": "pcl api protocol-manager --project <project-ref> --transfer-calldata --new-manager 0x..."},
                    {"name": "accept_calldata", "auth": true, "method": "GET", "path": "/projects/{project}/protocol-manager/accept-calldata", "required_flags": ["--project"], "example": "pcl api protocol-manager --project <project-ref> --accept-calldata"},
                    {"name": "confirm_transfer", "auth": true, "method": "POST", "path": "/projects/{project}/protocol-manager/confirm-transfer", "required_flags": ["--project"], "body_template": "protocol_manager_confirm", "example": "pcl api protocol-manager --project <project-ref> --confirm-transfer --body-template"}
                ]
            },
            {
                "command": "pcl api transfers [--pending|--transfer-id <id>|--reject --body '{...}']",
                "description": "Inspect and reject protocol manager transfers.",
                "output": "pending transfers, transfer detail, or reject result",
                "actions": [
                    {"name": "pending", "auth": true, "method": "GET", "path": "/views/transfers/pending", "example": "pcl api transfers --pending"},
                    {"name": "detail", "auth": true, "method": "GET", "path": "/views/transfers/{transfer_id}", "required_flags": ["--transfer-id"], "example": "pcl api transfers --transfer-id <transfer-id>"},
                    {"name": "reject", "auth": true, "method": "POST", "path": "/transfers/reject", "body_template": "transfer_reject", "example": "pcl api transfers --reject --body-template"}
                ]
            },
            {
                "command": "pcl api events --project <ref> [--audit-log]",
                "description": "Inspect project events and audit logs.",
                "output": "event or audit log data",
                "actions": [
                    {"name": "events", "auth": true, "method": "GET", "path": "/views/projects/{project}/events", "required_flags": ["--project"], "optional_flags": ["--page", "--limit", "--environment"], "example": "pcl api events --project <project-ref>"},
                    {"name": "audit_log", "auth": true, "method": "GET", "path": "/views/projects/{project}/audit-log", "required_flags": ["--project"], "optional_flags": ["--page", "--limit", "--environment"], "example": "pcl api events --project <project-ref> --audit-log"}
                ]
            },
            {
                "command": "pcl api manifest",
                "description": "Print this agent-readable command manifest.",
            },
            {
                "command": "pcl api list [--filter <term>] [--method <get|post|put|patch|delete>]",
                "description": "List OpenAPI operations with executable inspect and call commands.",
                "output": "operations[] with operation_id, method, path, summary, tags, inspect_command, call_command",
            },
            {
                "command": "pcl api inspect <operation_id>|<method> <path> [--full]",
                "description": "Inspect a compact operation manifest. Use --full for raw OpenAPI.",
                "output": "operation_id, method, path, path_params, required_query, body_fields, required_body_fields, body_template, response_statuses, example_call",
            },
            {
                "command": "pcl api call <method> <path> [--query key=value] [--body '{...}']",
                "description": "Execute any endpoint below /api/v1.",
                "output": "request and response status/body",
            },
        ],
        "examples": [
            "pcl api incidents --limit 5",
            "pcl api search --query settler",
            "pcl api releases --project <project-ref>",
            "pcl api access --project <project-ref> --members",
            "pcl api integrations --project <project-ref> --provider slack",
            "pcl api list --filter incidents",
        ],
    })
}

fn search_request(args: &SearchArgs) -> Result<WorkflowRequest, ApiCommandError> {
    if args.health {
        return Ok(WorkflowRequest::get(
            "/health",
            false,
            vec!["pcl api search --system-status".to_string()],
        ));
    }
    if args.system_status {
        return Ok(WorkflowRequest::get(
            "/system-status",
            false,
            vec!["pcl api search --stats".to_string()],
        ));
    }
    if args.stats {
        return Ok(WorkflowRequest::get(
            "/stats",
            false,
            vec!["pcl api projects --limit 10".to_string()],
        ));
    }
    if args.whitelist {
        return Ok(WorkflowRequest::get(
            "/whitelist",
            true,
            vec!["pcl api projects --home".to_string()],
        ));
    }
    if args.verified_contract {
        let address = required_arg(args.address.as_deref(), "--address")?;
        let chain_id = args.chain_id.ok_or_else(|| {
            ApiCommandError::InvalidWorkflow {
                message: "--verified-contract requires --chain-id".to_string(),
            }
        })?;
        let mut request = WorkflowRequest::get(
            "/web/verified-contract",
            false,
            vec!["pcl api contracts --project <project-ref>".to_string()],
        );
        push_query_string_value(&mut request.query, "address", address);
        push_query(&mut request.query, "chainId", Some(chain_id));
        return Ok(request);
    }

    let mut request = WorkflowRequest::get(
        "/search",
        false,
        vec![
            "pcl api projects --project <project-ref>".to_string(),
            "pcl api contracts --project <project-ref>".to_string(),
        ],
    );
    push_query_string(&mut request.query, "query", &args.query);
    Ok(request)
}

fn account_request(args: &AccountArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    if args.accept_terms {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/web/auth/accept-terms",
            true,
            body.or_else(|| Some(json!({}).to_string())),
            vec![
                "pcl api account".to_string(),
                "pcl api projects --home".to_string(),
            ],
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
            "pcl api account --accept-terms".to_string(),
            "pcl api projects --home".to_string(),
        ],
    ))
}

fn contracts_request(args: &ContractsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/assertion_adopters",
            true,
            body,
            vec!["pcl api contracts --unassigned".to_string()],
        ));
    }
    if args.assign_project {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/assertion_adopters/assign-project",
            true,
            body,
            vec!["pcl api contracts --project <project-ref>".to_string()],
        ));
    }
    if args.unassigned {
        return Ok(WorkflowRequest::get(
            "/assertion_adopters/no-project",
            true,
            vec!["pcl api contracts --assign-project --body '{...}'".to_string()],
        ));
    }
    if args.remove_calldata {
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        return Ok(WorkflowRequest::get(
            format!("/assertion_adopters/{address}/remove-assertions-calldata"),
            true,
            vec!["pcl api releases --project <project-ref>".to_string()],
        ));
    }
    if args.remove {
        let project = required_arg(args.project.as_deref(), "--project")?;
        let address = required_arg(args.aa_address.as_deref(), "--aa-address")?;
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/{address}"),
            true,
            body,
            vec![format!("pcl api contracts --project {project}")],
        ));
    }
    if let Some(project) = &args.project {
        if let Some(adopter_id) = &args.adopter_id {
            return Ok(WorkflowRequest::get(
                format!("/views/projects/{project}/contracts/{adopter_id}"),
                true,
                vec![format!("pcl api contracts --project {project}")],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/views/projects/{project}/contracts"),
            true,
            vec![format!(
                "pcl api contracts --project {project} --adopter-id <adopter-id>"
            )],
        ));
    }

    Ok(WorkflowRequest::get(
        "/assertion_adopters",
        true,
        vec!["pcl api contracts --unassigned".to_string()],
    ))
}

fn releases_request(args: &ReleasesArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    let project = &args.project;
    if args.preview {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases/preview"),
            true,
            body,
            vec![format!(
                "pcl api releases --project {project} --create --body-file release.json"
            )],
        ));
    }
    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/releases"),
            true,
            body,
            vec![format!("pcl api releases --project {project}")],
        ));
    }
    if args.deploy || args.remove || args.deploy_calldata || args.remove_calldata {
        let release_id = required_arg(args.release_id.as_deref(), "--release-id")?;
        if args.deploy {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/deploy"),
                true,
                body,
                vec![format!(
                    "pcl api releases --project {project} --release-id {release_id}"
                )],
            ));
        }
        if args.remove {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/releases/{release_id}/remove"),
                true,
                body,
                vec![format!("pcl api releases --project {project}")],
            ));
        }
        if args.deploy_calldata {
            return Ok(WorkflowRequest::get(
                format!("/projects/{project}/releases/{release_id}/deploy-calldata"),
                true,
                vec![format!(
                    "pcl api releases --project {project} --release-id {release_id} --deploy"
                )],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases/{release_id}/remove-calldata"),
            true,
            vec![format!(
                "pcl api releases --project {project} --release-id {release_id} --remove"
            )],
        ));
    }
    let Some(release_id) = &args.release_id else {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/releases"),
            true,
            vec![format!(
                "pcl api releases --project {project} --release-id <release-id>"
            )],
        ));
    };
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/releases/{release_id}"),
        true,
        vec![
            format!(
                "pcl api releases --project {project} --release-id {release_id} --deploy-calldata"
            ),
            format!(
                "pcl api releases --project {project} --release-id {release_id} --remove-calldata"
            ),
        ],
    ))
}

fn deployments_request(args: &DeploymentsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    if args.confirm {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{}/confirm-deployment", args.project),
            true,
            body,
            vec![format!("pcl api deployments --project {}", args.project)],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/views/projects/{}/deployments", args.project),
        true,
        vec![format!("pcl api releases --project {}", args.project)],
    ))
}

fn access_request(args: &AccessArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    if args.pending {
        return Ok(WorkflowRequest::get(
            "/invitations/pending",
            true,
            vec!["pcl api access --token <token> --accept".to_string()],
        ));
    }
    if args.accept || args.preview {
        let token = required_arg(args.token.as_deref(), "--token")?;
        if args.accept {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/invitations/{token}/accept"),
                true,
                body,
                vec!["pcl api projects --home".to_string()],
            ));
        }
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl api access --token {token} --accept")],
        ));
    }
    if let Some(token) = &args.token {
        return Ok(WorkflowRequest::get(
            format!("/invitations/{token}/preview"),
            false,
            vec![format!("pcl api access --token {token} --accept")],
        ));
    }
    let project = required_arg(args.project.as_deref(), "--project")?;
    if args.my_role {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/my-role"),
            true,
            vec![format!("pcl api access --project {project} --members")],
        ));
    }
    if args.invite {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project}/invitations"),
            true,
            body,
            vec![format!("pcl api access --project {project} --invitations")],
        ));
    }
    if args.resend || args.revoke {
        let invitation_id = required_arg(args.invitation_id.as_deref(), "--invitation-id")?;
        if args.resend {
            return Ok(workflow_with_body(
                HttpMethod::Post,
                format!("/projects/{project}/invitations/{invitation_id}/resend"),
                true,
                body,
                vec![format!("pcl api access --project {project} --invitations")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/invitations/{invitation_id}"),
            true,
            body,
            vec![format!("pcl api access --project {project} --invitations")],
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
                vec![format!("pcl api access --project {project} --members")],
            ));
        }
        return Ok(workflow_with_body(
            HttpMethod::Delete,
            format!("/projects/{project}/members/{member_user_id}"),
            true,
            body,
            vec![format!("pcl api access --project {project} --members")],
        ));
    }
    if args.invitations {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project}/invitations"),
            true,
            vec![format!(
                "pcl api access --project {project} --invite --body '{{...}}'"
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("/projects/{project}/members"),
        true,
        vec![
            format!("pcl api access --project {project} --my-role"),
            format!("pcl api access --project {project} --invitations"),
        ],
    ))
}

fn integrations_request(args: &IntegrationsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    let provider = args.provider.path();
    let base = format!("/projects/{}/integrations/{provider}", args.project);
    if args.configure {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl api integrations --project {} --provider {provider}",
                args.project
            )],
        ));
    }
    if args.test {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("{base}/test"),
            true,
            body,
            vec![format!(
                "pcl api integrations --project {} --provider {provider}",
                args.project
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
                "pcl api integrations --project {} --provider slack",
                args.project
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        base,
        true,
        vec![
            format!(
                "pcl api integrations --project {} --provider {provider} --test",
                args.project
            ),
            format!(
                "pcl api integrations --project {} --provider {provider} --configure --body '{{...}}'",
                args.project
            ),
        ],
    ))
}

fn protocol_manager_request(
    args: &ProtocolManagerArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    let base = format!("/projects/{}/protocol-manager", args.project);
    if args.nonce {
        return Ok(WorkflowRequest::get(
            format!("{base}/nonce"),
            true,
            vec![format!(
                "pcl api protocol-manager --project {} --set --body '{{...}}'",
                args.project
            )],
        ));
    }
    if args.set {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            base,
            true,
            body,
            vec![format!(
                "pcl api protocol-manager --project {} --pending-transfer",
                args.project
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
                "pcl api protocol-manager --project {} --nonce",
                args.project
            )],
        ));
    }
    if args.transfer_calldata {
        let new_manager = required_arg(args.new_manager.as_deref(), "--new-manager")?;
        let mut request = WorkflowRequest::get(
            format!("{base}/transfer-calldata"),
            true,
            vec![format!(
                "pcl api protocol-manager --project {} --set --body '{{...}}'",
                args.project
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
                "pcl api protocol-manager --project {} --confirm-transfer --body '{{...}}'",
                args.project
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
                "pcl api protocol-manager --project {} --pending-transfer",
                args.project
            )],
        ));
    }
    Ok(WorkflowRequest::get(
        format!("{base}/pending-transfer"),
        true,
        vec![
            format!(
                "pcl api protocol-manager --project {} --nonce",
                args.project
            ),
            format!(
                "pcl api protocol-manager --project {} --transfer-calldata",
                args.project
            ),
        ],
    ))
}

fn transfers_request(args: &TransfersArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;
    if args.reject {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/transfers/reject",
            true,
            body,
            vec!["pcl api transfers --pending".to_string()],
        ));
    }
    if let Some(transfer_id) = &args.transfer_id {
        return Ok(WorkflowRequest::get(
            format!("/views/transfers/{transfer_id}"),
            true,
            vec!["pcl api transfers --pending".to_string()],
        ));
    }
    Ok(WorkflowRequest::get(
        "/views/transfers/pending",
        true,
        vec!["pcl api transfers --transfer-id <transfer-id>".to_string()],
    ))
}

fn events_request(args: &EventsArgs) -> WorkflowRequest {
    let mut request = if args.audit_log {
        WorkflowRequest::get(
            format!("/views/projects/{}/audit-log", args.project),
            true,
            vec![format!("pcl api events --project {}", args.project)],
        )
    } else {
        WorkflowRequest::get(
            format!("/views/projects/{}/events", args.project),
            true,
            vec![format!(
                "pcl api events --project {} --audit-log",
                args.project
            )],
        )
    };
    push_query(&mut request.query, "page", args.page);
    push_query(&mut request.query, "limit", args.limit);
    push_query_string(&mut request.query, "environment", &args.environment);
    request
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

fn request_body(
    body: Option<&str>,
    body_file: &Option<PathBuf>,
    fields: &[String],
) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(body, body_file)?;
    body_with_fields(body, fields)
}

fn project_request_body(args: &ProjectsArgs) -> Result<Option<String>, ApiCommandError> {
    let body = read_body(args.body.as_deref(), &args.body_file)?;
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
    json!({
        "status": "ok",
        "data": data,
        "next_actions": [
            "Pass the template with --body-file <path>",
            "Or pass individual fields with --field key=value"
        ],
    })
}

fn project_body_template(args: &ProjectsArgs) -> Value {
    if args.update {
        return body_template("project_update");
    }
    if args.save || args.unsave {
        return body_template("project_saved");
    }
    if args.delete || args.resolve || args.widget || args.home || args.saved {
        return body_template("empty_object");
    }
    body_template("project_create")
}

fn assertions_body_template(args: &AssertionsArgs) -> Value {
    if args.submit {
        return body_template("submitted_assertions");
    }
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
    if args.deploy_calldata || args.remove_calldata || args.release_id.is_some() {
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
    body_template(args.provider.path())
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
                "project_description": null,
                "profile_image_url": null,
                "is_private": false
            })
        }
        "project_update" => {
            json!({
                "project_name": "<name>",
                "project_description": null,
                "github_url": null,
                "profile_image_url": null,
                "is_dev": false,
                "is_private": false,
                "assertion_adopters": []
            })
        }
        "project_saved" => json!({ "project_id": "<project-uuid>" }),
        "submitted_assertions" => {
            json!({
                "assertions": [
                    {
                        "contract_name": "<contract-name>",
                        "assertion_id": "<assertion-id>",
                        "signature": "0x..."
                    }
                ]
            })
        }
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
                "mode": "direct",
                "new_manager_address": "0x..."
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
                body: None,
                require_auth: true,
                next_actions: vec![format!(
                    "pcl api incidents --incident-id {incident_id} --tx-id {tx_id}"
                )],
            });
        }
        let path = if let Some(tx_id) = &args.tx_id {
            format!("/views/incidents/{incident_id}/transactions/{tx_id}/trace")
        } else {
            format!("/views/incidents/{incident_id}")
        };
        let next_actions = vec![
            "pcl api incidents --limit 5".to_string(),
            format!("pcl api inspect get {}", path),
        ];
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path,
            query,
            body: None,
            require_auth: false,
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
                    "pcl api incidents --project-id {project_id} --limit 10"
                )],
            });
        }
        push_query_string(&mut query, "assertionId", &args.assertion_id);
        push_query_string(&mut query, "assertionAdopterId", &args.assertion_adopter_id);
        push_query_string(&mut query, "environment", &args.environment);
        push_query_string(&mut query, "fromDate", &args.from_date);
        push_query_string(&mut query, "toDate", &args.to_date);
        let path = format!("/views/projects/{project_id}/incidents");
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path,
            query,
            body: None,
            require_auth: true,
            next_actions: vec![
                format!("pcl api assertions --project-id {project_id}"),
                "pcl api incidents --limit 5".to_string(),
            ],
        });
    }

    push_query(&mut query, "network", args.network);
    push_query_string(&mut query, "sort", &args.sort);
    push_query_string(&mut query, "devMode", &args.dev_mode);
    Ok(WorkflowRequest {
        method: HttpMethod::Get,
        path: "/views/public/incidents".to_string(),
        query,
        body: None,
        require_auth: false,
        next_actions: vec![
            "pcl api incidents --project-id <project-id> --limit 10".to_string(),
            "pcl api projects --limit 10".to_string(),
        ],
    })
}

fn incidents_next_actions(
    data: &Value,
    args: &IncidentsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if args.incident_id.is_some() {
        return fallback;
    }
    first_string_field(data, &["id", "incidentId", "incident_id"])
        .map(|incident_id| {
            vec![
                format!("pcl api incidents --incident-id {incident_id}"),
                "pcl api projects --limit 10".to_string(),
            ]
        })
        .unwrap_or(fallback)
}

fn projects_next_actions(data: &Value, fallback: Vec<String>) -> Vec<String> {
    first_string_field(data, &["project_id", "projectId", "id"])
        .map(|project_id| {
            vec![
                format!("pcl api projects --project-id {project_id}"),
                format!("pcl api assertions --project-id {project_id}"),
                format!("pcl api incidents --project-id {project_id} --limit 10"),
            ]
        })
        .unwrap_or(fallback)
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
        .map(|address| vec![format!("pcl api assertions --adopter-address {address}")])
        .unwrap_or(fallback);
    };

    first_string_field(data, &["assertion_id", "assertionId", "id"])
        .map(|assertion_id| {
            vec![
                format!(
                    "pcl api assertions --project-id {project_id} --assertion-id {assertion_id}",
                ),
                format!(
                    "pcl api incidents --project-id {project_id} --assertion-id {assertion_id}",
                ),
            ]
        })
        .unwrap_or(fallback)
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
    push_query_string(&mut query, "search", &args.search);
    let body = project_request_body(args)?;

    if args.create {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            "/projects",
            true,
            body,
            vec!["pcl api projects --home".to_string()],
        ));
    }

    if args.home {
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: "/views/projects/home".to_string(),
            query,
            body: None,
            require_auth: true,
            next_actions: vec!["pcl api projects --saved".to_string()],
        });
    }
    if args.saved {
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: "/projects/saved".to_string(),
            query,
            body: None,
            require_auth: true,
            next_actions: vec!["pcl api projects --home".to_string()],
        });
    }
    if args.project_id.is_none()
        && (args.update || args.delete || args.save || args.unsave || args.resolve || args.widget)
    {
        required_arg(args.project_id.as_deref(), "--project")?;
    }
    if let Some(project_id) = &args.project_id {
        if args.resolve {
            return Ok(WorkflowRequest {
                method: HttpMethod::Get,
                path: format!("/projects/resolve/{project_id}"),
                query,
                body: None,
                require_auth: false,
                next_actions: vec![format!("pcl api projects --project-id {project_id}")],
            });
        }
        if args.widget {
            return Ok(WorkflowRequest::get(
                format!("/projects/{project_id}/widget"),
                true,
                vec![format!("pcl api projects --project-id {project_id}")],
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
                vec!["pcl api projects --saved".to_string()],
            ));
        }
        if args.update {
            return Ok(workflow_with_body(
                HttpMethod::Put,
                format!("/projects/{project_id}"),
                true,
                body,
                vec![format!("pcl api projects --project-id {project_id}")],
            ));
        }
        if args.delete {
            return Ok(workflow_with_body(
                HttpMethod::Delete,
                format!("/projects/{project_id}"),
                true,
                body,
                vec!["pcl api projects --home".to_string()],
            ));
        }
        return Ok(WorkflowRequest {
            method: HttpMethod::Get,
            path: format!("/projects/{project_id}"),
            query,
            body: None,
            require_auth: true,
            next_actions: vec![
                format!("pcl api assertions --project-id {project_id}"),
                format!("pcl api incidents --project-id {project_id} --limit 10"),
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
            "pcl api projects --project-id <project-id>".to_string(),
            "pcl api incidents --limit 5".to_string(),
        ],
    })
}

fn assertions_request(args: &AssertionsArgs) -> Result<WorkflowRequest, ApiCommandError> {
    let body = request_body(args.body.as_deref(), &args.body_file, &args.field)?;

    if let Some(adopter_address) = &args.adopter_address {
        let mut request = WorkflowRequest::get(
            "/assertions",
            false,
            vec!["pcl api contracts --project <project-ref>".to_string()],
        );
        push_query_string_value(
            &mut request.query,
            "adopter_address",
            adopter_address.clone(),
        );
        push_query_string(&mut request.query, "network", &args.network);
        push_query_string(&mut request.query, "environment", &args.environment);
        push_query(
            &mut request.query,
            "include_onchain_only",
            args.include_onchain_only,
        );
        return Ok(request);
    }

    let project_id = required_arg(args.project_id.as_deref(), "--project")?;
    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);
    push_query_string(&mut query, "assertionAdopterId", &args.adopter_id);
    push_query_string(&mut query, "environment", &args.environment);

    if args.submit {
        return Ok(workflow_with_body(
            HttpMethod::Post,
            format!("/projects/{project_id}/submitted-assertions"),
            true,
            body,
            vec![format!(
                "pcl api assertions --project-id {project_id} --submitted"
            )],
        ));
    }
    if args.submitted {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/submitted-assertions"),
            true,
            vec![format!(
                "pcl api assertions --project-id {project_id} --registered"
            )],
        ));
    }
    if args.registered {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/registered-assertions"),
            true,
            vec![format!("pcl api assertions --project-id {project_id}")],
        ));
    }
    if args.remove_info {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/remove-assertions-info"),
            true,
            vec![format!(
                "pcl api assertions --project-id {project_id} --remove-calldata"
            )],
        ));
    }
    if args.remove_calldata {
        return Ok(WorkflowRequest::get(
            format!("/projects/{project_id}/remove-assertions-calldata"),
            true,
            vec![format!("pcl api releases --project {project_id}")],
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
                "pcl api incidents --project-id {project_id} --assertion-id {assertion_id}",
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
            format!("pcl api incidents --project-id {project_id} --limit 10"),
            format!("pcl api assertions --project-id {project_id} --assertion-id <assertion-id>"),
        ],
    })
}

fn push_query<T: ToString>(query: &mut Vec<(String, String)>, name: &str, value: Option<T>) {
    if let Some(value) = value {
        query.push((name.to_string(), value.to_string()));
    }
}

fn push_query_string(query: &mut Vec<(String, String)>, name: &str, value: &Option<String>) {
    if let Some(value) = value {
        query.push((name.to_string(), value.clone()));
    }
}

fn print_toon(value: &Value, indent: usize) {
    match value {
        Value::Object(object) => print_toon_object(object, indent),
        Value::Array(items) => print_toon_array(items, indent),
        _ => println!("{}{}", spaces(indent), scalar_to_string(value)),
    }
}

fn print_toon_object(object: &Map<String, Value>, indent: usize) {
    for (key, value) in object {
        print_toon_field(key, value, indent);
    }
}

fn print_toon_field(key: &str, value: &Value, indent: usize) {
    let prefix = spaces(indent);
    match value {
        Value::Object(object) => {
            println!("{prefix}{key}:");
            print_toon_object(object, indent + 2);
        }
        Value::Array(items) => print_toon_named_array(key, items, indent),
        _ => println!("{prefix}{key}: {}", scalar_to_string(value)),
    }
}

fn print_toon_named_array(key: &str, items: &[Value], indent: usize) {
    let prefix = spaces(indent);
    if items.is_empty() {
        println!("{prefix}{key}: []");
        return;
    }
    if let Some(columns) = scalar_object_columns(items) {
        println!("{prefix}{key}[{}]{{{}}}:", items.len(), columns.join(","));
        for item in items {
            print_table_row(item, &columns, indent + 2);
        }
        return;
    }
    println!("{prefix}{key}[{}]:", items.len());
    print_toon_array(items, indent + 2);
}

fn print_toon_array(items: &[Value], indent: usize) {
    for item in items {
        let prefix = spaces(indent);
        match item {
            Value::Object(object) => {
                println!("{prefix}-");
                print_toon_object(object, indent + 2);
            }
            Value::Array(items) => {
                println!("{prefix}-");
                print_toon_array(items, indent + 2);
            }
            _ => println!("{prefix}- {}", scalar_to_string(item)),
        }
    }
}

fn scalar_object_columns(items: &[Value]) -> Option<Vec<String>> {
    let object = items.first()?.as_object()?;
    let columns = object.keys().cloned().collect::<Vec<_>>();
    let scalar_objects = items.iter().all(|item| {
        item.as_object().is_some_and(|object| {
            object.keys().eq(columns.iter()) && object.values().all(is_toon_scalar)
        })
    });
    scalar_objects.then_some(columns)
}

fn print_table_row(item: &Value, columns: &[String], indent: usize) {
    let Some(object) = item.as_object() else {
        return;
    };
    let row = columns
        .iter()
        .filter_map(|column| object.get(column).map(scalar_to_string))
        .collect::<Vec<_>>()
        .join(",");
    println!("{}{}", spaces(indent), row);
}

fn is_toon_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn spaces(indent: usize) -> String {
    " ".repeat(indent)
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
    body_file: &Option<PathBuf>,
) -> Result<Option<String>, ApiCommandError> {
    if let Some(body) = body {
        return Ok(Some(body.to_string()));
    }

    if let Some(path) = body_file {
        if path == &PathBuf::from("-") {
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

            let Some(operation) = path_item
                .get(method.openapi_key())
                .and_then(Value::as_object)
            else {
                continue;
            };

            let operation_id = operation
                .get("operationId")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| synthetic_operation_id(method, path));
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

            operations.push(OperationSummary {
                inspect_command: format!("pcl api inspect {operation_id}"),
                call_command: format!("pcl api call {} {}", method.openapi_key(), path),
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
            .map(ToString::to_string)
            .unwrap_or_else(|| synthetic_operation_id(method, path));
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
                .map(ToString::to_string)
                .unwrap_or_else(|| synthetic_operation_id(method, candidate_path));
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
    let mut manifest = json!({
        "operation_id": operation_id,
        "method": method.as_str(),
        "path": path,
        "summary": operation.get("summary").and_then(Value::as_str),
        "description": operation.get("description").and_then(Value::as_str),
        "parameters": operation_parameters(operation),
        "path_params": named_parameters(operation, "path", false),
        "required_query": named_parameters(operation, "query", true),
        "request_body": request_body_manifest(operation),
        "body_fields": body_fields(operation),
        "required_body_fields": required_body_fields(operation),
        "body_template": openapi_body_template(operation),
        "response_statuses": response_statuses(operation),
        "example_call": example_call(method, path, operation),
    });

    if full && let Some(object) = manifest.as_object_mut() {
        object.insert("operation".to_string(), operation.clone());
    }

    manifest
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
            .map(compact_schema_type)
            .unwrap_or_else(|| "unknown".to_string()),
    })
}

fn body_schema(operation: &Value) -> Option<&Value> {
    operation.pointer("/requestBody/content/application~1json/schema")
}

fn required_body_fields(operation: &Value) -> Vec<String> {
    body_schema(operation)
        .and_then(|schema| schema.get("required"))
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
    let required = required_body_fields(operation);
    body_schema(operation)
        .and_then(|schema| schema.get("properties"))
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
                    })
                })
                .collect()
        })
        .unwrap_or_default()
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
                    .map(template_from_schema)
                    .unwrap_or(Value::String("<item>".to_string())),
            ])
        }
        Some("integer") | Some("number") => json!(0),
        Some("boolean") => json!(false),
        Some("string") => {
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
                    .map(template_from_schema)
                    .unwrap_or(Value::String("<value>".to_string()));
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
    let mut command = format!("pcl api call {} {}", method.openapi_key(), path);
    for parameter in required_query_parameters(operation) {
        command.push_str(&format!(" --query {parameter}=<value>"));
    }
    if operation.get("requestBody").is_some() {
        command.push_str(" --body '{...}'");
    }
    command
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
    operations
        .first()
        .map(|operation| {
            vec![
                operation.inspect_command.clone(),
                operation.call_command.clone(),
            ]
        })
        .unwrap_or_else(|| vec!["pcl api list".to_string(), "pcl api manifest".to_string()])
}

fn command_next_actions(inspected: &Value) -> Vec<String> {
    inspected
        .get("example_call")
        .and_then(Value::as_str)
        .map(|command| vec![command.to_string()])
        .unwrap_or_else(|| vec!["pcl api list".to_string()])
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
mod tests {
    use super::*;
    use clap::Parser;

    fn assertions_args(project_id: Option<&str>) -> AssertionsArgs {
        AssertionsArgs {
            project_id: project_id.map(ToString::to_string),
            assertion_id: None,
            adopter_id: None,
            adopter_address: None,
            network: None,
            include_onchain_only: None,
            environment: None,
            page: None,
            limit: None,
            submitted: false,
            registered: false,
            submit: false,
            remove_info: false,
            remove_calldata: false,
            field: Vec::new(),
            body: None,
            body_file: None,
            body_template: false,
        }
    }

    fn protocol_manager_args() -> ProtocolManagerArgs {
        ProtocolManagerArgs {
            project: "project-1".to_string(),
            nonce: false,
            set: false,
            clear: false,
            transfer_calldata: false,
            accept_calldata: false,
            pending_transfer: false,
            confirm_transfer: false,
            new_manager: None,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: false,
        }
    }

    fn access_args() -> AccessArgs {
        AccessArgs {
            project: Some("project-1".to_string()),
            member_user_id: None,
            invitation_id: None,
            token: None,
            members: false,
            invitations: false,
            pending: false,
            preview: false,
            accept: false,
            invite: false,
            resend: false,
            revoke: false,
            update_role: false,
            remove: false,
            my_role: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: false,
        }
    }

    fn release_args() -> ReleasesArgs {
        ReleasesArgs {
            project: "project-1".to_string(),
            release_id: None,
            create: false,
            preview: false,
            deploy: false,
            remove: false,
            deploy_calldata: false,
            remove_calldata: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: false,
        }
    }

    #[test]
    fn parses_key_values() {
        let parsed = parse_key_values("query", &["limit=5".to_string()]).unwrap();
        assert_eq!(parsed, vec![("limit".to_string(), "5".to_string())]);
    }

    #[test]
    fn lists_and_inspects_operations() {
        let spec = json!({
            "paths": {
                "/views/public/incidents": {
                    "get": {
                        "operationId": "get_views_public_incidents",
                        "summary": "Get public incidents",
                        "tags": ["views"]
                    }
                }
            }
        });

        let operations = list_operations(&spec, Some("incidents"), Some(HttpMethod::Get)).unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0].operation_id, "get_views_public_incidents");

        let operation =
            inspect_operation(&spec, "get_views_public_incidents", None, false).unwrap();
        assert_eq!(operation["method"], "GET");
        assert_eq!(operation["path"], "/views/public/incidents");
    }

    #[test]
    fn synthesizes_missing_operation_ids() {
        assert_eq!(
            synthetic_operation_id(HttpMethod::Post, "/web/auth/bootstrap-session"),
            "post_web_auth_bootstrap_session"
        );
    }

    #[test]
    fn builds_public_incidents_workflow_request() {
        let request = incidents_request(&IncidentsArgs {
            project_id: None,
            incident_id: None,
            tx_id: None,
            assertion_id: None,
            assertion_adopter_id: None,
            environment: None,
            from_date: None,
            to_date: None,
            page: None,
            limit: Some(5),
            network: None,
            sort: None,
            dev_mode: None,
            stats: false,
            retry_trace: false,
        })
        .unwrap();

        assert_eq!(request.path, "/views/public/incidents");
        assert!(!request.require_auth);
        assert_eq!(request.query, vec![("limit".to_string(), "5".to_string())]);
    }

    #[test]
    fn builds_project_incidents_workflow_request() {
        let request = incidents_request(&IncidentsArgs {
            project_id: Some("project-1".to_string()),
            incident_id: None,
            tx_id: None,
            assertion_id: Some("assertion-1".to_string()),
            assertion_adopter_id: None,
            environment: Some("production".to_string()),
            from_date: None,
            to_date: None,
            page: None,
            limit: Some(10),
            network: None,
            sort: None,
            dev_mode: None,
            stats: false,
            retry_trace: false,
        })
        .unwrap();

        assert_eq!(request.path, "/views/projects/project-1/incidents");
        assert!(request.require_auth);
        assert!(
            request
                .query
                .contains(&("limit".to_string(), "10".to_string()))
        );
        assert!(
            request
                .query
                .contains(&("assertionId".to_string(), "assertion-1".to_string()))
        );
        assert!(
            request
                .query
                .contains(&("environment".to_string(), "production".to_string()))
        );
    }

    #[test]
    fn builds_incident_trace_retry_request() {
        let request = incidents_request(&IncidentsArgs {
            project_id: None,
            incident_id: Some("incident-1".to_string()),
            tx_id: Some("tx-1".to_string()),
            assertion_id: None,
            assertion_adopter_id: None,
            environment: None,
            from_date: None,
            to_date: None,
            page: None,
            limit: None,
            network: None,
            sort: None,
            dev_mode: None,
            stats: false,
            retry_trace: true,
        })
        .unwrap();

        assert_eq!(
            request.path,
            "/incidents/incident-1/transactions/tx-1/trace/retry"
        );
        assert_eq!(request.method.openapi_key(), "post");
        assert!(request.require_auth);
    }

    #[test]
    fn builds_project_create_body_from_typed_flags() {
        let request = projects_request(&ProjectsArgs {
            project_id: None,
            home: false,
            saved: false,
            page: None,
            limit: None,
            search: None,
            create: true,
            update: false,
            delete: false,
            save: false,
            unsave: false,
            resolve: false,
            widget: false,
            project_name: Some("Demo".to_string()),
            project_description: None,
            profile_image_url: None,
            github_url: None,
            chain_id: Some(1),
            is_private: Some(false),
            is_dev: None,
            field: Vec::new(),
            body: None,
            body_file: None,
            body_template: false,
        })
        .unwrap();

        assert_eq!(request.path, "/projects");
        assert_eq!(request.method.openapi_key(), "post");
        assert_eq!(
            serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
            json!({
                "project_name": "Demo",
                "chain_id": 1,
                "is_private": false
            })
        );
    }

    #[test]
    fn builds_assertion_lifecycle_requests() {
        let submitted = assertions_request(&AssertionsArgs {
            submitted: true,
            ..assertions_args(Some("project-1"))
        })
        .unwrap();
        assert_eq!(submitted.path, "/projects/project-1/submitted-assertions");

        let remove = assertions_request(&AssertionsArgs {
            remove_calldata: true,
            ..assertions_args(Some("project-1"))
        })
        .unwrap();
        assert_eq!(
            remove.path,
            "/projects/project-1/remove-assertions-calldata"
        );
    }

    #[test]
    fn builds_adopter_assertion_lookup_request() {
        let request = assertions_request(&AssertionsArgs {
            adopter_address: Some("0xabc".to_string()),
            network: Some("1".to_string()),
            environment: Some("production".to_string()),
            include_onchain_only: Some(true),
            ..assertions_args(None)
        })
        .unwrap();

        assert_eq!(request.path, "/assertions");
        assert!(!request.require_auth);
        assert_eq!(
            request.query,
            vec![
                ("adopter_address".to_string(), "0xabc".to_string()),
                ("network".to_string(), "1".to_string()),
                ("environment".to_string(), "production".to_string()),
                ("include_onchain_only".to_string(), "true".to_string()),
            ]
        );
    }

    #[test]
    fn project_assertions_require_project_id() {
        let error = assertions_request(&assertions_args(None)).unwrap_err();
        assert!(error.to_string().contains("--project is required"));
    }

    #[test]
    fn protocol_manager_transfer_calldata_uses_new_manager_query() {
        let request = protocol_manager_request(&ProtocolManagerArgs {
            transfer_calldata: true,
            new_manager: Some("0xmanager".to_string()),
            ..protocol_manager_args()
        })
        .unwrap();

        assert_eq!(
            request.path,
            "/projects/project-1/protocol-manager/transfer-calldata"
        );
        assert_eq!(
            request.query,
            vec![("new_manager".to_string(), "0xmanager".to_string())]
        );
    }

    #[test]
    fn protocol_manager_transfer_calldata_requires_new_manager() {
        let error = protocol_manager_request(&ProtocolManagerArgs {
            transfer_calldata: true,
            ..protocol_manager_args()
        })
        .unwrap_err();

        assert!(error.to_string().contains("--new-manager is required"));
    }

    #[test]
    fn write_actions_require_target_identifiers() {
        let project_error = projects_request(&ProjectsArgs {
            project_id: None,
            home: false,
            saved: false,
            page: None,
            limit: None,
            search: None,
            create: false,
            update: false,
            delete: false,
            save: true,
            unsave: false,
            resolve: false,
            widget: false,
            project_name: None,
            project_description: None,
            profile_image_url: None,
            github_url: None,
            chain_id: None,
            is_private: None,
            is_dev: None,
            field: Vec::new(),
            body: None,
            body_file: None,
            body_template: false,
        })
        .unwrap_err();
        assert!(project_error.to_string().contains("--project is required"));

        let release_error = releases_request(&ReleasesArgs {
            deploy: true,
            ..release_args()
        })
        .unwrap_err();
        assert!(
            release_error
                .to_string()
                .contains("--release-id is required")
        );

        let token_error = access_request(&AccessArgs {
            token: None,
            accept: true,
            ..access_args()
        })
        .unwrap_err();
        assert!(token_error.to_string().contains("--token is required"));

        let invitation_error = access_request(&AccessArgs {
            resend: true,
            ..access_args()
        })
        .unwrap_err();
        assert!(
            invitation_error
                .to_string()
                .contains("--invitation-id is required")
        );

        let member_error = access_request(&AccessArgs {
            update_role: true,
            ..access_args()
        })
        .unwrap_err();
        assert!(
            member_error
                .to_string()
                .contains("--member-user-id is required")
        );
    }

    #[test]
    fn builds_account_workflow_requests() {
        let me = account_request(&AccountArgs {
            me: true,
            accept_terms: false,
            logout: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: false,
        })
        .unwrap();
        assert_eq!(me.path, "/web/auth/me");
        assert_eq!(me.method.openapi_key(), "get");

        let accept_terms = account_request(&AccountArgs {
            me: false,
            accept_terms: true,
            logout: false,
            body: None,
            field: Vec::new(),
            body_file: None,
            body_template: false,
        })
        .unwrap();
        assert_eq!(accept_terms.path, "/web/auth/accept-terms");
        assert_eq!(accept_terms.method.openapi_key(), "post");
        assert_eq!(accept_terms.body.as_deref(), Some("{}"));
    }

    #[test]
    fn body_templates_are_action_specific() {
        assert_eq!(
            access_body_template(&AccessArgs {
                project: Some("project-1".to_string()),
                member_user_id: Some("user-1".to_string()),
                invitation_id: None,
                token: None,
                members: false,
                invitations: false,
                pending: false,
                preview: false,
                accept: false,
                invite: false,
                resend: false,
                revoke: false,
                update_role: true,
                remove: false,
                my_role: false,
                body: None,
                field: Vec::new(),
                body_file: None,
                body_template: true,
            }),
            json!({ "role": "viewer" })
        );
        assert_eq!(
            release_body_template(&ReleasesArgs {
                project: "project-1".to_string(),
                release_id: Some("release-1".to_string()),
                create: false,
                preview: false,
                deploy: true,
                remove: false,
                deploy_calldata: false,
                remove_calldata: false,
                body: None,
                field: Vec::new(),
                body_file: None,
                body_template: true,
            }),
            json!({ "chainId": 1, "txHash": "0x..." })
        );
        assert_eq!(
            release_body_template(&ReleasesArgs {
                project: "project-1".to_string(),
                release_id: Some("release-1".to_string()),
                create: false,
                preview: false,
                deploy: false,
                remove: false,
                deploy_calldata: true,
                remove_calldata: false,
                body: None,
                field: Vec::new(),
                body_file: None,
                body_template: true,
            }),
            json!({})
        );
        assert_eq!(
            access_body_template(&AccessArgs {
                project: Some("project-1".to_string()),
                member_user_id: None,
                invitation_id: None,
                token: None,
                members: true,
                invitations: false,
                pending: false,
                preview: false,
                accept: false,
                invite: false,
                resend: false,
                revoke: false,
                update_role: false,
                remove: false,
                my_role: false,
                body: None,
                field: Vec::new(),
                body_file: None,
                body_template: true,
            }),
            json!({})
        );
        assert_eq!(
            protocol_manager_body_template(&ProtocolManagerArgs {
                transfer_calldata: true,
                new_manager: Some("0xmanager".to_string()),
                ..protocol_manager_args()
            }),
            json!({})
        );
        assert_eq!(
            body_template("pagerduty"),
            json!({ "routing_key": "<pagerduty-routing-key>", "enabled": true })
        );
    }

    #[test]
    fn manifest_lists_structured_actions_for_every_workflow() {
        let manifest = api_manifest();
        let commands = manifest["commands"].as_array().unwrap();
        for command_name in [
            "incidents",
            "projects",
            "assertions",
            "search",
            "account",
            "contracts",
            "releases",
            "deployments",
            "access",
            "integrations",
            "protocol-manager",
            "transfers",
            "events",
        ] {
            let command = commands
                .iter()
                .find(|command| {
                    command["command"]
                        .as_str()
                        .is_some_and(|value| value.contains(command_name))
                })
                .unwrap_or_else(|| panic!("missing manifest command {command_name}"));
            let actions = command["actions"].as_array().unwrap_or_else(|| {
                panic!("missing structured actions for manifest command {command_name}")
            });
            assert!(!actions.is_empty(), "empty actions for {command_name}");
        }
    }

    #[test]
    fn parser_rejects_conflicting_workflow_actions() {
        assert!(ApiArgs::try_parse_from(["api", "projects", "--save", "--unsave"]).is_err());
        assert!(
            ApiArgs::try_parse_from([
                "api",
                "releases",
                "--project",
                "project-1",
                "--deploy",
                "--remove"
            ])
            .is_err()
        );
        assert!(
            ApiArgs::try_parse_from(["api", "transfers", "--transfer-id", "t1", "--reject"])
                .is_err()
        );
    }

    #[test]
    fn summarizes_openapi_body_fields() {
        let operation = json!({
            "requestBody": {
                "content": {
                    "application/json": {
                        "schema": {
                            "type": "object",
                            "required": ["name"],
                            "properties": {
                                "name": {"type": "string"},
                                "private": {"type": "boolean"}
                            }
                        }
                    }
                }
            }
        });

        assert_eq!(required_body_fields(&operation), vec!["name"]);
        assert_eq!(body_fields(&operation).len(), 2);
        assert_eq!(
            openapi_body_template(&operation),
            json!({"name": "<string>", "private": false})
        );
    }

    #[test]
    fn project_list_next_actions_use_returned_project_id() {
        let data = json!({
            "data": {
                "items": [
                    {
                        "project_id": "project-1",
                        "project_name": "Project One"
                    }
                ]
            }
        });

        let next_actions = projects_next_actions(&data, Vec::new());
        assert_eq!(
            next_actions,
            vec![
                "pcl api projects --project-id project-1",
                "pcl api assertions --project-id project-1",
                "pcl api incidents --project-id project-1 --limit 10",
            ]
        );
    }
}
