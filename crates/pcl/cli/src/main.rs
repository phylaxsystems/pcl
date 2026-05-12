mod cli;

use crate::cli::{
    Cli,
    Commands,
};
use clap::{
    CommandFactory,
    Parser,
    error::ErrorKind,
};
use color_eyre::{
    Result,
    eyre::Report,
};
use pcl_common::args::{
    CliArgs,
    OutputMode,
    current_output_mode,
    set_current_output_mode,
};
#[cfg(feature = "credible")]
use pcl_core::error::VerifyError;
use pcl_core::{
    api::{
        ApiCommandError,
        envelope_output_string,
        with_envelope_metadata,
    },
    config::CliConfig,
    download::DownloadError,
    error::{
        ApplyError,
        AuthError,
        ConfigError,
    },
    surface::ProductSurfaceError,
};
use pcl_phoundry::error::PhoundryError;
use serde_json::{
    Value,
    json,
};
use std::{
    env,
    ffi::{
        OsStr,
        OsString,
    },
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

    if wants_llms_output(env::args_os()) {
        let output_mode = wants_output_mode(env::args_os());
        set_current_output_mode(output_mode);
        pcl_core::surface::print_llms_guide(output_mode == OutputMode::Json)?;
        return Ok(());
    }

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let output_mode = wants_output_mode(env::args_os());
            set_current_output_mode(output_mode);
            let raw_args = env::args_os().collect::<Vec<_>>();
            if output_mode == OutputMode::Human {
                if should_show_root_help(&err, &raw_args) {
                    let mut command = Cli::command();
                    command.print_help()?;
                    println!();
                    std::process::exit(0);
                }
                err.exit();
            }
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) && output_mode != OutputMode::Json
            {
                err.exit();
            }
            if output_mode == OutputMode::Json {
                let exit_code = err.exit_code();
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&clap_error_envelope(&err, &raw_args))?
                );
                std::process::exit(exit_code);
            }
            eprint!(
                "{}",
                envelope_output_string(&clap_error_envelope(&err, &raw_args), false)?
            );
            std::process::exit(err.exit_code());
        }
    };
    set_current_output_mode(cli.args.output_mode());
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
                eprint!("{}", envelope_output_string(&envelope, false)?);
            }
            std::process::exit(1);
        }
    };
    let original_config = config.clone();
    config.normalize_auth_expiry_from_access_token();
    let baseline_config = config.clone();

    let should_write_after_invalid_config = cli.command.should_write_after_invalid_config();
    let should_force_config_write = cli.command.should_force_config_write();
    let result = async {
        run_command(cli.command, &cli.args, &mut config, cli.args.json_output()).await?;
        let command_changed_config = config != baseline_config;
        let passive_config_changed = baseline_config != original_config;
        let should_persist_config = command_changed_config
            || passive_config_changed
            || (!read_valid_config && should_write_after_invalid_config);
        let force_config_write =
            should_force_config_write && (command_changed_config || !read_valid_config);
        if (read_valid_config || should_write_after_invalid_config) && should_persist_config {
            if read_valid_config && !force_config_write {
                config.write_to_file_if_unchanged(&cli.args, &original_config)?;
            } else {
                config.write_to_file(&cli.args)?;
            }
        }
        Ok::<_, Report>(())
    }
    .await;

    if let Err(err) = result {
        let envelope = with_envelope_metadata(error_envelope(&err));
        if cli.args.json_output() {
            eprintln!("{}", serde_json::to_string_pretty(&envelope)?);
        } else {
            eprint!("{}", envelope_output_string(&envelope, false)?);
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
        Commands::Api(api) => api.run(config, cli_args, json_output).await?,
        Commands::Incidents(command) => command.run(config, cli_args, json_output).await?,
        Commands::Projects(command) => command.run(config, cli_args, json_output).await?,
        Commands::Assertions(command) => command.run(config, cli_args, json_output).await?,
        Commands::Search(command) => command.run(config, cli_args, json_output).await?,
        Commands::Account(command) => command.run(config, cli_args, json_output).await?,
        Commands::Contracts(command) => command.run(config, cli_args, json_output).await?,
        Commands::Releases(command) => command.run(config, cli_args, json_output).await?,
        Commands::Deployments(command) => command.run(config, cli_args, json_output).await?,
        Commands::Access(command) => command.run(config, cli_args, json_output).await?,
        Commands::Integrations(command) => command.run(config, cli_args, json_output).await?,
        Commands::ProtocolManager(command) => command.run(config, cli_args, json_output).await?,
        Commands::Transfers(command) => command.run(config, cli_args, json_output).await?,
        Commands::Events(command) => command.run(config, cli_args, json_output).await?,
        Commands::Doctor(command) => command.run(config, cli_args, json_output).await?,
        Commands::Whoami(command) => command.run(config, json_output)?,
        Commands::Workflows(command) => command.run(json_output)?,
        Commands::Export(command) => command.run(config, cli_args, json_output).await?,
        Commands::Artifacts(command) => command.run(cli_args, json_output)?,
        Commands::Requests(command) => command.run_with_cli_args(cli_args, json_output)?,
        Commands::Schema(command) => command.run(json_output)?,
        Commands::Llms(command) => command.run(json_output)?,
        Commands::Jobs(command) => command.run(cli_args, json_output)?,
        Commands::Completions(command) => command.run(json_output)?,
        Commands::Auth(auth_cmd) => auth_cmd.run(config, cli_args, json_output).await?,
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
    if let Some(apply_error) = err.downcast_ref::<ApplyError>() {
        return with_envelope_metadata(apply_error_envelope(apply_error));
    }
    if let Some(auth_error) = err.downcast_ref::<AuthError>() {
        return with_envelope_metadata(auth_error_envelope(auth_error));
    }
    if let Some(config_error) = err.downcast_ref::<ConfigError>() {
        return with_envelope_metadata(config_error_envelope(config_error));
    }
    if let Some(download_error) = err.downcast_ref::<DownloadError>() {
        return with_envelope_metadata(download_error_envelope(download_error));
    }
    #[cfg(feature = "credible")]
    if let Some(verify_error) = err.downcast_ref::<VerifyError>() {
        return with_envelope_metadata(verify_error_envelope(verify_error));
    }
    if let Some(surface_error) = err.downcast_ref::<ProductSurfaceError>() {
        return surface_error.json_envelope();
    }
    if let Some(phoundry_error) = err.downcast_ref::<PhoundryError>() {
        return with_envelope_metadata(phoundry_error_envelope(phoundry_error));
    }
    if let Some(phoundry_error) = err.downcast_ref::<Box<PhoundryError>>() {
        return with_envelope_metadata(phoundry_error_envelope(phoundry_error));
    }

    with_envelope_metadata(simple_error_value("unknown", &err.to_string(), false, &[]))
}

fn simple_error_value(
    code: &str,
    message: &str,
    recoverable: bool,
    next_actions: &[&str],
) -> Value {
    json!({
        "status": "error",
        "error": {
            "code": code,
            "message": message,
            "recoverable": recoverable,
        },
        "next_actions": next_actions,
    })
}

fn apply_error_envelope(err: &ApplyError) -> Value {
    let (code, message, next_actions): (&str, String, &[&str]) = match err {
        ApplyError::NoAuthToken => {
            (
                "auth.no_token",
                err.to_string(),
                &["pcl auth login", "pcl auth status"],
            )
        }
        ApplyError::InvalidConfig(message) if message.contains("credible.toml not found") => {
            (
                "config.credible_toml_not_found",
                "No credible.toml found. Run from an assertion project or pass --config <path>."
                    .to_string(),
                &["pcl apply --help", "pcl projects --mine"],
            )
        }
        ApplyError::InvalidConfig(_) | ApplyError::Toml(_) => {
            (
                "config.invalid_credible_toml",
                err.to_string(),
                &["pcl apply --help"],
            )
        }
        ApplyError::BuildFailed(_) => {
            (
                "build.failed",
                err.to_string(),
                &["pcl build", "pcl apply --dry-run"],
            )
        }
        ApplyError::NoProjectsFound => {
            (
                "projects.none_for_account",
                err.to_string(),
                &["pcl projects --mine", "pcl account"],
            )
        }
        ApplyError::ApplyCancelled => ("apply.cancelled", err.to_string(), &["pcl apply --help"]),
        _ => {
            (
                "apply.failed",
                err.to_string(),
                &["pcl apply --help", "pcl doctor"],
            )
        }
    };
    simple_error_value(code, &message, true, next_actions)
}

fn download_error_envelope(err: &DownloadError) -> Value {
    let (code, message, next_actions): (&str, String, &[&str]) = match err {
        DownloadError::NoAuthToken => {
            (
                "auth.no_token",
                err.to_string(),
                &["pcl auth login", "pcl auth status"],
            )
        }
        DownloadError::MissingIdentifier => {
            (
                "download.missing_project_id",
                "--project-id is required".to_string(),
                &[
                    "pcl projects --mine",
                    "pcl download --project-id <project-id>",
                ],
            )
        }
        DownloadError::NoAssertionsFound => {
            (
                "download.no_assertions",
                err.to_string(),
                &["pcl assertions --project-id <project-id>"],
            )
        }
        _ => {
            (
                "download.failed",
                err.to_string(),
                &["pcl download --help", "pcl doctor"],
            )
        }
    };
    simple_error_value(code, &message, true, next_actions)
}

#[cfg(feature = "credible")]
fn verify_error_envelope(err: &VerifyError) -> Value {
    let (code, message, next_actions): (&str, String, &[&str]) = match err {
        VerifyError::Io { message, .. } if message.starts_with("Project root not found") => {
            (
                "verify.project_root_not_found",
                err.to_string(),
                &["pcl verify --help", "Check --root path"],
            )
        }
        VerifyError::Io { .. } => {
            (
                "verify.io_failed",
                err.to_string(),
                &["pcl verify --help", "Check file paths and permissions"],
            )
        }
        VerifyError::Config(_) => {
            (
                "verify.invalid_config",
                err.to_string(),
                &["pcl verify --help", "pcl apply --dry-run"],
            )
        }
        VerifyError::BuildFailed(_) => {
            (
                "verify.build_failed",
                err.to_string(),
                &["pcl build --help", "pcl verify --help"],
            )
        }
        VerifyError::AbiEncode(_) => {
            (
                "verify.invalid_constructor_args",
                err.to_string(),
                &["pcl verify --help"],
            )
        }
        VerifyError::Json(_) => {
            (
                "json.failed",
                err.to_string(),
                &["Retry without --json to inspect human output"],
            )
        }
    };
    simple_error_value(code, &message, true, next_actions)
}

fn phoundry_error_envelope(err: &PhoundryError) -> Value {
    let (code, message, next_actions): (&str, String, &[&str]) = match err {
        PhoundryError::DirectoryNotFound(path) => {
            (
                "build.source_dir_not_found",
                format!("Source directory not found: {}", path.display()),
                &["pcl build --help", "pcl apply --help"],
            )
        }
        PhoundryError::ForgeNotInstalled => {
            (
                "build.forge_not_installed",
                err.to_string(),
                &["Install Foundry forge", "pcl doctor"],
            )
        }
        _ => {
            (
                "build.failed",
                err.to_string(),
                &["pcl build --help", "pcl doctor"],
            )
        }
    };
    simple_error_value(code, &message, true, next_actions)
}

fn auth_error_envelope(err: &AuthError) -> Value {
    if let Some(envelope) = auth_refresh_error_envelope(err) {
        return envelope;
    }

    match err {
        AuthError::StoredTokenExpired {
            user,
            expires_at,
            platform_url,
        } => {
            let seconds_remaining = expires_at.timestamp() - unix_timestamp_now();
            let refresh_command = command_for_current_output("pcl auth refresh");
            let login_command = command_for_current_output("pcl auth login --force");
            let logout_command = command_for_current_output("pcl auth logout");
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
                    refresh_command,
                    login_command,
                    logout_command,
                ],
            }))
        }
        AuthError::SessionExpired | AuthError::SessionNotFound | AuthError::InvalidSession(_) => {
            with_envelope_metadata(simple_error_value(
                "auth.session_invalid",
                &err.to_string(),
                true,
                &["pcl auth login"],
            ))
        }
        AuthError::UserNotFound => {
            with_envelope_metadata(simple_error_value(
                "auth.user_not_found",
                &err.to_string(),
                true,
                &["pcl auth login"],
            ))
        }
        AuthError::AuthRequestFailed(_)
        | AuthError::StatusRequestFailed(_)
        | AuthError::ServerError(_)
        | AuthError::Timeout(_)
        | AuthError::InvalidAuthData(_)
        | AuthError::ConfigError(_)
        | AuthError::NoRefreshableSession
        | AuthError::MissingRefreshToken
        | AuthError::RefreshRejected { .. }
        | AuthError::RefreshEndpointNotFound { .. }
        | AuthError::RefreshRateLimited { .. }
        | AuthError::RefreshServerError { .. }
        | AuthError::RefreshRequestFailed(_)
        | AuthError::RefreshLockTimeout => {
            with_envelope_metadata(simple_error_value(
                "auth.request_failed",
                &err.to_string(),
                true,
                &["pcl auth login"],
            ))
        }
    }
}

