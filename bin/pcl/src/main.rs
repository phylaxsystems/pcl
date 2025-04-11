use clap::{command, Parser};
use color_eyre::Result;
use pcl_common::args::CliArgs;
use pcl_core::{
    assertion_da::DaStoreArgs,
    assertion_submission::DappSubmitArgs,
    auth::AuthCommand,
    config::{CliConfig, ConfigArgs},
};
use pcl_phoundry::phorge::PhorgeTest;

const VERSION_MESSAGE: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\nCommit: ",
    env!("VERGEN_GIT_SHA"),
    "\nBuild Timestamp: ",
    env!("VERGEN_BUILD_TIMESTAMP"),
);

#[derive(Parser)]
#[command(
    name = "pcl",
    version = VERSION_MESSAGE,
    about = "The Credible CLI for the Credible Layer"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    #[command(flatten)]
    args: CliArgs,
}

#[derive(clap::Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    #[command(name = "test")]
    Test(PhorgeTest),
    #[command(name = "store")]
    Store(DaStoreArgs),
    #[command(name = "submit")]
    Submit(DappSubmitArgs),
    Auth(AuthCommand),
    #[command(about = "Manage configuration")]
    Config(ConfigArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Configure color_eyre to hide location information and backtrace messages
    color_eyre::config::HookBuilder::default()
        .display_location_section(false)
        .display_env_section(false)
        .install()?;

    let mut config = CliConfig::read_from_file().unwrap_or_default();
    let cli = Cli::parse();

    match cli.command {
        Commands::Test(phorge) => {
            phorge.run().await?;
        }
        Commands::Store(store) => {
            store.run(&cli.args, &mut config).await?;
        }
        Commands::Submit(submit) => {
            submit.run(&cli.args, &mut config).await?;
        }
        Commands::Auth(auth_cmd) => {
            auth_cmd.run(&mut config).await?;
        }
        Commands::Config(config_cmd) => {
            config_cmd.run(&mut config)?;
        }
    };

    config.write_to_file()?;
    Ok(())
}
