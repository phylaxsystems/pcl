mod cli;

use crate::cli::{
    Cli,
    Commands,
};
use clap::{
    Parser,
    error::ErrorKind,
};
use color_eyre::{
    Result,
    eyre::Report,
};
use pcl_core::{
    api::ApiCommandError,
    config::CliConfig,
};
use serde_json::{
    Value,
    json,
};
use std::{
    env,
    ffi::OsStr,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Configure color_eyre to hide location information and backtrace messages
    color_eyre::config::HookBuilder::default()
        .display_location_section(true)
        .display_env_section(false)
        .install()?;

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            if wants_json_output(env::args_os()) {
                let exit_code = err.exit_code();
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&clap_error_envelope(&err))?
                );
                std::process::exit(exit_code);
            }
            err.exit();
        }
    };
    let mut config = CliConfig::read_from_file(&cli.args).unwrap_or_default();

    // TODO(Odysseas): Convert these commands to return strings to print for json output
    // We can also use something similar like the shell macro from Foundry
    // where a global static lazy is used to signal to every print statement
    // whether it should be a noop or print to stdout/stderr.

    let result = async {
        match cli.command {
            #[cfg(feature = "credible")]
            Commands::Test(phorge) => {
                phorge.run().await?;
            }
            Commands::Apply(apply) => {
                apply.run(&cli.args, &config).await?;
            }
            Commands::Api(api) => {
                api.run(&config, cli.args.json_output()).await?;
            }
            Commands::Auth(auth_cmd) => {
                auth_cmd.run(&mut config).await?;
            }
            Commands::Config(config_cmd) => {
                config_cmd.run(&mut config)?;
            }
            Commands::Build(build_cmd) => {
                build_cmd.run()?;
            }
            #[cfg(feature = "credible")]
            Commands::Verify(verify_cmd) => {
                verify_cmd.run(&cli.args)?;
            }
            Commands::Download(download_cmd) => {
                download_cmd.run(&cli.args, &config).await?;
            }
        }
        config.write_to_file(&cli.args)?;
        Ok::<_, Report>(())
    }
    .await;

    if let Err(err) = result {
        if cli.args.json_output() {
            eprintln!("{}", serde_json::to_string_pretty(&error_envelope(&err))?);
            std::process::exit(1);
        } else {
            return Err(err);
        }
    }

    Ok(())
}

fn error_envelope(err: &Report) -> Value {
    if let Some(api_error) = err.downcast_ref::<ApiCommandError>() {
        return api_error.json_envelope();
    }

    json!({
        "status": "error",
        "error": {
            "code": "unknown",
            "message": err.to_string(),
            "recoverable": false,
        },
        "next_actions": [],
    })
}

fn wants_json_output<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter().any(|arg| {
        let arg = arg.as_ref();
        arg == OsStr::new("--json") || arg == OsStr::new("-j")
    })
}

fn clap_error_envelope(err: &clap::Error) -> Value {
    json!({
        "status": "error",
        "error": {
            "code": clap_error_code(err.kind()),
            "message": err.to_string(),
            "recoverable": !matches!(err.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion),
        },
        "next_actions": [
            "pcl --help",
            "pcl api manifest --json"
        ],
    })
}

fn clap_error_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::ArgumentConflict => "cli.argument_conflict",
        ErrorKind::UnknownArgument => "cli.unknown_argument",
        ErrorKind::InvalidValue => "cli.invalid_value",
        ErrorKind::InvalidSubcommand => "cli.invalid_subcommand",
        ErrorKind::MissingRequiredArgument => "cli.missing_required_argument",
        ErrorKind::MissingSubcommand => "cli.missing_subcommand",
        ErrorKind::DisplayHelp => "cli.help",
        ErrorKind::DisplayVersion => "cli.version",
        _ => "cli.parse_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn detects_json_flag_before_successful_parse() {
        assert!(wants_json_output(["pcl", "--json", "api"]));
        assert!(wants_json_output(["pcl", "api", "projects", "-j"]));
        assert!(!wants_json_output(["pcl", "api", "projects"]));
    }

    #[test]
    fn wraps_clap_conflicts_as_json_errors() {
        let err = Cli::command()
            .try_get_matches_from(["pcl", "--json", "api", "projects", "--save", "--unsave"])
            .unwrap_err();
        let envelope = clap_error_envelope(&err);

        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["error"]["code"], "cli.argument_conflict");
        assert_eq!(envelope["error"]["recoverable"], true);
        assert!(envelope["next_actions"].as_array().unwrap().len() >= 2);
    }
}
