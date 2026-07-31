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
        DeployError,
        PlatformError,
    },
    output::command_for_mode,
    platform::{
        Interaction,
        Url,
        describe_platform,
        resolve_for_invocation,
        set_active_platform,
    },
    surface::ProductSurfaceError,
};
#[cfg(feature = "credible")]
use pcl_core::{
    error::VerifyError,
    verify::VerificationSummary,
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
            let is_success_display = matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            );
            let exit_code = err.exit_code();
            let envelope = clap_error_envelope(&err, &raw_args);
            let output = envelope_output_string(&envelope, output_mode == OutputMode::Json)?;
            if is_success_display {
                print!("{output}");
            } else {
                eprint!("{output}");
            }
            std::process::exit(exit_code);
        }
    };
    set_current_output_mode(cli.args.output_mode());
    let mut read_valid_config = true;
    let mut config = match CliConfig::read_from_file(&cli.args) {
        Ok(config) => config,
        Err(_err) if cli.command.can_run_without_valid_config() => {
            read_valid_config = false;
            CliConfig::default()
        }
        Err(err) => {
            let envelope = with_envelope_metadata(config_error_envelope(&err));
            eprint!("{}", envelope_output_string(&envelope, false)?);
            std::process::exit(1);
        }
    };
    let mut original_config = config.clone();
    config.normalize_auth_expiry_from_access_token();

    let should_write_after_invalid_config = cli.command.should_write_after_invalid_config();
    if establish_active_platform(&cli.command, &cli.args, &mut config)?
        && (read_valid_config || should_write_after_invalid_config)
    {
        // Persist a newly chosen platform immediately, before running the
        // command. The prompt is meant to be one-time, and the choice is the
        // user's regardless of whether the command then succeeds — deferring
        // this to the post-command write loses it on any failure and prompts
        // again on the next run.
        config.write_to_file(&cli.args)?;
        // The write below only fires when the file still matches this snapshot,
        // so it has to reflect what was just written.
        original_config = config.clone();
    }

    let baseline_config = config.clone();

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
        eprint!("{}", envelope_output_string(&envelope, false)?);
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
        Commands::Test(phorge) => {
            ensure_human_pass_through(cli_args, "pcl test")?;
            phorge.run().await?;
        }
        #[cfg(feature = "credible")]
        Commands::Apply(apply) => apply.run(cli_args, config).await?,
        #[cfg(feature = "credible")]
        Commands::Deploy(deploy) => deploy.run(cli_args, config).await?,
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
        Commands::Build(build_cmd) => {
            ensure_human_pass_through(cli_args, "pcl build")?;
            build_cmd.run()?;
        }
        #[cfg(feature = "credible")]
        Commands::Verify(verify_cmd) => verify_cmd.run(cli_args)?,
        Commands::Download(download_cmd) => download_cmd.run(cli_args, config).await?,
    }
    Ok(())
}

/// Resolves the platform for this invocation and records it so every argument
/// accessor sees the same one.
///
/// Runs before dispatch, while `config` is still writable: a fresh selection is
/// recorded here and persisted by the normal config-write path, which keeps the
/// prompt one-time. Resolving once also gives every command a single place to
/// announce its platform.
///
/// Exits with a structured envelope when nothing resolves — a non-interactive
/// run with no platform cannot proceed, and must never hang on a prompt.
///
/// Returns whether a platform was recorded in `config` and therefore needs
/// persisting.
fn establish_active_platform(
    command: &Commands,
    cli_args: &CliArgs,
    config: &mut CliConfig,
) -> Result<bool> {
    // A command that needs no platform still reports the one it would use when
    // that is already known — `pcl doctor --offline` should name the platform it
    // is diagnosing — but it must never prompt, and must not fail when nothing
    // is configured.
    let needs_platform = command.needs_platform_url();
    let interaction = if needs_platform {
        Interaction::detect(cli_args.json_output())
    } else {
        Interaction::Forbidden
    };
    let remembered_before = config.platform_url.clone();
    match resolve_for_invocation(
        command.platform_url_flag().as_ref(),
        config,
        command.should_persist_platform_url(),
        interaction,
    ) {
        Ok(platform_url) => {
            // Only a command that actually talks to the platform announces it:
            // the line exists so a wrong network is noticed before it is acted
            // on. Local-only commands that surface the platform do so in their
            // own output, and must not prepend anything to an error.
            if needs_platform {
                announce_platform(&platform_url, cli_args.output_mode());
            }
            set_active_platform(platform_url);
            Ok(config.platform_url != remembered_before)
        }
        // Nothing to report, and nothing this command needs.
        Err(_) if !needs_platform => Ok(false),
        Err(err) => {
            let envelope = with_envelope_metadata(platform_error_envelope(&err));
            eprint!("{}", envelope_output_string(&envelope, false)?);
            std::process::exit(1);
        }
    }
}

