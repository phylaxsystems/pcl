use crate::{
    api::{
        envelope_output_string,
        with_envelope_metadata,
    },
    error::ConfigError,
};
use alloy_primitives::Address;
use chrono::{
    DateTime,
    Utc,
};
use clap::Parser;
use colored::Colorize;
use dirs::home_dir;
use pcl_common::args::{
    CliArgs,
    OutputMode,
    current_output_mode,
};
use serde::{
    Deserialize,
    Serialize,
};
use serde_json::{
    Value,
    json,
};

use std::{
    collections::BTreeMap,
    fmt,
    fs::OpenOptions,
    io::Write,
    path::{
        Path,
        PathBuf,
    },
};
use tokio::time::{
    Duration,
    Instant,
    sleep,
};
use uuid::Uuid;

/// Legacy directory name for storing PCL configuration (deprecated)
const LEGACY_CONFIG_DIR: &str = ".pcl";
/// Directory name for storing PCL configuration under XDG config home
const CONFIG_DIR_NAME: &str = "pcl";
/// Configuration file name
pub const CONFIG_FILE: &str = "config.toml";
pub const AUTH_EXPIRES_SOON_SECONDS: i64 = 300;

/// How long to wait for another process to release the config lock.
const CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to retry while the config lock is held.
const CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Cross-process guard over the config file, held for a whole
/// read-modify-write.
///
/// Compare-and-write ([`CliConfig::write_to_file_if_unchanged`]) is not enough
/// on its own: it narrows the window between reading and writing but cannot
/// close it, so two processes can still interleave and the loser's write is
/// simply dropped. That is tolerable for a passive rewrite, and *not* tolerable
/// for the refresh token — there is only ever one valid refresh token, so
/// losing the write that recorded it logs the user out.
///
/// So every writer that merges into whatever is currently on disk takes this
/// lock, and they all take the *same* one: token refresh and platform selection
/// are different operations on the same file, and a lock that only refresh
/// honoured would not serialize them against each other.
///
/// Released on drop, including on panic. A stale file left by a killed process
/// does not wedge the CLI forever: waiting gives up after 30 seconds and
/// surfaces [`ConfigError::LockTimeout`] instead.
#[derive(Debug)]
pub struct ConfigLock {
    path: PathBuf,
}

impl ConfigLock {
    /// Acquires the lock, waiting up to 30 seconds for another process to
    /// release it before returning [`ConfigError::LockTimeout`].
    pub async fn acquire(cli_args: &CliArgs) -> Result<Self, ConfigError> {
        let path = CliConfig::config_file_path(cli_args).with_extension("toml.lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::WriteError)?;
        }
        let deadline = Instant::now() + CONFIG_LOCK_TIMEOUT;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(
                        file,
                        "pid={} acquired_at={}",
                        std::process::id(),
                        Utc::now().to_rfc3339()
                    );
                    let _ = file.sync_all();
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return Err(ConfigError::LockTimeout);
                    }
                    sleep(CONFIG_LOCK_POLL_INTERVAL).await;
                }
                Err(error) => return Err(ConfigError::WriteError(error)),
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Main configuration structure for PCL
///
/// This struct holds all the configuration data for the PCL tool,
/// including authentication details.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliConfig {
    /// Optional authentication details
    pub auth: Option<UserAuth>,
    /// Platform URL remembered from the last `pcl auth login` or interactive
    /// network selection.
    ///
    /// Always set explicitly once a platform has been chosen — there is no
    /// production default to encode as an absent value — and cleared on
    /// `pcl auth logout`. Resolution reads this after an explicit
    /// `-u`/`PCL_API_URL` and before prompting, so a platform only has to be
    /// chosen once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_url: Option<String>,
    /// Per-chain RPC endpoints used when broadcasting transactions.
    /// Keys are chain ids as strings (TOML table keys must be strings).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rpc: BTreeMap<String, RpcEndpoint>,
}

/// RPC endpoint configuration for a single chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcEndpoint {
    /// HTTP(S) RPC URL for the chain
    pub url: String,
    /// Confirmations to wait for after broadcasting (defaults to 1 when absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u64>,
}

impl CliConfig {
    /// Returns the configured RPC endpoint for a chain id, if any.
    pub fn rpc_endpoint(&self, chain_id: u64) -> Option<&RpcEndpoint> {
        self.rpc.get(&chain_id.to_string())
    }

    /// Records `platform_url` as both the remembered platform and the issuer of
    /// the stored credentials — the state a completed login leaves behind.
    ///
    /// Test-only, and deliberately one call: a fixture that sets only the
    /// remembered platform describes credentials of unknown provenance, which
    /// the platform-boundary check refuses. Tests that mean *that* state set
    /// `platform_url` on its own.
    #[cfg(test)]
    pub(crate) fn set_test_platform(&mut self, platform_url: &str) {
        self.platform_url = Some(platform_url.to_string());
        if let Some(auth) = &mut self.auth {
            auth.issuer_platform_url = Some(platform_url.to_string());
        }
    }
}

/// Command-line arguments for configuration management
#[derive(Parser)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

/// Subcommands for configuration management
#[derive(clap::Subcommand)]
enum ConfigCommand {
    #[command(about = "Display the current configuration")]
    Show,
    #[command(about = "Delete the current configuration")]
    Delete,
    #[command(
        name = "set-rpc",
        about = "Set the RPC endpoint used when broadcasting transactions on a chain"
    )]
    SetRpc {
        /// Chain id the endpoint serves
        chain_id: u64,
        /// HTTP(S) RPC URL
        url: String,
        /// Confirmations to wait for after broadcasting (default 1)
        #[arg(long)]
        confirmations: Option<u64>,
    },
    #[command(
        name = "unset-rpc",
        about = "Remove the stored RPC endpoint for a chain"
    )]
    UnsetRpc {
        /// Chain id to remove
        chain_id: u64,
    },
}

impl ConfigArgs {
    pub fn can_run_without_valid_config(&self) -> bool {
        matches!(self.command, ConfigCommand::Delete)
    }