fn auth_refresh_error_envelope(err: &AuthError) -> Option<Value> {
    match err {
        AuthError::NoRefreshableSession | AuthError::MissingRefreshToken => {
            Some(with_envelope_metadata(simple_error_value(
                "auth.refresh_unavailable",
                &err.to_string(),
                true,
                &["pcl auth login --force"],
            )))
        }
        AuthError::RefreshRejected {
            status,
            code,
            request_id,
            message,
        } => {
            Some(with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.invalid_refresh_token",
                    "message": err.to_string(),
                    "recoverable": true,
                    "http": {
                        "status": status,
                        "request_id": request_id,
                    },
                    "platform_code": code,
                    "request_id": request_id,
                    "details": message,
                },
                "next_actions": ["pcl auth login --force"],
            })))
        }
        AuthError::RefreshEndpointNotFound {
            request_id,
            message,
        } => {
            Some(auth_refresh_endpoint_not_found_envelope(
                err,
                request_id.as_ref(),
                message.as_ref(),
            ))
        }
        AuthError::RefreshRateLimited {
            retry_after_seconds,
            request_id,
            message,
        } => {
            let refresh_command = command_for_current_output("pcl auth refresh");
            Some(with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.refresh_rate_limited",
                    "message": err.to_string(),
                    "recoverable": true,
                    "retry_after_seconds": retry_after_seconds,
                    "request_id": request_id,
                    "details": message,
                },
                "next_actions": [format!("Wait for error.retry_after_seconds, then retry {refresh_command}")],
            })))
        }
        AuthError::RefreshServerError {
            status,
            request_id,
            message,
        } => {
            let refresh_command = command_for_current_output("pcl auth refresh");
            Some(with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.refresh_server_error",
                    "message": err.to_string(),
                    "recoverable": true,
                    "http": {
                        "status": status,
                        "request_id": request_id,
                    },
                    "request_id": request_id,
                    "details": message,
                },
                "next_actions": [format!("Retry {refresh_command} once before logging in again")],
            })))
        }
        AuthError::RefreshRequestFailed(_) | AuthError::RefreshLockTimeout => {
            Some(auth_refresh_failed_envelope(err))
        }
        _ => None,
    }
}

