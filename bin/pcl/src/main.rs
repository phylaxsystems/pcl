use clap::{command, Parser};
use eyre::{Context, Result};
use pcl_common::args::CliArgs;
use pcl_core::{
    assertion_da::DASubmitArgs, assertion_submission::DappSubmitArgs, config::CliConfig,
};
use pcl_phoundry::{build::BuildArgs, phorge::Phorge};

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
enum Commands {
    Phorge(Phorge),
    Build(BuildArgs),
    DASubmit(DASubmitArgs),
    DappSubmit(DappSubmitArgs),
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if forge is installed
    Phorge::forge_must_be_installed()?;
    let mut config = CliConfig::read_or_default();

    let cli = Cli::parse();
    match cli.command {
        Commands::Phorge(phorge) => {
            phorge.run(cli.args.clone(), true)?;
        }
        Commands::Build(build) => {
            build.run(cli.args.clone())?;
        }
        Commands::DASubmit(submit) => {
            config.must_be_authenticated().wrap_err("Authentication required for DA submission. Please authenticate first using 'pcl auth'")?;
            submit.run(cli.args.clone(), &mut config).await?;
        }
        Commands::DappSubmit(submit) => {
            config.must_be_authenticated().wrap_err("Authentication required for dapp submission. Please authenticate first using 'pcl auth'")?;
            submit.run(cli.args.clone(), &mut config).await?;
        }
    };
    config.write_to_file()?;
    Ok(())
}

async fn handle_auth_command(cmd: AuthCommand) -> Result<()> {
    match cmd.command {
        AuthSubcommands::Login => auth::login().await?,
        AuthSubcommands::Logout => auth::logout()?,
        AuthSubcommands::Status => auth::status()?,
    }
    Ok(())
}