    pub fn should_force_config_write(&self) -> bool {
        matches!(self.command, ConfigCommand::Delete)
    }

    /// Executes the configuration command
    ///
    /// # Arguments
    /// * `config` - The configuration to operate on
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    pub fn run(&self, config: &mut CliConfig, cli_args: &CliArgs) -> Result<(), ConfigError> {
        match &self.command {
            ConfigCommand::SetRpc {
                chain_id,
                url,
                confirmations,
            } => {
                let parsed = url::Url::parse(url).map_err(|e| {
                    ConfigError::InvalidValue(format!(
                        "invalid RPC URL {}: {e}",
                        redacted_rpc_host(url)
                    ))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(ConfigError::InvalidValue(format!(
                        "RPC URL must be http(s), got {:?}",
                        parsed.scheme()
                    )));
                }
                config.rpc.insert(
                    chain_id.to_string(),
                    RpcEndpoint {
                        url: url.clone(),
                        confirmations: *confirmations,
                    },
                );
                // Echo only non-secret metadata; the URL may embed an API key.
                print_config_output(
                    &with_envelope_metadata(json!({
                        "status": "ok",
                        "data": {
                            "chain_id": chain_id,
                            "rpc": {
                                "configured": true,
                                "host": redacted_rpc_host(url),
                                "confirmations": confirmations,
                            },
                        },
                        "next_actions": ["pcl config show"],
                    })),
                    cli_args.json_output(),
                )
            }
            ConfigCommand::UnsetRpc { chain_id } => {
                let removed = config.rpc.remove(&chain_id.to_string());
                print_config_output(
                    &with_envelope_metadata(json!({
                        "status": "ok",
                        "data": {
                            "chain_id": chain_id,
                            "removed": removed.is_some(),
                        },
                        "next_actions": ["pcl config show"],
                    })),
                    cli_args.json_output(),
                )
            }
            ConfigCommand::Show => {
                print_config_output(
                    &config_show_envelope(config, cli_args),
                    cli_args.json_output(),
                )
            }
            ConfigCommand::Delete => {
                *config = CliConfig::default();
                print_config_output(
                    &json!({
                        "status": "ok",
                        "data": {
                            "deleted": true,
                            "config_path": CliConfig::config_file_path(cli_args).display().to_string(),
                            "auth": config_auth_value(config),
                        },
                        "next_actions": [
                            "pcl auth login",
                            "pcl config show",
                        ],
                    }),
                    cli_args.json_output(),
                )
            }
        }
    }
}

impl CliConfig {
    /// Updates stored auth expiry from the JWT `exp` claim when available.
    ///
    /// Older CLI versions stored the short device-login session expiry here,
    /// which made valid tokens look expired after only a few minutes.
    pub fn normalize_auth_expiry_from_access_token(&mut self) -> bool {
        let Some(auth) = &mut self.auth else {
            return false;
        };
        let Some(token_expires_at) = auth.access_token_expires_at() else {
            return false;
        };
        if auth.expires_at == token_expires_at {
            return false;
        }
        auth.expires_at = token_expires_at;
        true
    }

    /// Returns the path to the active config file for the supplied CLI arguments.
    pub fn config_file_path(cli_args: &CliArgs) -> PathBuf {
        cli_args
            .config_dir
            .clone()
            .unwrap_or(Self::get_config_dir())
            .join(CONFIG_FILE)
    }

    /// Writes the configuration to the default config file, or a specific directory
    ///
    /// # Arguments
    /// * `cli_args` - Command line arguments
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    pub fn write_to_file(&self, cli_args: &CliArgs) -> Result<(), ConfigError> {
        self.write_to_file_at_dir(
            &cli_args
                .config_dir
                .clone()
                .unwrap_or(Self::get_config_dir()),
        )
    }

    /// Records this process's chosen platform without clobbering a concurrent
    /// write, then adopts whatever ended up on disk.
    ///
    /// The in-memory config was read before the network selector prompted, and
    /// the prompt stays open for as long as the user takes to answer. Another
    /// `pcl` can rotate the refresh token in that window, so this process's
    /// snapshot must not be written wholesale — only `platform_url` is merged
    /// into the current file.
    ///
    /// The read and the write are both inside a [`ConfigLock`] because merging
    /// is only safe if nothing else writes in between: a token refresh landing
    /// after the read would be overwritten by the write, discarding the only
    /// valid refresh token. Holding the same lock refresh takes makes the two
    /// operations mutually exclusive rather than merely narrow.
    pub async fn merge_selected_platform(&mut self, cli_args: &CliArgs) -> Result<(), ConfigError> {
        let _lock = ConfigLock::acquire(cli_args).await?;
        let Ok(mut current) = Self::read_from_file(cli_args) else {
            // Unreadable only if it changed under us since startup, where the
            // caller already parsed it. Nothing to merge, so record the choice.
            return self.write_to_file(cli_args);
        };
        if current == *self {
            return Ok(());
        }
        current.normalize_auth_expiry_from_access_token();
        current.platform_url.clone_from(&self.platform_url);
        current.write_to_file(cli_args)?;
        *self = current;
        Ok(())
    }

    /// Writes the configuration only when the on-disk config still matches
    /// the snapshot read at process start. This prevents a read-only command
    /// from overwriting credentials that another process just refreshed.
    pub fn write_to_file_if_unchanged(
        &self,
        cli_args: &CliArgs,
        expected_current: &Self,
    ) -> Result<bool, ConfigError> {
        let current = Self::read_from_file(cli_args)?;
        if current != *expected_current {
            return Ok(false);
        }
        self.write_to_file(cli_args)?;
        Ok(true)
    }

