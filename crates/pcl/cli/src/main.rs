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
use pcl_common::args::CliArgs;
use pcl_core::{
    api::{
        ApiCommandError,
        toon_string,
        with_envelope_metadata,
    },
    config::CliConfig,
    error::{
        AuthError,
        ConfigError,
    },
    surface::ProductSurfaceError,
};
use serde_json::{
    Value,
    json,
};
use std::{
    env,
    ffi::OsStr,
    time::{
        SystemTime,
        UNIX_EPOCH,
    },
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
                let envelope = with_envelope_metadata(clap_error_envelope(&err));
                eprintln!("{}", serde_json::to_string_pretty(&envelope)?);
                std::process::exit(exit_code);
            }
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                err.exit();
            }
            eprint!(
                "{}",
                toon_string(&with_envelope_metadata(clap_error_envelope(&err)))
            );
            std::process::exit(err.exit_code());
        }
    };
    let mut read_valid_config = true;
    let mut config = match CliConfig::read_from_file(&cli.args) {
        Ok(config) => config,
        Err(err) if cli.command.can_run_without_valid_config() => {
            read_valid_config = false;
            CliConfig::default()
        }
        Err(err) => {
            let envelope = with_envelope_metadata(config_error_envelope(&err));
            if cli.args.json_output() {
                eprintln!("{}", serde_json::to_string_pretty(&envelope)?);
            } else {
                eprint!("{}", toon_string(&envelope));
            }
            std::process::exit(1);
        }
    };

    // TODO(Odysseas): Convert these commands to return strings to print for json output
    // We can also use something similar like the shell macro from Foundry
    // where a global static lazy is used to signal to every print statement
    // whether it should be a noop or print to stdout/stderr.

    let should_write_after_invalid_config = cli.command.should_write_after_invalid_config();
    let result = async {
        run_command(cli.command, &cli.args, &mut config, cli.args.json_output()).await?;
        if read_valid_config || should_write_after_invalid_config {
            config.write_to_file(&cli.args)?;
        }
        Ok::<_, Report>(())
    }
    .await;

    if let Err(err) = result {
        let envelope = with_envelope_metadata(error_envelope(&err));
        if cli.args.json_output() {
            eprintln!("{}", serde_json::to_string_pretty(&envelope)?);
        } else {
            eprint!("{}", toon_string(&envelope));
        }
        std::process::exit(1);
    }

    Ok(())
}

async fn run_command(
    command: Commands,
    cli_args: &CliArgs,
    config: &mut CliConfig,
    json_output: bool,
) -> Result<(), Report> {
    match command {
        #[cfg(feature = "credible")]
        Commands::Test(phorge) => phorge.run().await?,
        Commands::Apply(apply) => apply.run(cli_args, config).await?,
        Commands::Api(api) => api.run(config, json_output).await?,
        Commands::Incidents(command) => command.run(config, json_output).await?,
        Commands::Projects(command) => command.run(config, json_output).await?,
        Commands::Assertions(command) => command.run(config, json_output).await?,
        Commands::Search(command) => command.run(config, json_output).await?,
        Commands::Account(command) => command.run(config, json_output).await?,
        Commands::Contracts(command) => command.run(config, json_output).await?,
        Commands::Releases(command) => command.run(config, json_output).await?,
        Commands::Deployments(command) => command.run(config, json_output).await?,
        Commands::Access(command) => command.run(config, json_output).await?,
        Commands::Integrations(command) => command.run(config, json_output).await?,
        Commands::ProtocolManager(command) => command.run(config, json_output).await?,
        Commands::Transfers(command) => command.run(config, json_output).await?,
        Commands::Events(command) => command.run(config, json_output).await?,
        Commands::Doctor(command) => command.run(config, cli_args, json_output).await?,
        Commands::Whoami(command) => command.run(config, json_output)?,
        Commands::Workflows(command) => command.run(json_output)?,
        Commands::Export(command) => command.run(config, cli_args, json_output).await?,
        Commands::Artifacts(command) => command.run(cli_args, json_output)?,
        Commands::Requests(command) => command.run(json_output)?,
        Commands::Schema(command) => command.run(json_output)?,
        Commands::Auth(auth_cmd) => auth_cmd.run(config, json_output).await?,
        Commands::Config(config_cmd) => config_cmd.run(config, cli_args)?,
        Commands::Build(build_cmd) => build_cmd.run()?,
        #[cfg(feature = "credible")]
        Commands::Verify(verify_cmd) => verify_cmd.run(cli_args)?,
        Commands::Download(download_cmd) => download_cmd.run(cli_args, config).await?,
    }
    Ok(())
}

