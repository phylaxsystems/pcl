use clap::{
    CommandFactory,
    Parser,
};
use clap_complete::Shell;
use pcl_common::args::CliArgs;
#[cfg(feature = "credible")]
use pcl_core::verify::VerifyArgs;
use pcl_core::{
    DEFAULT_PLATFORM_URL,
    api::{
        AccessCommand,
        AccountCommand,
        ApiArgs,
        AssertionsCommand,
        ContractsCommand,
        DeploymentsCommand,
        EventsCommand,
        IncidentsCommand,
        IntegrationsCommand,
        ProjectsCommand,
        ProtocolManagerCommand,
        ReleasesCommand,
        SearchCommand,
        TransfersCommand,
        with_envelope_metadata,
    },
    apply::ApplyArgs,
    auth::AuthCommand,
    config::ConfigArgs,
    download::DownloadArgs,
    surface::{
        ArtifactsArgs,
        DoctorArgs,
        ExportArgs,
        JobsArgs,
        LlmsArgs,
        RequestsArgs,
        SchemaArgs,
        WhoamiArgs,
        WorkflowsArgs,
    },
};
use pcl_phoundry::build::BuildArgs;
#[cfg(feature = "credible")]
use pcl_phoundry::phorge_test::PhorgeTest;
use serde_json::json;
use std::{
    io,
    sync::OnceLock,
};

fn version_message() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            format!(
                "{}\nCommit: {}\nBuild Timestamp: {}\nDefault Platform URL: {}",
                env!("CARGO_PKG_VERSION"),
                env!("VERGEN_GIT_SHA"),
                env!("VERGEN_BUILD_TIMESTAMP"),
                DEFAULT_PLATFORM_URL,
            )
        })
        .as_str()
}

#[derive(Parser)]
#[command(
    name = "pcl",
    version = version_message(),
    long_version = version_message(),
    about = "The Credible CLI for the Credible Layer"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[command(flatten)]
    pub args: CliArgs,
}

#[derive(clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    #[cfg(feature = "credible")]
    #[command(name = "test")]
    Test(PhorgeTest),
    #[command(name = "apply")]
    Apply(ApplyArgs),
    #[command(name = "api")]
    Api(ApiArgs),
    #[command(name = "incidents")]
    Incidents(IncidentsCommand),
    #[command(name = "projects")]
    Projects(ProjectsCommand),
    #[command(name = "assertions")]
    Assertions(AssertionsCommand),
    #[command(name = "search")]
    Search(SearchCommand),
    #[command(name = "account")]
    Account(AccountCommand),
    #[command(name = "contracts")]
    Contracts(ContractsCommand),
    #[command(name = "releases")]
    Releases(ReleasesCommand),
    #[command(name = "deployments")]
    Deployments(DeploymentsCommand),
    #[command(name = "access")]
    Access(AccessCommand),
    #[command(name = "integrations")]
    Integrations(IntegrationsCommand),
    #[command(name = "protocol-manager")]
    ProtocolManager(ProtocolManagerCommand),
    #[command(name = "transfers")]
    Transfers(TransfersCommand),
    #[command(name = "events")]
    Events(EventsCommand),
    #[command(name = "doctor")]
    Doctor(DoctorArgs),
    #[command(name = "whoami")]
    Whoami(WhoamiArgs),
    #[command(name = "workflows")]
    Workflows(WorkflowsArgs),
    #[command(name = "export")]
    Export(ExportArgs),
    #[command(name = "artifacts")]
    Artifacts(ArtifactsArgs),
    #[command(name = "requests", alias = "logs")]
    Requests(RequestsArgs),
    #[command(name = "schema")]
    Schema(SchemaArgs),
    #[command(name = "llms")]
    Llms(LlmsArgs),
    #[command(name = "jobs")]
    Jobs(JobsArgs),
    #[command(name = "completions")]
    Completions(CompletionsArgs),
    Auth(AuthCommand),
    #[command(about = "Manage configuration")]
    Config(ConfigArgs),
    #[command(name = "build")]
    Build(BuildArgs),
    #[cfg(feature = "credible")]
    #[command(name = "verify")]
    Verify(VerifyArgs),
    #[command(name = "download")]
    Download(DownloadArgs),
}

impl Commands {
    pub fn can_run_without_valid_config(&self) -> bool {
        matches!(
            self,
            Self::Config(config) if config.can_run_without_valid_config()
        ) || matches!(
            self,
            Self::Doctor(_)
                | Self::Workflows(_)
                | Self::Artifacts(_)
                | Self::Requests(_)
                | Self::Schema(_)
                | Self::Llms(_)
                | Self::Jobs(_)
                | Self::Completions(_)
        )
    }

    pub fn should_write_after_invalid_config(&self) -> bool {
        matches!(self, Self::Config(config) if config.can_run_without_valid_config())
    }
}

#[derive(clap::Args)]
#[command(about = "Generate shell completion scripts")]
pub struct CompletionsArgs {
    #[arg(value_enum, help = "Shell to generate completions for")]
    shell: Shell,
}