    /// Copies an unreadable config file aside before it is replaced.
    ///
    /// A repair command (`pcl auth login`, `pcl config delete`) is allowed to
    /// write a fresh config over a file it could not parse. Those bytes may
    /// still hold recoverable credentials or RPC settings, so they are kept at
    /// `config.toml.invalid` rather than dropped. Returns the backup path, or
    /// `None` when there was no file to preserve.
    ///
    /// Only ever reached while the current file is unparseable, so it cannot
    /// overwrite a backup taken from a good config: once the repair succeeds the
    /// file parses and this is not called again.
    pub fn back_up_unreadable(cli_args: &CliArgs) -> Result<Option<PathBuf>, ConfigError> {
        let source = Self::config_file_path(cli_args);
        if !source.exists() {
            return Ok(None);
        }
        let backup = source.with_extension("toml.invalid");
        std::fs::copy(&source, &backup).map_err(ConfigError::WriteError)?;
        Ok(Some(backup))
    }

    /// Writes the configuration to a specific directory
    ///
    /// # Arguments
    /// * `config_dir` - Directory to write the config file to
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn write_to_file_at_dir(&self, config_dir: &PathBuf) -> Result<(), ConfigError> {
        // Ensure directory exists and is writable
        Self::ensure_writable_directory(config_dir)?;

        // Get config file path and check permissions
        let config_file = config_dir.join(CONFIG_FILE);
        Self::ensure_writable_file(&config_file)?;

        // Serialize and write config atomically so access/refresh tokens never
        // land on disk as a partially-written pair.
        let config_str = toml::to_string(self).map_err(ConfigError::SerializeError)?;
        let temp_file = config_dir.join(format!(
            ".{CONFIG_FILE}.{}.{}.tmp",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_file)
                .map_err(ConfigError::WriteError)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&temp_file, std::fs::Permissions::from_mode(0o600))
                    .map_err(ConfigError::WriteError)?;
            }
            file.write_all(config_str.as_bytes())
                .map_err(ConfigError::WriteError)?;
            file.sync_all().map_err(ConfigError::WriteError)?;
        }
        std::fs::rename(&temp_file, &config_file).map_err(|error| {
            let _ = std::fs::remove_file(&temp_file);
            ConfigError::WriteError(error)
        })?;
        if let Some(parent) = config_file.parent()
            && let Ok(parent_file) = std::fs::File::open(parent)
        {
            let _ = parent_file.sync_all();
        }
        Ok(())
    }

    /// Ensures a directory exists and is writable
    ///
    /// # Arguments
    /// * `dir` - Directory to check
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn ensure_writable_directory(dir: &PathBuf) -> Result<(), ConfigError> {
        if !dir.exists() {
            std::fs::create_dir_all(dir).map_err(|e| {
                ConfigError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Failed to create config directory: {e}"),
                ))
            })?;
        }

        // Test write permissions by creating a temporary file
        let temp_file = dir.join(".pcl_test_write");
        std::fs::write(&temp_file, "").map_err(|e| {
            ConfigError::WriteError(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("No write permissions in config directory: {e}"),
            ))
        })?;
        std::fs::remove_file(&temp_file).ok(); // Clean up test file

        Ok(())
    }

    /// Ensures a file is writable
    ///
    /// # Arguments
    /// * `file` - File to check
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn ensure_writable_file(file: &PathBuf) -> Result<(), ConfigError> {
        if file.exists() {
            let metadata = std::fs::metadata(file).map_err(|e| {
                ConfigError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Failed to check file permissions: {e}"),
                ))
            })?;

            if metadata.permissions().readonly() {
                return Err(ConfigError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Config file is read-only",
                )));
            }
        }
        Ok(())
    }

    /// Gets the legacy configuration directory path (~/.pcl)
    ///
    /// # Returns
    /// * `PathBuf` - Path to the legacy config directory
    ///
    /// # Panics
    ///
    /// Will panic if it does not find the home directory
    fn get_legacy_config_dir() -> PathBuf {
        home_dir().unwrap().join(LEGACY_CONFIG_DIR)
    }

    /// Gets the default configuration directory path
    ///
    /// Uses XDG Base Directory Specification:
    /// - `$XDG_CONFIG_HOME/pcl` if `XDG_CONFIG_HOME` is set
    /// - `~/.config/pcl` otherwise
    ///
    /// # Returns
    /// * `PathBuf` - Path to the config directory
    ///
    /// # Panics
    ///
    /// Will panic if it does not find the home directory
    pub fn get_config_dir() -> PathBuf {
        std::env::var("XDG_CONFIG_HOME")
            .map_or_else(|_| home_dir().unwrap().join(".config"), PathBuf::from)
            .join(CONFIG_DIR_NAME)
    }

    /// Migrates configuration from the legacy location (`~/.pcl`) to the new
    /// XDG-compliant location (`~/.config/pcl` or `$XDG_CONFIG_HOME/pcl`)
    ///
    /// Migration only occurs if:
    /// - The legacy directory exists
    /// - The new directory does not exist
    ///
    /// # Returns
    /// * `Ok(true)` - Migration was performed
    /// * `Ok(false)` - No migration needed
    /// * `Err(ConfigError)` - Migration failed
    pub fn migrate_legacy_config() -> Result<bool, ConfigError> {
        let legacy_dir = Self::get_legacy_config_dir();
        let new_dir = Self::get_config_dir();

        // Only migrate if legacy exists and new doesn't
        if legacy_dir.exists() && !new_dir.exists() {
            // Create parent dirs if needed
            if let Some(parent) = new_dir.parent() {
                std::fs::create_dir_all(parent).map_err(ConfigError::WriteError)?;
            }
            // Move the directory
            std::fs::rename(&legacy_dir, &new_dir).map_err(ConfigError::WriteError)?;
            if current_output_mode() == OutputMode::Human {
                eprintln!(
                    "{}: Migrated PCL config from {} to {}",
                    "Warning".yellow().bold(),
                    legacy_dir.display(),
                    new_dir.display()
                );
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Reads configuration from a specific directory
    ///
    /// # Arguments
    /// * `config_dir` - Directory to read the config file from
    ///
    /// # Returns
    /// * `Result<Self, ConfigError>` - Configuration or error
    fn read_from_file_at_dir(config_dir: &Path) -> Result<Self, ConfigError> {
        let config_file = config_dir.join(CONFIG_FILE);

        // If file doesn't exist, return default config
        if !config_file.exists() {
            return Ok(Self::default());
        }

        // Check if we have read permissions
        let metadata = std::fs::metadata(&config_file).map_err(|e| {
            ConfigError::ReadError(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("Failed to check file permissions: {e}"),
            ))
        })?;

        if !metadata.permissions().readonly() {
            // Test read permissions
            std::fs::read_to_string(&config_file).map_err(|e| {
                ConfigError::ReadError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("No read permissions for config file: {e}"),
                ))
            })?;
        }

        let config_str = std::fs::read_to_string(config_file).map_err(ConfigError::ReadError)?;
        toml::from_str(&config_str).map_err(ConfigError::ParseError)
    }

    /// Reads configuration from the default config file, or a specific directory
    ///
    /// If using the default config directory, this will first attempt to migrate
    /// any existing configuration from the legacy location (~/.pcl) to the new
    /// XDG-compliant location.
    ///
    /// # Arguments
    /// * `cli_args` - Command line arguments
    ///
    /// # Returns
    /// * `Result<Self, ConfigError>` - Configuration or error
    pub fn read_from_file(cli_args: &CliArgs) -> Result<Self, ConfigError> {
        // Only attempt migration when using default config dir
        if cli_args.config_dir.is_none() {
            // Attempt migration from legacy location (errors are non-fatal)
            let _ = Self::migrate_legacy_config();
        }

        Self::read_from_file_at_dir(
            &cli_args
                .config_dir
                .clone()
                .unwrap_or(Self::get_config_dir()),
        )
    }
}

