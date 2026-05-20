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
    config::CliConfig,
};
use clap::{
    ArgGroup,
    ValueEnum,
};
use pcl_common::args::CliArgs;
use std::{
    cell::Cell,
    path::PathBuf,
};

mod definitions;
mod envelopes;
mod error;
mod input;
mod manifest;
mod method;
mod openapi;
mod operations;
mod render;
mod runner;
mod runtime_types;
mod spec;
mod templates;
mod transport;
mod workflow_options;
mod workflows;

pub use crate::output::{
    ENVELOPE_SCHEMA_VERSION,
    with_envelope_metadata,
};
pub use error::ApiCommandError;
pub use manifest::api_manifest;
pub use render::{
    envelope_output_string,
    human_string,
    toon_string,
};

use envelopes::{
    extract_paginated_items,
    ok_envelope,
    query_pairs_value,
    upsert_query,
    workflow_data_for_output_mode,
    workflow_success_envelope,
    workflow_success_envelope_with_data,
};
pub(in crate::api) use error::method_side_effecting;
use input::{
    parse_headers,
    parse_key_values,
    read_body,
    split_path_and_inline_query,
    write_json_output_file,
    write_jsonl_items_output_file,
};
pub(in crate::api) use method::HttpMethod;
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
use operations::WorkflowOperation;
use render::print_output;
use runtime_types::{
    ApiRequestInput,
    PreparedApiRequest,
    RawPaginationOptions,
    WorkflowCallResult,
    WorkflowPaginationOptions,
    WorkflowRequest,
};
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
use transport::{
    read_api_response,
    write_request_log,
};
pub(crate) use transport::{
    request_id_from_headers,
    response_body_value,
};
use workflow_options::ApiWorkflowOptions;
use workflows::{
    access_request,
    account_request,
    assertions_next_actions,
    assertions_request,
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

    #[arg(skip = Cell::new(true))]
    refresh_after_401: Cell<bool>,
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

#[cfg(test)]
mod tests;