fn auth_refresh_failed_envelope(err: &AuthError) -> Value {
    let refresh_command = command_for_current_output("pcl auth refresh");
    let login_command = command_for_current_output("pcl auth login --force");
    with_envelope_metadata(json!({
        "status": "error",
        "error": {
            "code": "auth.refresh_failed",
            "message": err.to_string(),
            "recoverable": true,
        },
        "next_actions": [
            format!("Retry {refresh_command}"),
            login_command,
        ],
    }))
}

fn command_for_current_output(command: &str) -> String {
    match current_output_mode() {
        OutputMode::Human => command.to_string(),
        OutputMode::Toon => format!("{command} --toon"),
        OutputMode::Json => format!("{command} --json"),
    }
}

fn auth_refresh_endpoint_not_found_envelope(
    err: &AuthError,
    request_id: Option<&String>,
    message: Option<&String>,
) -> Value {
    with_envelope_metadata(json!({
        "status": "error",
        "error": {
            "code": "auth.refresh_unavailable",
            "message": err.to_string(),
            "recoverable": true,
            "http": {
                "status": 404,
                "request_id": request_id,
            },
            "platform_code": "REFRESH_ENDPOINT_NOT_FOUND",
            "request_id": request_id,
            "details": message,
        },
        "next_actions": ["pcl auth login --force"],
    }))
}