fn config_show_envelope(config: &CliConfig, cli_args: &CliArgs) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": {
            "config_path": CliConfig::config_file_path(cli_args).display().to_string(),
            "platform_url": config.platform_url.as_deref(),
            "auth": config_auth_value(config),
            "rpc": redacted_rpc_endpoints(config),
        },
        "next_actions": if config.auth.is_some() {
            json!(["pcl auth status", "pcl account", "pcl doctor"])
        } else {
            json!(["pcl auth login", "pcl doctor"])
        },
    }))
}

fn config_auth_value(config: &CliConfig) -> Value {
    let Some(auth) = &config.auth else {
        return json!({
            "authenticated": false,
            "token_present": false,
            "refresh_token_present": false,
            "refresh_expires_at": null,
            "refresh_seconds_remaining": null,
            "token_valid": false,
            "token_expired": false,
            "expires_soon": false,
            "expired": false,
            "expires_at": null,
            "seconds_remaining": null,
            "expires_in_seconds": null,
        });
    };

    let now = Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    let token_expired = auth.expires_at <= now;
    let expires_soon = !token_expired && seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS;
    let refresh_seconds_remaining = auth
        .refresh_expires_at
        .map(|expires_at| (expires_at - now).num_seconds());
    json!({
        "authenticated": true,
        "user": auth.display_name(),
        "user_id": auth.user_id.map(|id| id.to_string()),
        "wallet_address": auth.wallet_address.map(|address| address.to_string()),
        "email": auth.email.as_deref(),
        "token_present": !auth.access_token.is_empty(),
        "refresh_token_present": !auth.refresh_token.is_empty(),
        "refresh_expires_at": auth.refresh_expires_at.map(|expires_at| expires_at.to_rfc3339()),
        "refresh_seconds_remaining": refresh_seconds_remaining,
        "token_valid": !token_expired,
        "token_expired": token_expired,
        "expires_soon": expires_soon,
        "expired": token_expired,
        "expires_at": auth.expires_at.to_rfc3339(),
        "seconds_remaining": seconds_remaining,
        "expires_in_seconds": seconds_remaining,
    })
}

fn print_config_output(value: &Value, json_output: bool) -> Result<(), ConfigError> {
    print!(
        "{}",
        envelope_output_string(value, json_output).map_err(ConfigError::JsonError)?
    );
    Ok(())
}

/// Non-secret view of the stored RPC endpoints for `pcl config show`.
/// Provider URLs commonly carry API keys in userinfo, path, or query
/// parameters, so only chain id, host, and confirmations are exposed; the
/// full URL never leaves config storage.
fn redacted_rpc_endpoints(config: &CliConfig) -> Value {
    let mut endpoints = serde_json::Map::new();
    for (chain_id, endpoint) in &config.rpc {
        endpoints.insert(
            chain_id.clone(),
            json!({
                "configured": true,
                "host": redacted_rpc_host(&endpoint.url),
                "confirmations": endpoint.confirmations,
            }),
        );
    }
    Value::Object(endpoints)
}

/// Scheme, host, and explicit port of an RPC URL; everything else (userinfo,
/// path, query) may carry credentials and is dropped.
pub(crate) fn redacted_rpc_host(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "<unparseable-url>".to_string();
    };
    let host = parsed.host_str().unwrap_or("<unknown-host>");
    match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    }
}

impl fmt::Display for CliConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config_path = Self::get_config_dir().join(CONFIG_FILE);

        writeln!(f, "PCL Configuration")?;
        writeln!(f, "==================")?;
        writeln!(f, "Config path: {}", config_path.display())?;
        if let Some(platform_url) = &self.platform_url {
            writeln!(f, "Platform URL: {platform_url}")?;
        }

        match &self.auth {
            Some(auth) => writeln!(f, "{auth}")?,
            None => writeln!(f, "Authentication: Not authenticated")?,
        }

        if !self.rpc.is_empty() {
            writeln!(f, "RPC endpoints:")?;
            for (chain_id, endpoint) in &self.rpc {
                // Redacted: provider URLs commonly embed API keys.
                write!(
                    f,
                    "  chain {chain_id}: {}",
                    redacted_rpc_host(&endpoint.url)
                )?;
                match endpoint.confirmations {
                    Some(confirmations) => writeln!(f, " ({confirmations} confirmations)")?,
                    None => writeln!(f)?,
                }
            }
        }

        Ok(())
    }
}