/// Announces the platform every command is about to talk to, so a wrong
/// network is caught by eye rather than by a prompt.
///
/// Goes to stderr: stdout carries command data, and a machine run must stay
/// parseable.
fn announce_platform(platform_url: &Url, output_mode: OutputMode) {
    if output_mode == OutputMode::Human {
        eprintln!("Using {}", describe_platform(platform_url));
    }
}

fn platform_error_envelope(err: &PlatformError) -> Value {
    let (code, next_actions): (&str, &[&str]) = match err {
        PlatformError::NoPlatformResolved { .. } => {
            (
                "platform.not_selected",
                &[
                    "pcl auth login (choose a network interactively)",
                    "pcl <command> -u https://linea.phylax.systems",
                    "PCL_API_URL=https://linea.phylax.systems pcl <command>",
                ],
            )
        }
        PlatformError::SelectionFailed(_) => ("platform.selection_failed", &["pcl auth login"]),
    };
    simple_error_value(code, &err.to_string(), true, next_actions)
}

fn ensure_human_pass_through(
    cli_args: &CliArgs,
    command: &'static str,
) -> Result<(), ProductSurfaceError> {
    if cli_args.human_output() {
        return Ok(());
    }
    Err(ProductSurfaceError::InvalidInput(format!(
        "{command} is a developer pass-through command and does not support --json yet. Use human output, or use pcl verify/apply from a credible-enabled build for structured assertion workflows."
    )))
}