fn unix_timestamp_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn config_error_envelope(err: &ConfigError) -> Value {
    simple_error_value(
        config_error_code(err),
        &err.to_string(),
        !matches!(err, ConfigError::ParseError(_) | ConfigError::JsonError(_)),
        &["pcl config show", "pcl config delete"],
    )
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

fn wants_output_mode<I, S>(args: I) -> OutputMode
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut saw_json = false;
    let mut saw_toon = false;
    let mut saw_format_flag = false;

    for arg in args {
        let arg = arg.as_ref();
        if saw_format_flag {
            saw_format_flag = false;
            match arg.to_str() {
                Some("json") => saw_json = true,
                Some("toon") => saw_toon = true,
                _ => {}
            }
            continue;
        }
        if arg == OsStr::new("--json") || arg == OsStr::new("-j") {
            saw_json = true;
        } else if arg == OsStr::new("--toon") {
            saw_toon = true;
        } else if arg == OsStr::new("--format") {
            saw_format_flag = true;
        } else if let Some(value) = arg.to_str().and_then(|arg| arg.strip_prefix("--format=")) {
            match value {
                "json" => saw_json = true,
                "toon" => saw_toon = true,
                _ => {}
            }
        }
    }

    if saw_json {
        OutputMode::Json
    } else if saw_toon {
        OutputMode::Toon
    } else {
        OutputMode::Human
    }
}