/// Authentication details for a user
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAuth {
    /// Access token for API authentication
    pub access_token: String,
    /// Refresh token for obtaining new access tokens
    pub refresh_token: String,
    /// Token expiration timestamp
    #[serde(with = "chrono::serde::ts_seconds")]
    pub expires_at: DateTime<Utc>,
    /// Refresh token sliding expiration timestamp, when returned by the platform.
    #[serde(
        default,
        with = "chrono::serde::ts_seconds_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub refresh_expires_at: Option<DateTime<Utc>>,
    /// Platform user ID (UUID), used for API calls that require it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    /// Ethereum address of the user (only present for wallet-based auth)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_address: Option<Address>,
    /// Email address of the user (for email-based auth)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The platform that issued these credentials.
    ///
    /// Deliberately stored *with* the credentials rather than alongside the
    /// remembered platform in [`CliConfig::platform_url`]. The two answer
    /// different questions — "which platform did I choose?" versus "who issued
    /// this token?" — and only the second may gate attaching the token to a
    /// request. Keeping provenance here makes that separation structural: it is
    /// written only when fresh credentials are stored, cleared with them on
    /// logout, and cannot be rebound by platform resolution or an interactive
    /// selection.
    ///
    /// `None` for credentials stored by a release that did not record it. There
    /// is no default to assume those into, so they belong to an unknown
    /// platform and force a one-time re-login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_platform_url: Option<String>,
}

impl UserAuth {
    pub fn access_token_expires_at(&self) -> Option<DateTime<Utc>> {
        access_token_expires_at(&self.access_token)
    }

    /// Returns the best available display name for this user.
    pub fn display_name(&self) -> String {
        if let Some(addr) = &self.wallet_address
            && *addr != Address::ZERO
        {
            return addr.to_string();
        }
        if let Some(email) = &self.email {
            return email.clone();
        }
        if let Some(id) = &self.user_id {
            return id.to_string();
        }
        "unknown".to_string()
    }
}

pub fn access_token_expires_at(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let payload = decode_base64_url(payload)?;
    let payload: Value = serde_json::from_slice(&payload).ok()?;
    let exp = payload.get("exp").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|exp| i64::try_from(exp).ok()))
    })?;
    DateTime::from_timestamp(exp, 0)
}

fn decode_base64_url(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Some(output)
}