fn error_envelope(err: &Report) -> Value {
    if let Some(api_error) = err.downcast_ref::<ApiCommandError>() {
        return api_error.json_envelope();
    }
    if let Some(auth_error) = err.downcast_ref::<AuthError>() {
        return with_envelope_metadata(auth_error_envelope(auth_error));
    }
    if let Some(config_error) = err.downcast_ref::<ConfigError>() {
        return with_envelope_metadata(config_error_envelope(config_error));
    }
    if let Some(surface_error) = err.downcast_ref::<ProductSurfaceError>() {
        return surface_error.json_envelope();
    }

    with_envelope_metadata(json!({
        "status": "error",
        "error": {
            "code": "unknown",
            "message": err.to_string(),
            "recoverable": false,
        },
        "next_actions": [],
    }))
}

fn auth_error_envelope(err: &AuthError) -> Value {
    match err {
        AuthError::StoredTokenExpired {
            user,
            expires_at,
            platform_url,
        } => {
            let seconds_remaining = expires_at.timestamp() - unix_timestamp_now();
            with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.expired_token",
                    "message": err.to_string(),
                    "recoverable": true,
                    "auth": {
                        "authenticated": true,
                        "user": user,
                        "token_valid": false,
                        "token_expired": true,
                        "expired": true,
                        "expires_at": expires_at.to_rfc3339(),
                        "seconds_remaining": seconds_remaining,
                        "expires_in_seconds": seconds_remaining,
                        "platform_url": platform_url,
                    },
                },
                "next_actions": [
                    "pcl auth login",
                    "pcl auth logout",
                ],
            }))
        }
        AuthError::SessionExpired | AuthError::SessionNotFound | AuthError::InvalidSession(_) => {
            with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.session_invalid",
                    "message": err.to_string(),
                    "recoverable": true,
                },
                "next_actions": ["pcl auth login"],
            }))
        }
        AuthError::UserNotFound => {
            with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.user_not_found",
                    "message": err.to_string(),
                    "recoverable": true,
                },
                "next_actions": ["pcl auth login"],
            }))
        }
        AuthError::AuthRequestFailed(_)
        | AuthError::StatusRequestFailed(_)
        | AuthError::ServerError(_)
        | AuthError::Timeout(_)
        | AuthError::InvalidAuthData(_)
        | AuthError::ConfigError(_) => {
            with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.request_failed",
                    "message": err.to_string(),
                    "recoverable": true,
                },
                "next_actions": ["pcl auth login"],
            }))
        }
    }
}

fn unix_timestamp_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn config_error_envelope(err: &ConfigError) -> Value {
    with_envelope_metadata(json!({
        "status": "error",
        "error": {
            "code": config_error_code(err),
            "message": err.to_string(),
            "recoverable": !matches!(err, ConfigError::ParseError(_) | ConfigError::JsonError(_)),
        },
        "next_actions": [
            "pcl config show",
            "pcl config delete",
        ],
    }))
}

fn config_error_code(err: &ConfigError) -> &'static str {
    match err {
        ConfigError::ReadError(_) => "config.read_failed",
        ConfigError::WriteError(_) => "config.write_failed",
        ConfigError::ParseError(_) => "config.parse_failed",
        ConfigError::SerializeError(_) => "config.serialize_failed",
        ConfigError::JsonError(_) => "config.json_failed",
        ConfigError::NotAuthenticated => "config.not_authenticated",
    }
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
    with_envelope_metadata(json!({
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
    }))
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
        assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
        assert_eq!(envelope["pcl_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(envelope["error"]["code"], "cli.argument_conflict");
        assert_eq!(envelope["error"]["recoverable"], true);
        assert!(envelope["next_actions"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn wraps_clap_conflicts_as_toon_errors() {
        let err = Cli::command()
            .try_get_matches_from(["pcl", "api", "projects", "--save", "--unsave"])
            .unwrap_err();
        let output = toon_string(&clap_error_envelope(&err));

        assert!(output.contains("status: error"));
        assert!(output.contains("code: cli.argument_conflict"));
        assert!(output.contains("message: |"));
        assert!(output.contains("  Usage: pcl api projects --save"));
        assert!(output.contains("recoverable: true"));
        assert!(output.contains("next_actions[2]:"));
        assert!(!output.contains("Location:"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn wraps_runtime_errors_as_toon_errors() {
        let err = Report::new(ApiCommandError::NoAuthToken);
        let output = toon_string(&error_envelope(&err));

        assert!(output.contains("status: error"));
        assert!(output.contains("code: auth.no_token"));
        assert!(output.contains("recoverable: true"));
        assert!(output.contains("pcl auth login"));
        assert!(!output.contains("Location:"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn wraps_auth_errors_as_structured_errors() {
        let expires_at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .to_utc();
        let err = Report::new(AuthError::StoredTokenExpired {
            user: "user-1".to_string(),
            expires_at,
            platform_url: "https://app.phylax.systems/".to_string(),
        });
        let envelope = error_envelope(&err);

        assert_eq!(envelope["status"], "error");
        assert_eq!(envelope["error"]["code"], "auth.expired_token");
        assert_eq!(envelope["error"]["auth"]["token_valid"], false);
        assert_eq!(envelope["next_actions"][0], "pcl auth login");
    }
}
