use super::{
    ApiArgs,
    ApiCommand,
    ApiCommandError,
    WorkflowCommand,
};
use crate::config::CliConfig;
use pcl_common::args::CliArgs;
use std::cell::Cell;

#[derive(clap::Args, Debug)]
pub(in crate::api) struct ApiWorkflowOptions {
    #[arg(
        long = "api-url",
        env = "PCL_API_URL",
        global = true,
        help = "Base URL for the platform API. Defaults to the platform remembered from the last login or network selection"
    )]
    api_url: Option<url::Url>,

    #[arg(
        long,
        global = true,
        help = "Do not attach the stored bearer token to API requests"
    )]
    allow_unauthenticated: bool,
}

impl ApiWorkflowOptions {
    /// The explicit `--api-url`/`PCL_API_URL` value, when one was given.
    pub(in crate::api) fn platform_url_flag(&self) -> Option<&url::Url> {
        self.api_url.as_ref()
    }

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
