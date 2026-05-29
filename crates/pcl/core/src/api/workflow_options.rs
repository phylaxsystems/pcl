use super::{
    ApiArgs,
    ApiCommand,
    ApiCommandError,
    WorkflowCommand,
};
use crate::{
    DEFAULT_PLATFORM_URL,
    config::CliConfig,
};
use pcl_common::args::CliArgs;
use std::cell::Cell;

#[derive(clap::Args, Debug)]
pub(in crate::api) struct ApiWorkflowOptions {
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

impl ApiWorkflowOptions {
    pub(in crate::api) async fn run(
        self,
        command: WorkflowCommand,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), ApiCommandError> {
        ApiArgs {
            command: ApiCommand::Manifest,
            api_url: self.api_url,
            allow_unauthenticated: self.allow_unauthenticated,
            refresh_after_401: Cell::new(true),
        }
        .run_workflow_command(&command, config, cli_args, json_output)
        .await
    }
}
