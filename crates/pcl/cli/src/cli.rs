use clap::Parser;
use pcl_common::args::CliArgs;
#[cfg(feature = "credible")]
use pcl_core::verify::VerifyArgs;
use pcl_core::{
    DEFAULT_PLATFORM_URL,
    api::ApiArgs,
    apply::ApplyArgs,
    auth::AuthCommand,
    config::ConfigArgs,
    download::DownloadArgs,
};
use pcl_phoundry::build::BuildArgs;
#[cfg(feature = "credible")]
use pcl_phoundry::phorge_test::PhorgeTest;
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
        matches!(self, Self::Config(config) if config.can_run_without_valid_config())
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