fn wants_llms_output<I, S>(args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .any(|arg| arg.as_ref() == OsStr::new("--llms"))
}

fn should_show_root_help(err: &clap::Error, args: &[OsString]) -> bool {
    matches!(
        err.kind(),
        ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) && parsed_command_name(args).is_none()
}

fn clap_error_envelope(err: &clap::Error, args: &[OsString]) -> Value {
    let command = parsed_command_name(args);
    let message = clap_error_message(err, command.as_deref());
    let next_actions = clap_error_next_actions(err.kind(), command.as_deref());
    with_envelope_metadata(json!({
        "status": "error",
        "error": {
            "code": clap_error_code(err.kind()),
            "message": message,
            "recoverable": !matches!(err.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion),
        },
        "next_actions": next_actions,
    }))
}

fn clap_error_message(err: &clap::Error, command: Option<&str>) -> String {
    if matches!(
        err.kind(),
        ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) && let Some(command) = command
    {
        return format!("Choose a subcommand for `pcl {command}`.");
    }
    err.to_string()
}

fn clap_error_next_actions(kind: ErrorKind, command: Option<&str>) -> Vec<String> {
    if let Some(command) = command {
        if kind == ErrorKind::InvalidSubcommand && !is_known_top_level_command(command) {
            return vec!["pcl --help".to_string(), "pcl workflows".to_string()];
        }
        let mut actions = Vec::new();
        match (kind, command) {
            (ErrorKind::InvalidSubcommand, "schema") => {
                actions.push("pcl schema list".to_string());
                actions.push("pcl schema get projects".to_string());
            }
            (ErrorKind::InvalidSubcommand, "workflows") => {
                actions.push("pcl workflows list".to_string());
                actions.push("pcl workflows show incident-investigation".to_string());
            }
            (ErrorKind::MissingRequiredArgument, "completions") => {
                actions.push("pcl completions bash".to_string());
                actions.push("pcl completions zsh".to_string());
            }
            (
                ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
                "api",
            ) => {
                actions.push("pcl api manifest".to_string());
                actions.push("pcl api list".to_string());
            }
            (
                ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
                "auth",
            ) => {
                actions.push("pcl auth status".to_string());
                actions.push("pcl auth login".to_string());
            }
            (
                ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
                "config",
            ) => {
                actions.push("pcl config show".to_string());
                actions.push("pcl doctor".to_string());
            }
            (
                ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand,
                "export",
            ) => {
                actions.push("pcl export incidents --help".to_string());
                actions.push("pcl jobs list".to_string());
            }
            _ => {}
        }
        actions.push(format!("pcl {command} --help"));
        actions.push("pcl --help".to_string());
        return actions;
    }
    vec!["pcl --help".to_string(), "pcl api manifest".to_string()]
}

