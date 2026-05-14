use clap::{
    CommandFactory,
    Parser,
};
use clap_complete::Shell;
use pcl_common::args::{
    CliArgs,
    OutputMode,
    current_output_mode,
};
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
use std::sync::OnceLock;

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
    about = "The Credible CLI for the Credible Layer",
    long_about = "The Credible CLI for the Credible Layer.\n\nUse workflow commands for normal project, assertion, incident, release, and access work. Use --toon for agents and --json for strict parsers. Use `pcl --toon --llms` for agent guidance. Use `pcl api` only when a workflow command does not exist or when debugging the raw API."
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
    Auth(AuthCommand),
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
    #[command(name = "api")]
    Api(ApiArgs),
    #[command(name = "completions")]
    Completions(CompletionsArgs),
    #[command(about = "Manage configuration")]
    Config(ConfigArgs),
    #[command(name = "apply")]
    Apply(ApplyArgs),
    #[cfg(feature = "credible")]
    #[command(name = "verify")]
    Verify(VerifyArgs),
    #[command(name = "download")]
    Download(DownloadArgs),
    #[command(name = "build")]
    Build(BuildArgs),
    #[cfg(feature = "credible")]
    #[command(name = "test")]
    Test(PhorgeTest),
}

impl Commands {
    pub fn can_run_without_valid_config(&self) -> bool {
        matches!(
            self,
            Self::Config(config) if config.can_run_without_valid_config()
        ) || matches!(
            self,
            Self::Auth(auth) if auth.can_run_without_valid_config()
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
            || matches!(self, Self::Auth(auth) if auth.can_run_without_valid_config())
    }

    pub fn should_force_config_write(&self) -> bool {
        matches!(self, Self::Config(config) if config.should_force_config_write())
            || matches!(self, Self::Auth(auth) if auth.should_force_config_write())
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
        let script = completion_script(self.shell);
        let output_mode = if json_output {
            OutputMode::Json
        } else {
            current_output_mode()
        };
        if output_mode == OutputMode::Human {
            print!("{script}");
        } else {
            let envelope = with_envelope_metadata(json!({
                "status": "ok",
                "data": {
                    "shell": self.shell.to_string(),
                    "script": script,
                    "install_note": "Run without --toon/--json and redirect stdout into your shell completion directory.",
                },
                "next_actions": [
                    format!("pcl completions {} > <completion-file>", self.shell),
                ],
            }));
            print!(
                "{}",
                pcl_core::api::envelope_output_string(&envelope, json_output)?
            );
        }
        Ok(())
    }
}

fn completion_script(shell: Shell) -> String {
    let mut command = Cli::command();
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, "pcl", &mut script);
    strip_hidden_completion_options(&String::from_utf8_lossy(&script))
}

fn strip_hidden_completion_options(script: &str) -> String {
    let mut lines = Vec::new();
    let mut skipped_bash_case_lines = 0_u8;
    for line in script.lines() {
        if skipped_bash_case_lines > 0 {
            skipped_bash_case_lines -= 1;
            continue;
        }
        if line.trim_start().starts_with("--config-dir)") {
            skipped_bash_case_lines = 3;
            continue;
        }
        if line.contains("'--config-dir") || line.contains("-l config-dir") {
            continue;
        }
        lines.push(
            line.replace(" --config-dir", "")
                .replace(" config-dir=", ""),
        );
    }
    let mut output = lines.join("\n");
    if script.ends_with('\n') {
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{
        Parser,
        error::ErrorKind,
    };

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
    fn parses_output_json_as_global_machine_output() {
        let cli = Cli::try_parse_from([
            "pcl",
            "--format",
            "json",
            "api",
            "call",
            "get",
            "/views/public/incidents",
        ])
        .unwrap();
        assert!(matches!(cli.command, Commands::Api(_)));
        assert!(cli.args.json_output());
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

    #[test]
    fn every_visible_command_accepts_toon_before_help() {
        let mut command_paths = Vec::new();
        let command = Cli::command();
        collect_command_paths(&command, &[], &mut command_paths);
        assert!(!command_paths.is_empty(), "expected visible commands");

        for path in command_paths {
            let mut argv = vec!["pcl".to_string()];
            argv.extend(path.iter().cloned());
            argv.push("--toon".to_string());
            argv.push("--help".to_string());

            let Err(err) = Cli::try_parse_from(argv.clone()) else {
                panic!("expected help display for `{}`", argv.join(" "));
            };
            assert_eq!(
                err.kind(),
                ErrorKind::DisplayHelp,
                "`{}` should accept --toon and then display help; got {err}",
                argv.join(" ")
            );
        }
    }

    fn collect_command_paths(
        command: &clap::Command,
        prefix: &[String],
        paths: &mut Vec<Vec<String>>,
    ) {
        let subcommands = command
            .get_subcommands()
            .filter(|subcommand| !subcommand.is_hide_set() && subcommand.get_name() != "help")
            .collect::<Vec<_>>();

        if !prefix.is_empty() {
            paths.push(prefix.to_owned());
        }

        for subcommand in subcommands {
            let mut path = prefix.to_owned();
            path.push(subcommand.get_name().to_string());
            collect_command_paths(subcommand, &path, paths);
        }
    }
}