fn error_envelope(err: &Report) -> Value {
    if let Some(api_error) = err.downcast_ref::<ApiCommandError>() {
        return api_error.json_envelope();
    }
    if let Some(apply_error) = err.downcast_ref::<ApplyError>() {
        return with_envelope_metadata(apply_error_envelope(apply_error));
    }
    if let Some(deploy_error) = err.downcast_ref::<DeployError>() {
        return deploy_error_envelope(deploy_error);
    }
    if let Some(auth_error) = err.downcast_ref::<AuthError>() {
        return with_envelope_metadata(auth_error_envelope(auth_error));
    }
    if let Some(config_error) = err.downcast_ref::<ConfigError>() {
        return with_envelope_metadata(config_error_envelope(config_error));
    }
    if let Some(platform_error) = err.downcast_ref::<PlatformError>() {
        return with_envelope_metadata(platform_error_envelope(platform_error));
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
    #[cfg(feature = "credible")]
    if let ApplyError::AssertionsFailed(summary) = err {
        return verification_assertions_failed_envelope(
            summary,
            "apply.assertions_failed",
            "pcl apply --dry-run",
        );
    }

    let (code, message, next_actions): (&str, String, &[&str]) = match err {
        ApplyError::NoAuthToken => {
            (
                "auth.no_token",
                err.to_string(),
                &["pcl auth login", "pcl auth status"],
            )
        }
        ApplyError::ExpiredAuthToken(_) => {
            (
                "auth.expired_token",
                err.to_string(),
                &["pcl auth refresh --json", "pcl auth login --force"],
            )
        }
        ApplyError::AuthRefresh(_) => {
            (
                "auth.refresh_failed",
                err.to_string(),
                &["pcl auth refresh --json", "pcl auth login --force"],
            )
        }
        ApplyError::PlatformMismatch(_) => {
            (
                "auth.platform_mismatch",
                err.to_string(),
                &[
                    "pcl auth login --auth-url <platform-url>",
                    "pcl auth status --json",
                ],
            )
        }
        ApplyError::InvalidConfig(message) if message.contains("credible.toml not found") => {
            (
                "config.credible_toml_not_found",
                "No credible.toml found. Run from an assertion project or pass --config <path>."
                    .to_string(),
                &["pcl apply --help", "pcl projects mine"],
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
                &["pcl projects mine", "pcl account"],
            )
        }
        ApplyError::ApplyCancelled => ("apply.cancelled", err.to_string(), &["pcl apply --help"]),
        ApplyError::Platform(platform_error) => {
            return platform_error_envelope(platform_error);
        }
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
    if let DownloadError::Api {
        endpoint,
        status,
        request_id,
        body,
    } = err
    {
        return json!({
            "status": "error",
            "error": {
                "code": "download.api_failed",
                "message": err.to_string(),
                "recoverable": true,
                "request_id": request_id,
                "http": {
                    "method": "GET",
                    "path": endpoint,
                    "status": status,
                    "request_id": request_id,
                    "body": body,
                },
            },
            "next_actions": ["pcl download --help", "pcl doctor"],
        });
    }

    let (code, message, next_actions): (&str, String, &[&str]) = match err {
        DownloadError::NoAuthToken => {
            (
                "auth.no_token",
                err.to_string(),
                &["pcl auth login", "pcl auth status"],
            )
        }
        DownloadError::ExpiredAuthToken(_) => {
            (
                "auth.expired_token",
                err.to_string(),
                &["pcl auth refresh --json", "pcl auth login --force"],
            )
        }
        DownloadError::AuthRefresh(_) => {
            (
                "auth.refresh_failed",
                err.to_string(),
                &["pcl auth refresh --json", "pcl auth login --force"],
            )
        }
        DownloadError::PlatformMismatch(_) => {
            (
                "auth.platform_mismatch",
                err.to_string(),
                &[
                    "pcl auth login --auth-url <platform-url>",
                    "pcl auth status --json",
                ],
            )
        }
        DownloadError::MissingIdentifier => {
            (
                "download.missing_project_id",
                "--project-id is required".to_string(),
                &[
                    "pcl projects mine",
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
    if let VerifyError::AssertionsFailed(summary) = err {
        return verification_assertions_failed_envelope(
            summary,
            "verify.assertions_failed",
            "pcl verify --help",
        );
    }

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
        VerifyError::BytecodeHex(_) => {
            (
                "verify.invalid_bytecode_hex",
                err.to_string(),
                &["pcl verify --help"],
            )
        }
        VerifyError::ConstructorAbi(_) => {
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
        VerifyError::Output(_) => {
            (
                "output.failed",
                err.to_string(),
                &["Retry without --json to inspect human output"],
            )
        }
        VerifyError::AssertionsFailed(_) => unreachable!("handled above"),
    };
    simple_error_value(code, &message, true, next_actions)
}

#[cfg(feature = "credible")]
fn verification_assertions_failed_envelope(
    summary: &VerificationSummary,
    code: &str,
    help_command: &str,
) -> Value {
    json!({
        "status": "error",
        "data": summary,
        "error": {
            "code": code,
            "message": verification_failed_message(summary),
            "recoverable": true,
        },
        "next_actions": [
            "Inspect data.assertions for failing assertions",
            help_command,
        ],
    })
}

#[cfg(feature = "credible")]
fn verification_failed_message(summary: &VerificationSummary) -> String {
    format!(
        "{} of {} assertion{} failed verification",
        summary.failed,
        summary.total,
        if summary.total == 1 { "" } else { "s" }
    )
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

/// Machine envelope for `pcl deploy` failures. Wrapped API/apply/auth
/// errors delegate to their own envelope mappers so provenance (HTTP
/// metadata, tx hashes, structured codes) survives; every orchestration
/// variant gets a stable code, recoverability, and next actions.
#[allow(clippy::too_many_lines)]
fn deploy_error_envelope(err: &DeployError) -> Value {
    match err {
        DeployError::WithWarnings { source, warnings } => {
            let mut envelope = deploy_error_envelope(source);
            if !warnings.is_empty() {
                envelope["warnings"] = json!(warnings);
            }
            envelope
        }
        // Delegate wrapped errors to the mappers that know their structure —
        // notably ConfirmAfterTx, whose envelope carries the landed tx hash
        // and the nested API provenance.
        DeployError::Api(api_error) => api_error.json_envelope(),
        DeployError::Apply(apply_error) => {
            with_envelope_metadata(apply_error_envelope(apply_error))
        }
        _ => {
            let (code, recoverable, next_actions): (&str, bool, Vec<String>) = match err {
                DeployError::Wallet(_) => {
                    (
                        "wallet.failed",
                        true,
                        vec![
                            "pcl deploy --dry-run".to_string(),
                            "pcl deploy --help".to_string(),
                        ],
                    )
                }
                DeployError::ManagerMismatch {
                    project_id, wallet, ..
                } => {
                    (
                        "deploy.manager_mismatch",
                        true,
                        vec![format!(
                            "pcl protocol-manager --project {project_id} --transfer-calldata --new-manager {wallet} --broadcast"
                        )],
                    )
                }
                DeployError::MissingProjectInfo => {
                    (
                        "deploy.missing_project_info",
                        true,
                        vec!["pcl deploy --project-name <name> --chain-id <id>".to_string()],
                    )
                }
                DeployError::ChainIdMismatch { .. } => {
                    (
                        "deploy.chain_id_mismatch",
                        true,
                        vec!["pcl deploy (omit --chain-id for an existing project)".to_string()],
                    )
                }
                DeployError::UnexpectedResponse { .. } => {
                    (
                        "deploy.unexpected_response",
                        true,
                        vec!["pcl doctor --json".to_string()],
                    )
                }
                DeployError::ChecksTimeout { .. } => {
                    (
                        "deploy.checks_timeout",
                        true,
                        vec!["pcl deploy (re-run to resume once checks finish)".to_string()],
                    )
                }
                DeployError::ChecksFailed {
                    project_id,
                    release_id,
                    ..
                } => {
                    (
                        "deploy.checks_failed",
                        true,
                        vec![
                            format!("pcl releases show {project_id} {release_id}"),
                            format!(
                                "pcl releases retry-check {project_id} {release_id} <check-id>"
                            ),
                        ],
                    )
                }
                DeployError::TomlWriteBack { .. } => {
                    (
                        "deploy.toml_write_back_failed",
                        true,
                        vec!["Check credible.toml permissions, then re-run pcl deploy".to_string()],
                    )
                }
                DeployError::TomlWriteBackAfterCreate { project_id, .. } => {
                    (
                        "deploy.project_created_write_back_failed",
                        true,
                        vec![
                            format!("Add project_id = \"{project_id}\" to credible.toml"),
                            "pcl deploy (re-run to resume with the existing project)".to_string(),
                        ],
                    )
                }
                DeployError::CreateIntentWrite { .. } => {
                    (
                        "deploy.create_intent_write_failed",
                        true,
                        vec![
                            "Check the credible.toml directory permissions, then re-run pcl deploy"
                                .to_string(),
                        ],
                    )
                }
                DeployError::CreateIntentUnreadable { path, .. } => {
                    (
                        "deploy.create_intent_unreadable",
                        true,
                        vec![
                        "pcl projects mine --json".to_string(),
                        "Add project_id = \"<id>\" to credible.toml if the project already exists"
                            .to_string(),
                        format!("Delete {path} once the earlier create is resolved"),
                    ],
                    )
                }
                DeployError::CreateIntentPlatformMismatch { path, .. } => {
                    (
                        "deploy.create_intent_platform_mismatch",
                        true,
                        vec![
                            "pcl projects mine --json (against the intent's platform)".to_string(),
                            format!("Delete {path} once the earlier create is resolved"),
                        ],
                    )
                }
                DeployError::AmbiguousProjectCreate { path, .. } => {
                    (
                        "deploy.project_create_ambiguous",
                        true,
                        vec![
                            "pcl projects mine --json".to_string(),
                            "Add project_id = \"<id>\" to credible.toml".to_string(),
                            format!("Delete {path} once the right project is recorded"),
                        ],
                    )
                }
                DeployError::PendingProjectCreate { path, .. } => {
                    (
                        "deploy.project_create_pending_reconciliation",
                        true,
                        vec![
                            "pcl projects mine --json".to_string(),
                            "pcl deploy (retry reconciliation; no second project will be created)"
                                .to_string(),
                            format!(
                                "Only after confirming no project was created: delete {path} and re-run pcl deploy"
                            ),
                        ],
                    )
                }
                DeployError::AdoptedProjectChainMismatch {
                    project_id, path, ..
                } => {
                    (
                        "deploy.adopted_project_chain_mismatch",
                        true,
                        vec![
                            "pcl projects mine --json".to_string(),
                            "pcl deploy (retry reconciliation; no second project will be created)"
                                .to_string(),
                            format!(
                                "If {project_id} is the intended project, add project_id = \"{project_id}\" to credible.toml and delete {path}"
                            ),
                        ],
                    )
                }
                DeployError::AmbiguousInactiveRelease { project_id, .. } => {
                    (
                        "deploy.inactive_release_ambiguous",
                        true,
                        vec![
                            format!("pcl releases list {project_id}"),
                            "Activate or delete the duplicate inactive releases, then re-run pcl deploy"
                                .to_string(),
                        ],
                    )
                }
                DeployError::MachineYesRequired => {
                    (
                        "deploy.yes_required",
                        true,
                        vec!["pcl deploy --yes --json".to_string()],
                    )
                }
                DeployError::Cancelled => {
                    ("deploy.cancelled", true, vec!["pcl deploy".to_string()])
                }
                DeployError::Json(_) | DeployError::Output(_) => {
                    ("deploy.output_failed", false, Vec::new())
                }
                DeployError::Api(_)
                | DeployError::Apply(_)
                | DeployError::WithWarnings { .. } => unreachable!("delegated above"),
            };
            let mut error = serde_json::Map::new();
            error.insert("code".to_string(), json!(code));
            error.insert("message".to_string(), json!(err.to_string()));
            error.insert("recoverable".to_string(), json!(recoverable));
            if let DeployError::TomlWriteBackAfterCreate {
                project_id, path, ..
            } = err
            {
                error.insert("project_id".to_string(), json!(project_id));
                error.insert("path".to_string(), json!(path));
            }
            with_envelope_metadata(json!({
                "status": "error",
                "error": error,
                "next_actions": next_actions,
            }))
        }
    }
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
        AuthError::PlatformMismatch { requested, .. } => {
            with_envelope_metadata(json!({
                "status": "error",
                "error": {
                    "code": "auth.platform_mismatch",
                    "message": err.to_string(),
                    "recoverable": true,
                },
                "next_actions": [
                    format!("pcl auth login --auth-url {requested}"),
                    "pcl auth status --json".to_string(),
                ],
            }))
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
    command_for_mode(command, current_output_mode())
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
        ConfigError::InvalidValue(_) => "config.invalid_value",
    }
}

fn wants_output_mode<I, S>(args: I) -> OutputMode
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let saw_json = args.into_iter().any(|arg| {
        let arg = arg.as_ref();
        arg == OsStr::new("--json") || arg == OsStr::new("-j")
    });

    if saw_json {
        OutputMode::Json
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
    if matches!(
        err.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        return with_envelope_metadata(json!({
            "status": "ok",
            "data": {
                "kind": clap_error_code(err.kind()),
                "message": message,
            },
            "next_actions": next_actions,
        }));
    }
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
            "--json" | "-j" | "--llms" | "--help" | "-h" | "--version" | "-V" => {}
            "--config-dir" => {
                let _ = iter.next();
            }
            _ if value.starts_with("--config-dir=") => {}
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
    use pcl_core::api::envelope_output_string;

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
            wants_output_mode(["pcl", "--toon", "api"]),
            OutputMode::Human
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
            .try_get_matches_from([
                "pcl",
                "--json",
                "api",
                "call",
                "get",
                "/health",
                "--body",
                "{}",
                "--body-file",
                "body.json",
            ])
            .unwrap_err();
        let args = vec![
            OsString::from("pcl"),
            OsString::from("--json"),
            OsString::from("api"),
            OsString::from("call"),
            OsString::from("get"),
            OsString::from("/health"),
            OsString::from("--body"),
            OsString::from("{}"),
            OsString::from("--body-file"),
            OsString::from("body.json"),
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
    fn renders_clap_conflicts_as_machine_errors() {
        let err = Cli::command()
            .try_get_matches_from([
                "pcl",
                "api",
                "call",
                "get",
                "/health",
                "--body",
                "{}",
                "--body-file",
                "body.json",
            ])
            .unwrap_err();
        let args = vec![
            OsString::from("pcl"),
            OsString::from("api"),
            OsString::from("call"),
            OsString::from("get"),
            OsString::from("/health"),
            OsString::from("--body"),
            OsString::from("{}"),
            OsString::from("--body-file"),
            OsString::from("body.json"),
        ];
        let output = envelope_output_string(&clap_error_envelope(&err, &args), true).unwrap();

        assert!(output.contains("\"status\": \"error\""));
        assert!(output.contains("cli.argument_conflict"));
        assert!(output.contains("\"message\""));
        assert!(output.contains("Usage: pcl api call --body"));
        assert!(output.contains("\"recoverable\": true"));
        assert!(!output.contains("Location:"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn wraps_clap_help_as_success_envelope() {
        let err = Cli::command()
            .try_get_matches_from(["pcl", "--help"])
            .unwrap_err();
        let args = vec![OsString::from("pcl"), OsString::from("--help")];
        let envelope = clap_error_envelope(&err, &args);

        assert_eq!(envelope["status"], "ok");
        assert_eq!(envelope["data"]["kind"], "cli.help");
        assert!(envelope["error"].is_null());
    }

    #[test]
    fn renders_runtime_errors_as_machine_errors() {
        let err = Report::new(ApiCommandError::NoAuthToken);
        let output = envelope_output_string(&error_envelope(&err), true).unwrap();

        assert!(output.contains("\"status\": \"error\""));
        assert!(output.contains("auth.no_token"));
        assert!(output.contains("\"recoverable\": true"));
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

    #[test]
    fn deploy_api_failure_keeps_post_scan_spec_warnings() {
        let warning = json!({
            "code": "assertion_spec.v2_unsupported",
            "message": "production runs V1",
        });
        let err = DeployError::WithWarnings {
            source: Box::new(DeployError::Apply(ApplyError::Api {
                endpoint: "/projects/id/releases".to_string(),
                status: Some(400),
                body: "unsupported assertion spec".to_string(),
            })),
            warnings: vec![warning],
        };

        let envelope = deploy_error_envelope(&err);

        assert_eq!(envelope["status"], "error");
        assert_eq!(
            envelope["warnings"][0]["code"],
            "assertion_spec.v2_unsupported"
        );
        assert_eq!(envelope["error"]["code"], "apply.failed");
    }
}