fn is_known_top_level_command(command: &str) -> bool {
    matches!(
        command,
        "apply"
            | "api"
            | "incidents"
            | "projects"
            | "assertions"
            | "search"
            | "account"
            | "contracts"
            | "releases"
            | "deployments"
            | "access"
            | "integrations"
            | "protocol-manager"
            | "transfers"
            | "events"
            | "doctor"
            | "whoami"
            | "workflows"
            | "export"
            | "artifacts"
            | "requests"
            | "logs"
            | "schema"
            | "llms"
            | "jobs"
            | "completions"
            | "auth"
            | "config"
            | "build"
            | "download"
            | "help"
    )
}

fn parsed_command_name(args: &[OsString]) -> Option<String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--json" | "-j" | "--toon" | "--llms" | "--help" | "-h" | "--version" | "-V" => {}
            "--config-dir" | "--format" => {
                let _ = iter.next();
            }
            _ if value.starts_with("--config-dir=") || value.starts_with("--format=") => {}
            _ if value.starts_with('-') => {}
            _ => return Some(value.into_owned()),
        }
    }
    None
}

fn clap_error_code(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::ArgumentConflict => "cli.argument_conflict",
        ErrorKind::UnknownArgument => "cli.unknown_argument",
        ErrorKind::InvalidValue => "cli.invalid_value",
        ErrorKind::InvalidSubcommand => "cli.invalid_subcommand",
        ErrorKind::MissingRequiredArgument => "cli.missing_required_argument",
        ErrorKind::MissingSubcommand | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            "cli.missing_subcommand"
        }
        ErrorKind::DisplayHelp => "cli.help",
        ErrorKind::DisplayVersion => "cli.version",
        _ => "cli.parse_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use pcl_core::api::toon_string;

    #[test]
    fn detects_output_mode_before_successful_parse() {
        assert_eq!(
            wants_output_mode(["pcl", "--json", "api"]),
            OutputMode::Json
        );
        assert_eq!(
            wants_output_mode(["pcl", "api", "projects", "-j"]),
            OutputMode::Json
        );
        assert_eq!(
            wants_output_mode(["pcl", "--format", "json", "api"]),
            OutputMode::Json
        );
        assert_eq!(
            wants_output_mode(["pcl", "--format=json", "api"]),
            OutputMode::Json
        );
        assert_eq!(
            wants_output_mode(["pcl", "--toon", "api"]),
            OutputMode::Toon
        );
        assert_eq!(
            wants_output_mode(["pcl", "--format", "toon", "api"]),
            OutputMode::Toon
        );
        assert_eq!(wants_output_mode(["pcl", "api"]), OutputMode::Human);
    }

    #[test]
    fn detects_llms_flag_before_successful_parse() {
        assert!(wants_llms_output(["pcl", "--llms"]));
        assert!(wants_llms_output(["pcl", "--json", "--llms"]));
        assert!(!wants_llms_output(["pcl", "llms"]));
    }

    #[test]
    fn classifies_missing_subcommand_help_as_missing_subcommand() {
        let err = Cli::command().try_get_matches_from(["pcl"]).unwrap_err();
        assert_eq!(clap_error_code(err.kind()), "cli.missing_subcommand");
    }

    #[test]
    fn wraps_clap_conflicts_as_json_errors() {
        let err = Cli::command()
            .try_get_matches_from(["pcl", "--json", "api", "projects", "--save", "--unsave"])
            .unwrap_err();
        let args = vec![
            OsString::from("pcl"),
            OsString::from("--json"),
            OsString::from("api"),
            OsString::from("projects"),
            OsString::from("--save"),
            OsString::from("--unsave"),
        ];
        let envelope = clap_error_envelope(&err, &args);

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
        let args = vec![
            OsString::from("pcl"),
            OsString::from("api"),
            OsString::from("projects"),
            OsString::from("--save"),
            OsString::from("--unsave"),
        ];
        let output = toon_string(&clap_error_envelope(&err, &args));

        assert!(output.contains("status: error"));
        assert!(output.contains("code: cli.argument_conflict"));
        assert!(output.contains("message:"));
        assert!(output.contains("Usage: pcl api projects --save"));
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
        assert_eq!(envelope["next_actions"][0], "pcl auth refresh");
        assert_eq!(envelope["next_actions"][1], "pcl auth login --force");
    }
}
