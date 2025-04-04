use clap::{command, Parser};
use eyre::Result;
use pcl_common::args::CliArgs;
use pcl_core::{
    assertion_da::DASubmitArgs, assertion_submission::DappSubmitArgs, auth::AuthCommand,
    config::CliConfig,
};
use pcl_phoundry::phorge::Phorge;

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
    #[command(name = "store")]
    DASubmit(DASubmitArgs),
    #[command(name = "submit")]
    DappSubmit(DappSubmitArgs),
    Auth(AuthCommand),
    #[command(about = "Display the current configuration")]
    Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Check if forge is installed
    Phorge::forge_must_be_installed()?;
    let mut config = CliConfig::read_from_file().unwrap_or_default();

    let cli = Cli::parse();
    match cli.command {
        Commands::Phorge(phorge) => {
            phorge.run(&cli.args, true)?;
        }
        Commands::DASubmit(submit) => {
            submit.run(&cli.args, &mut config).await?;
        }
        Commands::DappSubmit(submit) => {
            submit.run(&cli.args, &mut config).await?;
        }
        Commands::Auth(auth_cmd) => {
            auth_cmd.run(&mut config).await?;
        }
        Commands::Config => {
            println!("{}", config);
        }
    };
    config.write_to_file()?;
    Ok(())
}

//TODO(GREG): Add integration tests that run cli with all the commands and confirm the output is as
//expected. 
//This serves the purpose of forced testing of cli args and output testing. For example
//conflicting short args can fall through CI without tests like this. 
//Consider adding unit tests with dapp and da mocks for a quicker 0-1 than running
//the dapp in CI.