impl CompletionsArgs {
    pub fn run(&self, json_output: bool) -> Result<(), serde_json::Error> {
        let mut command = Cli::command();
        if json_output {
            let mut script = Vec::new();
            clap_complete::generate(self.shell, &mut command, "pcl", &mut script);
            let script = String::from_utf8_lossy(&script).to_string();
            let envelope = with_envelope_metadata(json!({
                "status": "ok",
                "data": {
                    "shell": self.shell.to_string(),
                    "script": script,
                    "install_note": "Run without --json and redirect stdout into your shell completion directory.",
                },
                "next_actions": [
                    format!("pcl completions {}", self.shell),
                ],
            }));
            println!("{}", serde_json::to_string_pretty(&envelope)?);
        } else {
            clap_complete::generate(self.shell, &mut command, "pcl", &mut io::stdout());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_config_show_command() {
        let cli = Cli::try_parse_from(["pcl", "config", "show"]).unwrap();
        assert!(matches!(cli.command, Commands::Config(_)));
    }

    #[test]
    fn parses_hidden_config_dir_globally() {
        let cli = Cli::try_parse_from(["pcl", "--config-dir", "/tmp/pcl", "config", "show"])
            .expect("config-dir should parse as a hidden global flag");
        assert_eq!(
            cli.args.config_dir.as_deref(),
            Some(std::path::Path::new("/tmp/pcl"))
        );
        assert!(matches!(cli.command, Commands::Config(_)));
    }

    #[test]
    fn parses_apply_command() {
        let cli =
            Cli::try_parse_from(["pcl", "apply", "--root", "./testdata/mock-protocol"]).unwrap();
        match cli.command {
            Commands::Apply(args) => {
                assert_eq!(
                    args.root,
                    std::path::PathBuf::from("./testdata/mock-protocol")
                );
                assert_eq!(
                    args.config,
                    std::path::PathBuf::from("assertions/credible.toml")
                );
                assert!(!args.json);
                assert!(!args.yes);
            }
            _ => panic!("expected apply command"),
        }
    }

    #[test]
    fn parses_api_call_command() {
        let cli = Cli::try_parse_from([
            "pcl",
            "api",
            "call",
            "get",
            "/views/public/incidents",
            "--query",
            "limit=5",
            "--json",
        ])
        .unwrap();
        assert!(matches!(cli.command, Commands::Api(_)));
        assert!(cli.args.json);
    }

    #[test]
    fn parses_top_level_workflow_commands() {
        let incidents = Cli::try_parse_from([
            "pcl",
            "incidents",
            "--project-id",
            "project-1",
            "--all",
            "--limit",
            "50",
        ])
        .unwrap();
        assert!(matches!(incidents.command, Commands::Incidents(_)));

        let projects = Cli::try_parse_from([
            "pcl",
            "projects",
            "--dry-run",
            "--create",
            "--project-name",
            "demo",
            "--chain-id",
            "1",
        ])
        .unwrap();
        assert!(matches!(projects.command, Commands::Projects(_)));

        let manager = Cli::try_parse_from([
            "pcl",
            "protocol-manager",
            "--confirm-transfer",
            "--body-template",
        ])
        .unwrap();
        assert!(matches!(manager.command, Commands::ProtocolManager(_)));
    }

    #[test]
    fn parses_agent_product_surface_commands() {
        assert!(matches!(
            Cli::try_parse_from(["pcl", "doctor", "--offline"])
                .unwrap()
                .command,
            Commands::Doctor(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["pcl", "workflows", "show", "incident-investigation"])
                .unwrap()
                .command,
            Commands::Workflows(_)
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "pcl",
                "schema",
                "get",
                "incidents",
                "--action",
                "list_public"
            ])
            .unwrap()
            .command,
            Commands::Schema(_)
        ));
        assert!(matches!(
            Cli::try_parse_from([
                "pcl",
                "export",
                "incidents",
                "--project-id",
                "project-1",
                "--dry-run"
            ])
            .unwrap()
            .command,
            Commands::Export(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["pcl", "logs", "list"])
                .unwrap()
                .command,
            Commands::Requests(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["pcl", "llms"]).unwrap().command,
            Commands::Llms(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["pcl", "jobs", "list"])
                .unwrap()
                .command,
            Commands::Jobs(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["pcl", "completions", "bash"])
                .unwrap()
                .command,
            Commands::Completions(_)
        ));
    }

    #[test]
    fn parses_apply_command_with_custom_config() {
        let cli = Cli::try_parse_from([
            "pcl",
            "apply",
            "--root",
            "./testdata/mock-protocol",
            "-c",
            "custom/path/credible.toml",
        ])
        .unwrap();
        match cli.command {
            Commands::Apply(args) => {
                assert_eq!(
                    args.config,
                    std::path::PathBuf::from("custom/path/credible.toml")
                );
            }
            _ => panic!("expected apply command"),
        }
    }
}