impl fmt::Display for UserAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Authentication:")?;
        writeln!(f, "  User: {}", self.display_name())?;
        let now = Utc::now();
        let expired = self.expires_at < now;
        let expiry_text = self.expires_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
        if expired {
            writeln!(f, "  Token Expired at {}", expiry_text.red())?;
        } else {
            writeln!(f, "  Token Expires at {}", expiry_text.green())?;
        }

        // Don't display actual tokens for security reasons
        writeln!(f, "  Access Token: [Set]")?;
        writeln!(f, "  Refresh Token: [Set]")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        fs,
        os::unix::fs::PermissionsExt,
    };
    use tempfile::TempDir;

    /// Helper function to set up a temporary config directory
    fn setup_config_dir() -> (PathBuf, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        unsafe {
            env::set_var("HOME", temp_dir.path());
            // Clear XDG_CONFIG_HOME to ensure we use ~/.config
            env::remove_var("XDG_CONFIG_HOME");
        }
        (
            temp_dir.path().join(".config").join(CONFIG_DIR_NAME),
            temp_dir,
        )
    }

    /// Helper function to create a read-only directory
    fn create_readonly_dir(path: &PathBuf) -> std::io::Result<()> {
        fs::create_dir_all(path)?;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o555); // Read and execute only
        fs::set_permissions(path, perms)
    }

    /// Helper function to create a read-only file
    fn create_readonly_file(path: &PathBuf) -> std::io::Result<()> {
        fs::write(path, "")?;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o444); // Read only
        fs::set_permissions(path, perms)
    }

    #[test]
    fn test_write_and_read_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Use a fixed timestamp for testing
        let fixed_timestamp = DateTime::from_timestamp(1672502400, 0).unwrap(); // 2022-12-31 16:00:00 UTC

        let config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                expires_at: fixed_timestamp,
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };

        // Test writing
        config.write_to_file_at_dir(&config_dir).unwrap();

        // Test reading
        let read_config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert_eq!(
            read_config.auth.as_ref().unwrap().access_token,
            "test_access"
        );
        assert_eq!(
            read_config.auth.as_ref().unwrap().refresh_token,
            "test_refresh"
        );
        assert!(read_config.auth.as_ref().unwrap().wallet_address.is_none());

        // Test display format - check for key content rather than exact match due to color codes
        let formatted_cfg = format!("{read_config}");

        // Check that all the important information is present
        assert!(formatted_cfg.contains("PCL Configuration"));
        assert!(formatted_cfg.contains("Config path:"));
        assert!(formatted_cfg.contains("pcl/config.toml"));
        assert!(formatted_cfg.contains("User: unknown"));
        assert!(formatted_cfg.contains("2022-12-31 16:00:00 UTC"));
        assert!(formatted_cfg.contains("Access Token: [Set]"));
        assert!(formatted_cfg.contains("Refresh Token: [Set]"));
    }

    #[test]
    fn rpc_map_roundtrips_through_toml() {
        let mut config = CliConfig::default();
        config.rpc.insert(
            "84532".to_string(),
            RpcEndpoint {
                url: "https://sepolia.base.org".to_string(),
                confirmations: Some(3),
            },
        );
        config.rpc.insert(
            "31337".to_string(),
            RpcEndpoint {
                url: "http://127.0.0.1:8545".to_string(),
                confirmations: None,
            },
        );

        let serialized = toml::to_string(&config).unwrap();
        let deserialized: CliConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized, config);
        assert_eq!(
            deserialized.rpc_endpoint(84532).unwrap().confirmations,
            Some(3)
        );
        assert_eq!(
            deserialized.rpc_endpoint(31337).unwrap().url,
            "http://127.0.0.1:8545"
        );
        assert!(deserialized.rpc_endpoint(1).is_none());
    }

    #[test]
    fn legacy_config_without_rpc_section_still_parses() {
        let config: CliConfig = toml::from_str(
            r#"
[auth]
access_token = "a"
refresh_token = "r"
expires_at = 1672502400
"#,
        )
        .unwrap();
        assert!(config.rpc.is_empty());
        assert_eq!(config.auth.unwrap().access_token, "a");
    }

    #[test]
    fn empty_rpc_map_is_not_serialized() {
        let serialized = toml::to_string(&CliConfig::default()).unwrap();
        assert!(!serialized.contains("rpc"));
    }

    #[test]
    fn config_show_redacts_rpc_urls() {
        let mut config = CliConfig::default();
        config.rpc.insert(
            "84532".to_string(),
            RpcEndpoint {
                url: "https://user:hunter2@rpc.example.com/v2/apikey123?token=sekrit".to_string(),
                confirmations: Some(3),
            },
        );

        let envelope = config_show_envelope(&config, &CliArgs::default());
        let rendered = envelope.to_string();
        assert!(!rendered.contains("hunter2"));
        assert!(!rendered.contains("apikey123"));
        assert!(!rendered.contains("sekrit"));
        assert_eq!(
            envelope["data"]["rpc"]["84532"]["host"],
            "https://rpc.example.com"
        );
        assert_eq!(envelope["data"]["rpc"]["84532"]["configured"], true);
        assert_eq!(envelope["data"]["rpc"]["84532"]["confirmations"], 3);

        // The human formatter is redacted the same way.
        let display = config.to_string();
        assert!(display.contains("https://rpc.example.com"));
        assert!(!display.contains("hunter2"));
        assert!(!display.contains("apikey123"));
    }

    #[test]
    fn redacted_rpc_host_keeps_scheme_host_and_port() {
        assert_eq!(
            redacted_rpc_host("http://127.0.0.1:8545"),
            "http://127.0.0.1:8545"
        );
        assert_eq!(
            redacted_rpc_host("https://mainnet.infura.io/v3/SECRET"),
            "https://mainnet.infura.io"
        );
        assert_eq!(redacted_rpc_host("not a url"), "<unparseable-url>");
    }

    /// A config carrying credentials, as another process would have left it.
    fn authenticated_config(access: &str, refresh: &str) -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: access.to_string(),
                refresh_token: refresh.to_string(),
                expires_at: DateTime::from_timestamp(4_102_444_800, 0).expect("timestamp"),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
                issuer_platform_url: Some("https://linea.phylax.systems".to_string()),
            }),
            ..CliConfig::default()
        }
    }

    /// Recording a selected platform must not roll back an update that landed
    /// before the merge started.
    ///
    /// The interactive selector can sit open indefinitely, and another `pcl` may
    /// rotate the refresh token while it does. Writing this process's pre-prompt
    /// snapshot would discard the rotation and leave a refresh token the
    /// platform has already invalidated.
    #[tokio::test]
    async fn merging_a_selected_platform_keeps_an_already_rotated_token() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };

        let stale = authenticated_config("old-access", "old-refresh");
        authenticated_config("rotated-access", "rotated-refresh")
            .write_to_file(&cli_args)
            .expect("another process writes the rotated pair");

        let mut config = stale.clone();
        config.platform_url = Some("https://ethereum.phylax.systems".to_string());
        config
            .merge_selected_platform(&cli_args)
            .await
            .expect("merge the selection");

        let on_disk = CliConfig::read_from_file(&cli_args).expect("read merged config");
        let auth = on_disk.auth.as_ref().expect("credentials survive");
        assert_eq!(
            auth.refresh_token, "rotated-refresh",
            "the concurrently rotated refresh token must survive"
        );
        assert_eq!(auth.access_token, "rotated-access");
        assert_eq!(
            on_disk.platform_url.as_deref(),
            Some("https://ethereum.phylax.systems"),
            "the selection still has to be recorded"
        );
        assert_eq!(
            config, on_disk,
            "this process adopts the merged state for the rest of the run"
        );
    }

    /// The rotation that overlaps the merge is the dangerous one.
    ///
    /// Reloading before the write only protects updates that finished before the
    /// reload. Without a lock, a refresh that writes its new pair *after* the
    /// reload and before the merge's own write is silently overwritten, throwing
    /// away the only valid refresh token — and the sibling test above cannot
    /// catch it, because there the rotation is already on disk when the merge
    /// begins.
    ///
    /// The lock itself is the barrier here: holding it forces the merge to wait,
    /// and the rotation is written while it waits, so the rotation lands strictly
    /// between the merge being asked for and the merge reading the file.
    #[tokio::test]
    async fn merging_a_selected_platform_waits_for_an_overlapping_rotation() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };

        let stale = authenticated_config("old-access", "old-refresh");
        stale
            .write_to_file(&cli_args)
            .expect("the pre-prompt state is on disk");

        // Stand in for a concurrent `refresh_stored_auth`, which holds this same
        // lock across its own read-modify-write.
        let lock = ConfigLock::acquire(&cli_args)
            .await
            .expect("the refresher takes the lock first");

        let mut config = stale.clone();
        config.platform_url = Some("https://ethereum.phylax.systems".to_string());
        let merge_args = cli_args.clone();
        let merge = tokio::spawn(async move {
            config.merge_selected_platform(&merge_args).await?;
            Ok::<CliConfig, ConfigError>(config)
        });

        // The merge cannot have read anything yet: it is parked on the lock.
        tokio::time::sleep(CONFIG_LOCK_POLL_INTERVAL * 3).await;
        assert!(
            !merge.is_finished(),
            "the merge must block until the refresher releases the lock"
        );
        authenticated_config("rotated-access", "rotated-refresh")
            .write_to_file(&cli_args)
            .expect("the refresher writes its rotated pair");
        drop(lock);

        let merged = merge
            .await
            .expect("merge task joins")
            .expect("merge the selection");

        let on_disk = CliConfig::read_from_file(&cli_args).expect("read merged config");
        let auth = on_disk.auth.as_ref().expect("credentials survive");
        assert_eq!(
            auth.refresh_token, "rotated-refresh",
            "a rotation overlapping the merge must survive it"
        );
        assert_eq!(auth.access_token, "rotated-access");
        assert_eq!(
            on_disk.platform_url.as_deref(),
            Some("https://ethereum.phylax.systems"),
            "the selection still has to be recorded"
        );
        assert_eq!(merged, on_disk);
    }

    /// A lock left behind by a killed process must not wedge the CLI forever.
    ///
    /// Runs on a paused clock, which tokio auto-advances whenever every task is
    /// parked on a timer, so the whole `CONFIG_LOCK_TIMEOUT` budget elapses
    /// instantly instead of costing the suite 30 real seconds.
    #[tokio::test(start_paused = true)]
    async fn a_held_config_lock_times_out_rather_than_blocking_forever() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let _held = ConfigLock::acquire(&cli_args).await.expect("first acquire");

        let error = ConfigLock::acquire(&cli_args)
            .await
            .expect_err("the lock is still held, so acquiring must fail");
        assert!(
            matches!(error, ConfigError::LockTimeout),
            "expected a lock timeout, got {error:?}"
        );
    }

    #[test]
    fn set_rpc_rejects_invalid_urls() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 1,
                url: "not a url".to_string(),
                confirmations: None,
            },
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_err());

        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 1,
                url: "ws://example.com".to_string(),
                confirmations: None,
            },
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_err());
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn set_rpc_parse_errors_do_not_expose_url_credentials() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 1,
                url: "https://user:hunter2@rpc.example.com:invalid/v2/apikey123?token=sekrit"
                    .to_string(),
                confirmations: None,
            },
        };

        let error = args
            .run(&mut config, &CliArgs::default())
            .unwrap_err()
            .to_string();

        assert!(error.contains("<unparseable-url>"));
        for secret in ["user", "hunter2", "apikey123", "sekrit"] {
            assert!(!error.contains(secret), "error exposed {secret:?}: {error}");
        }
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn set_and_unset_rpc_mutate_config() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 84532,
                url: "https://sepolia.base.org".to_string(),
                confirmations: Some(2),
            },
        };
        args.run(&mut config, &CliArgs::default()).unwrap();
        assert_eq!(
            config.rpc_endpoint(84532),
            Some(&RpcEndpoint {
                url: "https://sepolia.base.org".to_string(),
                confirmations: Some(2),
            })
        );

        let args = ConfigArgs {
            command: ConfigCommand::UnsetRpc { chain_id: 84532 },
        };
        args.run(&mut config, &CliArgs::default()).unwrap();
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn write_to_file_if_unchanged_preserves_newer_disk_auth() {
        let temp_dir = TempDir::new().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let old_config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };
        let stale_process_config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "normalized_old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: DateTime::from_timestamp(4102444800, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };
        let newer_config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "new_access".to_string(),
                refresh_token: "new_refresh".to_string(),
                expires_at: DateTime::from_timestamp(4102444800, 0).unwrap(),
                refresh_expires_at: Some(DateTime::from_timestamp(4105036800, 0).unwrap()),
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };

        old_config.write_to_file(&cli_args).unwrap();
        newer_config.write_to_file(&cli_args).unwrap();

        let wrote = stale_process_config
            .write_to_file_if_unchanged(&cli_args, &old_config)
            .unwrap();

        assert!(!wrote);
        let persisted = CliConfig::read_from_file(&cli_args).unwrap();
        assert_eq!(
            persisted.auth.as_ref().unwrap().refresh_token,
            "new_refresh"
        );
    }

    #[test]
    fn platform_url_round_trips_and_is_omitted_when_unset() {
        let (config_dir, _temp_dir) = setup_config_dir();

        let config = CliConfig {
            auth: None,
            platform_url: Some("https://custom.phylax.example".to_string()),
            rpc: BTreeMap::default(),
        };
        config.write_to_file_at_dir(&config_dir).unwrap();
        let read_config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert_eq!(
            read_config.platform_url.as_deref(),
            Some("https://custom.phylax.example")
        );

        CliConfig::default()
            .write_to_file_at_dir(&config_dir)
            .unwrap();
        let raw = fs::read_to_string(config_dir.join(CONFIG_FILE)).unwrap();
        assert!(!raw.contains("platform_url"));
    }

    #[test]
    fn config_without_platform_url_parses_as_none() {
        let (config_dir, _temp_dir) = setup_config_dir();
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join(CONFIG_FILE), "").unwrap();

        let config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert!(config.platform_url.is_none());
    }

    #[test]
    fn test_read_nonexistent_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Try reading without creating a file
        let config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_user_auth_display() {
        let auth = UserAuth {
            issuer_platform_url: None,
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(), // 2022-12-31 16:00:00 UTC
            refresh_expires_at: None,
            user_id: None,
            wallet_address: None,
            email: Some("test@example.com".to_string()),
        };

        let display = format!("{auth}");
        assert!(display.contains("User: test@example.com"));
        assert!(display.contains("Token Expired at"));
        assert!(display.contains("Access Token: [Set]"));
        assert!(display.contains("Refresh Token: [Set]"));
    }

    #[test]
    fn test_display_name_priority() {
        let expires = DateTime::from_timestamp(0, 0).unwrap();

        // Non-zero wallet address takes priority over everything
        let with_addr = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: Some(Address::from_slice(&[1; 20])),
            email: Some("test@example.com".to_string()),
            user_id: Some(Uuid::nil()),
        };
        assert_eq!(
            with_addr.display_name(),
            "0x0101010101010101010101010101010101010101"
        );

        // Email is next priority when no wallet address
        let with_email = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: None,
            email: Some("test@example.com".to_string()),
            user_id: Some(Uuid::nil()),
        };
        assert_eq!(with_email.display_name(), "test@example.com");

        // User ID is fallback when no address or email
        let with_id = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: None,
            email: None,
            user_id: Some(Uuid::nil()),
        };
        assert_eq!(
            with_id.display_name(),
            "00000000-0000-0000-0000-000000000000"
        );

        // "unknown" when nothing is set
        let bare = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: None,
            email: None,
            user_id: None,
        };
        assert_eq!(bare.display_name(), "unknown");
    }

    #[test]
    fn extracts_expiry_from_jwt_access_token() {
        let auth = UserAuth {
            issuer_platform_url: None,
            access_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string(),
            refresh_token: String::new(),
            expires_at: DateTime::from_timestamp(0, 0).unwrap(),
            refresh_expires_at: None,
            wallet_address: None,
            email: None,
            user_id: None,
        };

        assert_eq!(
            auth.access_token_expires_at(),
            DateTime::from_timestamp(4102444800, 0)
        );
    }

    #[test]
    fn normalizes_legacy_device_session_expiry_from_access_token_exp() {
        let mut config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string(),
                refresh_token: "refresh".to_string(),
                expires_at: DateTime::from_timestamp(1, 0).unwrap(),
                refresh_expires_at: None,
                wallet_address: None,
                email: None,
                user_id: None,
            }),
            platform_url: None,
        };

        assert!(config.normalize_auth_expiry_from_access_token());
        assert_eq!(
            config.auth.unwrap().expires_at,
            DateTime::from_timestamp(4102444800, 0).unwrap()
        );
    }

    #[test]
    fn test_config_args_show() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::Show,
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_ok());
    }

    #[test]
    fn test_config_args_delete() {
        let mut config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "test".to_string(),
                refresh_token: "test".to_string(),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };
        let args = ConfigArgs {
            command: ConfigCommand::Delete,
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_ok());
        assert!(config.auth.is_none());
    }

    #[test]
    fn config_show_envelope_hides_tokens_and_reports_expiry() {
        let args = CliArgs {
            config_dir: Some(PathBuf::from("/tmp/pcl-test-config")),
            ..Default::default()
        };
        let config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "secret-access".to_string(),
                refresh_token: "secret-refresh".to_string(),
                expires_at: Utc::now() + chrono::Duration::minutes(10),
                refresh_expires_at: None,
                user_id: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
                wallet_address: None,
                email: Some("test@example.com".to_string()),
            }),
            platform_url: None,
        };

        let envelope = config_show_envelope(&config, &args);

        assert_eq!(envelope["status"], "ok");
        assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
        assert_eq!(envelope["pcl_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            envelope["data"]["config_path"],
            "/tmp/pcl-test-config/config.toml"
        );
        assert_eq!(envelope["data"]["auth"]["authenticated"], true);
        assert_eq!(envelope["data"]["auth"]["user"], "test@example.com");
        assert_eq!(envelope["data"]["auth"]["token_valid"], true);
        assert_eq!(envelope["data"]["auth"]["expired"], false);
        assert!(envelope["data"]["auth"]["seconds_remaining"].is_number());
        let serialized = serde_json::to_string(&envelope).unwrap();
        assert!(!serialized.contains("secret-access"));
        assert!(!serialized.contains("secret-refresh"));
    }

    #[test]
    fn test_write_to_file_permission_error() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create a read-only directory
        let mut perms = std::fs::metadata(&temp_dir).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&temp_dir, perms).unwrap();

        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(&temp_dir.path().to_path_buf());

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }

    #[test]
    fn test_read_from_file_invalid_toml() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_file, "invalid toml content").unwrap();

        let result = CliConfig::read_from_file_at_dir(&config_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_display_empty_config() {
        let config = CliConfig::default();
        let display = format!("{config}");
        assert!(display.contains("Not authenticated"));
    }

    #[test]
    fn test_user_auth_serialization() {
        let auth = UserAuth {
            issuer_platform_url: None,
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
            refresh_expires_at: Some(DateTime::from_timestamp(1675094400, 0).unwrap()),
            user_id: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
            wallet_address: Some(Address::from_slice(&[0; 20])),
            email: Some("test@example.com".to_string()),
        };

        let serialized = toml::to_string(&auth).unwrap();
        let deserialized: UserAuth = toml::from_str(&serialized).unwrap();

        assert_eq!(auth.access_token, deserialized.access_token);
        assert_eq!(auth.refresh_token, deserialized.refresh_token);
        assert_eq!(auth.wallet_address, deserialized.wallet_address);
        assert_eq!(auth.expires_at, deserialized.expires_at);
    }

    #[test]
    fn test_ensure_writable_directory_success() {
        let (config_dir, _temp_dir) = setup_config_dir();
        assert!(CliConfig::ensure_writable_directory(&config_dir).is_ok());
    }

    #[test]
    fn test_ensure_writable_directory_readonly() {
        let (config_dir, _temp_dir) = setup_config_dir();
        create_readonly_dir(&config_dir).unwrap();

        let result = CliConfig::ensure_writable_directory(&config_dir);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }

    #[test]
    fn test_ensure_writable_file_success() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(&config_file, "").unwrap();

        assert!(CliConfig::ensure_writable_file(&config_file).is_ok());
    }

    #[test]
    fn test_ensure_writable_file_readonly() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        create_readonly_file(&config_file).unwrap();

        let result = CliConfig::ensure_writable_file(&config_file);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("read-only"));
    }

    #[test]
    fn test_ensure_writable_file_nonexistent() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);

        assert!(CliConfig::ensure_writable_file(&config_file).is_ok());
    }

    #[test]
    fn test_write_to_file_at_dir_permission_denied() {
        let (config_dir, _temp_dir) = setup_config_dir();
        create_readonly_dir(&config_dir).unwrap();

        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(&config_dir);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }

    #[test]
    fn test_write_to_file_at_dir_readonly_file() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        create_readonly_file(&config_file).unwrap();

        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(&config_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("read-only"));
    }

    #[test]
    fn test_write_to_file_at_dir_success() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config = CliConfig::default();
        assert!(config.write_to_file_at_dir(&config_dir).is_ok());
    }
}
