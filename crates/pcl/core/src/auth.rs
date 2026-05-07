use crate::{
    DEFAULT_PLATFORM_URL,
    api::{
        toon_string,
        with_envelope_metadata,
    },
    config::{
        AUTH_EXPIRES_SOON_SECONDS,
        CliConfig,
        UserAuth,
        access_token_expires_at,
    },
    error::{
        AuthError,
        ConfigError,
    },
};
use alloy_primitives::Address;
use color_eyre::Result;
use colored::Colorize;
use dapp_api_client::generated::client::{
    Client as GeneratedClient,
    types::{
        GetCliAuthCodeResponse,
        GetCliAuthStatusResponse,
    },
};
use indicatif::{
    ProgressBar,
    ProgressStyle,
};
use pcl_common::args::CliArgs;
use reqwest::header::{
    CONTENT_TYPE,
    HeaderMap,
    HeaderName,
    HeaderValue,
    RETRY_AFTER,
};
use serde::Deserialize;
use serde_json::{
    Value,
    json,
};
use std::{
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
};
use tokio::time::{
    Duration,
    Instant,
    sleep,
};
use uuid::Uuid;

/// Initial interval between authentication status checks.
const INITIAL_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Maximum interval between authentication status checks.
const MAX_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Overall polling budget, matching the previous 150 x 2s behavior.
const POLL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Maximum time to wait for another local CLI process to finish rotating auth.
const REFRESH_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll interval while waiting on the local refresh lock.
const REFRESH_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub struct RefreshOutcome {
    pub refreshed: bool,
    pub reason: &'static str,
    pub request_id: Option<String>,
}

#[derive(Deserialize)]
struct RefreshResponse {
    token: String,
    refresh_token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
    refresh_expires_at: chrono::DateTime<chrono::Utc>,
}

struct RefreshErrorDetails {
    code: Option<String>,
    message: Option<String>,
}

struct RefreshLock {
    path: PathBuf,
}

/// Authentication commands for the PCL CLI
#[derive(clap::Parser)]
#[command(about = "Authenticate the CLI with your Credible Layer Platform account")]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommands,

    #[arg(
        short = 'u',
        long = "auth-url",
        env = "PCL_AUTH_URL",
        help = "Base URL for authentication service. Defaults to PCL_AUTH_URL, then PCL_API_URL, then the production app URL"
    )]
    pub auth_url: Option<url::Url>,
}

/// Available authentication subcommands
#[derive(clap::Subcommand)]
#[command(about = "Authentication operations")]
pub enum AuthSubcommands {
    /// Ensure auth is usable, or return a one-envelope login challenge
    #[command(
        long_about = "Checks whether auth is usable. If not, returns a structured device-login challenge without waiting.",
        after_help = "Examples:\n  pcl auth ensure\n  pcl auth ensure --json\n  pcl auth ensure --force --json"
    )]
    Ensure {
        #[arg(long, help = "Return a fresh login challenge even when auth is usable")]
        force: bool,
    },

    /// Login to PCL
    #[command(
        long_about = "Initiates the login process. Opens a browser window for authentication.",
        after_help = "Examples:\n  pcl auth login\n  pcl auth login --force\n  pcl auth login --no-wait --json"
    )]
    Login {
        #[arg(
            long,
            help = "Start a fresh login even when the stored token is still valid"
        )]
        force: bool,
        #[arg(
            long,
            help = "Return a login challenge without waiting for verification"
        )]
        no_wait: bool,
    },

    /// Poll a pending device-login session once
    #[command(
        long_about = "Checks a device-login session once and stores credentials if verification completed.",
        after_help = "Example: pcl auth poll --session-id <uuid> --device-secret <secret> --expires-at <rfc3339> --json"
    )]
    Poll {
        #[arg(
            long,
            help = "Device-login session ID from auth ensure/login --no-wait"
        )]
        session_id: Uuid,
        #[arg(long, help = "Device-login secret from auth ensure/login --no-wait")]
        device_secret: String,
        #[arg(long, help = "Device-login expiry from auth ensure/login --no-wait")]
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    },

    /// Refresh auth when possible, or return a login challenge when refresh is unavailable
    #[command(
        long_about = "Refreshes auth non-interactively by rotating the stored CLI refresh token; returns a structured login challenge when no refreshable session exists.",
        after_help = "Example: pcl auth refresh --json"
    )]
    Refresh {
        #[arg(
            long,
            help = "Return a login challenge even when the token is still valid"
        )]
        force: bool,
    },

    /// Logout from PCL
    #[command(
        long_about = "Attempts remote logout with the stored access token, then removes local authentication credentials. Use --local to skip the remote logout attempt.",
        after_help = "Examples:\n  pcl auth logout\n  pcl auth logout --local"
    )]
    Logout {
        #[arg(
            long,
            help = "Only remove local credentials; skip the remote logout request"
        )]
        local: bool,
    },

    /// Check current authentication status
    #[command(
        long_about = "Displays whether you're currently logged in and shows the connected identity if authenticated.",
        after_help = "Example: pcl auth status"
    )]
    Status,
}

impl AuthCommand {
    pub fn can_run_without_valid_config(&self) -> bool {
        matches!(
            self.command,
            AuthSubcommands::Ensure { .. }
                | AuthSubcommands::Login { .. }
                | AuthSubcommands::Poll { .. }
                | AuthSubcommands::Refresh { .. }
                | AuthSubcommands::Logout { .. }
        )
    }

    pub fn should_force_config_write(&self) -> bool {
        matches!(
            self.command,
            AuthSubcommands::Login { .. }
                | AuthSubcommands::Poll { .. }
                | AuthSubcommands::Logout { .. }
        )
    }

    fn effective_auth_url(&self) -> url::Url {
        if let Some(auth_url) = &self.auth_url {
            return auth_url.clone();
        }
        if let Ok(api_url) = std::env::var("PCL_API_URL")
            && let Ok(parsed) = api_url.parse()
        {
            return parsed;
        }
        DEFAULT_PLATFORM_URL
            .parse()
            .expect("default platform URL is valid")
    }

    /// Execute the authentication command
    pub async fn run(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
    ) -> Result<(), AuthError> {
        match &self.command {
            AuthSubcommands::Ensure { force } => {
                self.ensure(config, cli_args, json_output, *force).await
            }
            AuthSubcommands::Login { force, no_wait } => {
                self.login(config, json_output, *force, *no_wait).await
            }
            AuthSubcommands::Poll {
                session_id,
                device_secret,
                expires_at,
            } => {
                self.poll(
                    config,
                    json_output,
                    session_id,
                    device_secret,
                    expires_at.to_owned(),
                )
                .await
            }
            AuthSubcommands::Refresh { force } => {
                self.refresh(config, cli_args, json_output, *force).await
            }
            AuthSubcommands::Logout { local } => {
                let logout = self.remote_logout(config, *local).await;
                Self::logout(config);
                Self::print_output(
                    &json!({
                        "status": "ok",
                        "data": {
                            "authenticated": false,
                            "platform_url": self.effective_auth_url().as_str(),
                            "local_credentials_removed": true,
                            "remote_logout": logout,
                        },
                        "next_actions": ["pcl auth login"],
                    }),
                    json_output,
                )?;
                Ok(())
            }
            AuthSubcommands::Status => self.status(config, json_output),
        }
    }

    async fn ensure(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
        force: bool,
    ) -> Result<(), AuthError> {
        let mut refresh_challenge_reason = None;
        if !force && let Some(auth) = &config.auth {
            let now = chrono::Utc::now();
            let seconds_remaining = (auth.expires_at - now).num_seconds();
            if auth.expires_at > now && seconds_remaining > AUTH_EXPIRES_SOON_SECONDS {
                Self::print_output(&self.status_envelope(config), json_output)?;
                return Ok(());
            }
        }

        if config
            .auth
            .as_ref()
            .is_some_and(|auth| !auth.refresh_token.trim().is_empty())
        {
            match refresh_stored_auth(config, &self.effective_auth_url(), cli_args, force).await {
                Ok(outcome) => {
                    Self::print_output(&Self::refresh_envelope(config, &outcome), json_output)?;
                    return Ok(());
                }
                Err(AuthError::RefreshRejected { .. }) => {
                    refresh_challenge_reason = Some(AuthChallengeReason::InvalidRefresh);
                }
                Err(AuthError::RefreshEndpointNotFound { .. }) => {
                    refresh_challenge_reason = Some(AuthChallengeReason::RefreshUnavailable);
                }
                Err(AuthError::NoRefreshableSession | AuthError::MissingRefreshToken) => {}
                Err(error) => return Err(error),
            }
        }

        let reason =
            refresh_challenge_reason.unwrap_or_else(|| auth_challenge_reason(config, force));
        let auth_response = Self::request_auth_code(&self.api_client()).await?;
        Self::print_output(
            &self.login_challenge_envelope(&auth_response, reason),
            json_output,
        )
    }

    async fn refresh(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        json_output: bool,
        force: bool,
    ) -> Result<(), AuthError> {
        if !force && let Some(auth) = &config.auth {
            let now = chrono::Utc::now();
            let seconds_remaining = (auth.expires_at - now).num_seconds();
            if auth.expires_at > now && seconds_remaining > AUTH_EXPIRES_SOON_SECONDS {
                Self::print_output(
                    &json!({
                        "status": "ok",
                            "data": {
                            "refreshed": false,
                            "refresh_supported": true,
                            "reason": "token_still_valid",
                            "authenticated": true,
                            "expires_at": auth.expires_at.to_rfc3339(),
                            "seconds_remaining": seconds_remaining,
                            "expires_in_seconds": seconds_remaining,
                            "refresh_expires_at": auth.refresh_expires_at.map(|expires_at| expires_at.to_rfc3339()),
                            "refresh_seconds_remaining": auth.refresh_expires_at.map(|expires_at| (expires_at - now).num_seconds()),
                        },
                        "next_actions": ["pcl account", "pcl projects --home"],
                    }),
                    json_output,
                )?;
                return Ok(());
            }
        }

        let mut refresh_challenge_reason = None;
        match refresh_stored_auth(config, &self.effective_auth_url(), cli_args, force).await {
            Ok(outcome) => {
                Self::print_output(&Self::refresh_envelope(config, &outcome), json_output)?;
                return Ok(());
            }
            Err(AuthError::RefreshRejected { .. }) => {
                refresh_challenge_reason = Some(AuthChallengeReason::InvalidRefresh);
            }
            Err(AuthError::NoRefreshableSession | AuthError::MissingRefreshToken) => {}
            Err(error) => return Err(error),
        }

        let auth_response = Self::request_auth_code(&self.api_client()).await?;
        let reason = refresh_challenge_reason.unwrap_or(AuthChallengeReason::Missing);
        Self::print_output(
            &self.login_challenge_envelope(&auth_response, reason),
            json_output,
        )
    }

    /// Initiate the login process and wait for user authentication
    async fn login(
        &self,
        config: &mut CliConfig,
        json_output: bool,
        force: bool,
        no_wait: bool,
    ) -> Result<(), AuthError> {
        let mut expired_auth = None;
        if let Some(auth) = &config.auth {
            if auth.expires_at > chrono::Utc::now() && !force {
                Self::print_output(&self.status_envelope(config), json_output)?;
                return Ok(());
            }
            if auth.expires_at <= chrono::Utc::now() {
                expired_auth = Some(auth.expires_at);
            }
            if auth.expires_at <= chrono::Utc::now() && !json_output {
                println!(
                    "{} Stored auth token expired at {}. Starting a fresh login.",
                    "⚠️".yellow(),
                    auth.expires_at.to_rfc3339()
                );
            }
        }

        let client = self.api_client();
        let auth_response = Self::request_auth_code(&client).await?;
        if no_wait {
            Self::print_output(
                &self.login_challenge_envelope(
                    &auth_response,
                    if force {
                        AuthChallengeReason::Forced
                    } else {
                        auth_challenge_reason(config, false)
                    },
                ),
                json_output,
            )?;
            return Ok(());
        }
        if json_output {
            Self::print_json_event(
                &self.login_instructions_envelope(&auth_response, expired_auth),
            )?;
            self.wait_for_verification(config, &client, &auth_response, true)
                .await?;
            let mut output = self.status_envelope(config);
            if let Some(object) = output.as_object_mut() {
                object.insert("event".to_string(), json!("auth.login_complete"));
                object.insert("terminal".to_string(), json!(true));
                object.insert("output_mode".to_string(), json!("jsonl"));
            }
            Self::print_json_event(&output)?;
            return Ok(());
        }

        self.display_login_instructions(&auth_response);
        self.wait_for_verification(config, &client, &auth_response, json_output)
            .await
    }

    // Helper to create a new API client with the base URL set
    fn api_client(&self) -> GeneratedClient {
        let mut base = self.effective_auth_url();
        base.set_path("/api/v1");
        GeneratedClient::new(base.as_str())
    }

    /// Request an authentication code from the server
    async fn request_auth_code(
        client: &GeneratedClient,
    ) -> Result<GetCliAuthCodeResponse, AuthError> {
        client
            .get_cli_auth_code()
            .await
            .map(dapp_api_client::generated::client::ResponseValue::into_inner)
            .map_err(|e| AuthError::AuthRequestFailed(e.to_string()))
    }

    /// Display login URL and code to the user, attempting to open the browser automatically
    fn display_login_instructions(&self, auth_response: &GetCliAuthCodeResponse) {
        let mut device_url = self.effective_auth_url();
        device_url.set_path("/device");
        device_url
            .query_pairs_mut()
            .append_pair("session_id", &auth_response.session_id.to_string());
        let url = device_url.as_str();

        if Self::should_open_browser() && open::that(url).is_ok() {
            println!(
                "\n{} Opening browser for authentication...\n\n🔗 {}\n📝 {}\n",
                "🌐".green(),
                url.white(),
                format!("Code: {}", *auth_response.code).green().bold()
            );
        } else {
            println!(
                "\nTo authenticate, please visit:\n\n🔗 {}\n📝 {}\n",
                url.white(),
                format!("Code: {}", *auth_response.code).green().bold()
            );
        }
    }

    fn login_instructions_envelope(
        &self,
        auth_response: &GetCliAuthCodeResponse,
        previous_token_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Value {
        let mut device_url = self.effective_auth_url();
        device_url.set_path("/device");
        device_url
            .query_pairs_mut()
            .append_pair("session_id", &auth_response.session_id.to_string());
        with_envelope_metadata(json!({
            "status": "pending",
            "event": "auth.login_instructions",
            "terminal": false,
            "output_mode": "jsonl",
            "data": {
                "state": "login_instructions",
                "device_url": device_url.as_str(),
                "code": auth_response.code.as_str(),
                "session_id": auth_response.session_id.to_string(),
                "device_secret": auth_response.device_secret.as_str(),
                "expires_at": auth_response.expires_at.to_rfc3339(),
                "previous_token_expired_at": previous_token_expires_at.map(|expires_at| expires_at.to_rfc3339()),
                "browser_opened": false,
                "waiting_for_verification": true,
                "poll_command": self.poll_command(auth_response, true),
            },
            "next_actions": [
                "Open data.device_url and enter data.code",
                "Wait for this command to finish",
            ],
        }))
    }

    fn login_challenge_envelope(
        &self,
        auth_response: &GetCliAuthCodeResponse,
        reason: AuthChallengeReason,
    ) -> Value {
        let mut device_url = self.effective_auth_url();
        device_url.set_path("/device");
        device_url
            .query_pairs_mut()
            .append_pair("session_id", &auth_response.session_id.to_string());
        with_envelope_metadata(json!({
            "status": "action_required",
            "data": {
                "state": "login_required",
                "reason": reason.as_str(),
                "requires_user": true,
                "refresh_supported": reason.refresh_supported(),
                "refresh_attempted": reason.refresh_attempted(),
                "device_url": device_url.as_str(),
                "code": auth_response.code.as_str(),
                "session_id": auth_response.session_id.to_string(),
                "device_secret": auth_response.device_secret.as_str(),
                "expires_at": auth_response.expires_at.to_rfc3339(),
                "poll_command": self.poll_command(auth_response, true),
                "wait_command": "pcl auth login --force --json",
            },
            "next_actions": [
                "Open data.device_url and enter data.code",
                "Run data.poll_command until status is ok or error",
            ],
        }))
    }

    fn refresh_envelope(config: &CliConfig, outcome: &RefreshOutcome) -> Value {
        let Some(auth) = &config.auth else {
            return with_envelope_metadata(json!({
                "status": "action_required",
                "data": {
                    "state": "login_required",
                    "reason": "missing_auth",
                    "refresh_supported": true,
                    "refresh_attempted": true,
                    "authenticated": false,
                },
                "next_actions": ["pcl auth login"],
            }));
        };
        let now = chrono::Utc::now();
        let seconds_remaining = (auth.expires_at - now).num_seconds();
        let refresh_seconds_remaining = auth
            .refresh_expires_at
            .map(|expires_at| (expires_at - now).num_seconds());
        with_envelope_metadata(json!({
            "status": "ok",
            "data": {
                "state": "authenticated",
                "authenticated": true,
                "user": auth.display_name(),
                "user_id": auth.user_id.map(|id| id.to_string()),
                "wallet_address": auth.wallet_address.map(|address| address.to_string()),
                "email": auth.email.as_deref(),
                "refreshed": outcome.refreshed,
                "refresh_supported": true,
                "refresh_attempted": outcome.refreshed,
                "reason": outcome.reason,
                "request_id": outcome.request_id,
                "token_present": !auth.access_token.is_empty(),
                "refresh_token_present": !auth.refresh_token.is_empty(),
                "expires_at": auth.expires_at.to_rfc3339(),
                "seconds_remaining": seconds_remaining,
                "expires_in_seconds": seconds_remaining,
                "refresh_expires_at": auth.refresh_expires_at.map(|expires_at| expires_at.to_rfc3339()),
                "refresh_seconds_remaining": refresh_seconds_remaining,
            },
            "next_actions": ["pcl account", "pcl projects --home"],
        }))
    }

    fn poll_command(&self, auth_response: &GetCliAuthCodeResponse, json_output: bool) -> String {
        let json_flag = if json_output { " --json" } else { "" };
        let auth_url = self.effective_auth_url();
        format!(
            "pcl auth --auth-url {} poll --session-id {} --device-secret {} --expires-at {}{}",
            shell_quote(auth_url.as_str()),
            auth_response.session_id,
            shell_quote(&auth_response.device_secret),
            shell_quote(&auth_response.expires_at.to_rfc3339()),
            json_flag
        )
    }

    fn should_open_browser() -> bool {
        !cfg!(test) && std::env::var_os("PCL_AUTH_NO_BROWSER").is_none()
    }

    /// Wait for the user to complete the authentication process
    async fn wait_for_verification(
        &self,
        config: &mut CliConfig,
        client: &GeneratedClient,
        auth_response: &GetCliAuthCodeResponse,
        json_output: bool,
    ) -> Result<(), AuthError> {
        let spinner = if json_output {
            ProgressBar::hidden()
        } else {
            ProgressBar::new_spinner()
        };
        spinner.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
                .template("{spinner} {msg}")
                .map_err(|e| {
                    AuthError::InvalidAuthData(format!("Failed to set spinner style: {e}"))
                })?,
        );
        spinner.enable_steady_tick(Duration::from_millis(80));
        spinner.set_message("Waiting for authentication...");

        let deadline = Instant::now() + POLL_TIMEOUT;
        let mut attempts = 0_u32;
        loop {
            // Stop polling once the session has expired
            if chrono::Utc::now() >= auth_response.expires_at {
                if json_output {
                    spinner.finish_and_clear();
                } else {
                    spinner.finish_with_message("❌ Session expired");
                }
                return Err(AuthError::SessionExpired);
            }

            let status = match Self::check_auth_status(
                client,
                &auth_response.device_secret,
                &auth_response.session_id,
            )
            .await
            {
                Ok(s) => s,
                // Transient errors — keep polling
                Err(AuthError::ServerError(_) | AuthError::StatusRequestFailed(_)) => {
                    spinner.tick();
                    attempts =
                        Self::sleep_before_next_poll(deadline, attempts, &auth_response.session_id)
                            .await
                            .inspect_err(|error| {
                                finish_timeout_if_needed(&spinner, json_output, error);
                            })?;
                    continue;
                }
                // Terminal errors — stop immediately
                Err(e) => {
                    if json_output {
                        spinner.finish_and_clear();
                    } else {
                        spinner.finish_with_message(format!("❌ {e}"));
                    }
                    return Err(e);
                }
            };

            if status.verified {
                if json_output {
                    spinner.finish_and_clear();
                } else {
                    spinner.finish_with_message("✅ Authentication successful!");
                }
                update_config_from_verified_status(config, status, auth_response.expires_at)?;
                if !json_output {
                    Self::display_success_message(config)?;
                }
                return Ok(());
            }

            spinner.tick();
            attempts = Self::sleep_before_next_poll(deadline, attempts, &auth_response.session_id)
                .await
                .inspect_err(|error| {
                    finish_timeout_if_needed(&spinner, json_output, error);
                })?;
        }
    }

    async fn poll(
        &self,
        config: &mut CliConfig,
        json_output: bool,
        session_id: &Uuid,
        device_secret: &str,
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<(), AuthError> {
        let status = Self::check_auth_status(&self.api_client(), device_secret, session_id).await?;
        if status.verified {
            update_config_from_verified_status(
                config,
                status,
                expires_at.unwrap_or_else(default_poll_fallback_expires_at),
            )?;
            let mut output = self.status_envelope(config);
            if let Some(object) = output.as_object_mut() {
                object.insert("event".to_string(), json!("auth.login_complete"));
                object.insert("terminal".to_string(), json!(true));
            }
            Self::print_output(&output, json_output)?;
            return Ok(());
        }

        Self::print_output(
            &json!({
                "status": "pending",
                "event": "auth.login_pending",
                "terminal": false,
                "data": {
                    "session_id": session_id.to_string(),
                    "verified": false,
                    "state": "waiting_for_user",
                },
                "next_actions": [
                    "Open the device URL from auth ensure/login --no-wait",
                    "Run this poll command again",
                ],
            }),
            json_output,
        )
    }

    /// Check authentication status using the generated client.
    async fn check_auth_status(
        client: &GeneratedClient,
        device_secret: &str,
        session_id: &Uuid,
    ) -> Result<GetCliAuthStatusResponse, AuthError> {
        client
            .get_cli_auth_status(device_secret, session_id)
            .await
            .map(dapp_api_client::generated::client::ResponseValue::into_inner)
            .map_err(AuthError::from)
    }

    async fn refresh_request(
        auth_url: &url::Url,
        refresh_token: &str,
    ) -> Result<reqwest::Response, AuthError> {
        let mut url = auth_url.clone();
        url.set_path("/api/v1/auth/refresh");
        url.set_query(None);
        reqwest::Client::new()
            .post(url)
            .header(
                HeaderName::from_static("api-version"),
                HeaderValue::from_static("1"),
            )
            .json(&json!({ "refresh_token": refresh_token }))
            .send()
            .await
            .map_err(|error| AuthError::RefreshRequestFailed(error.to_string()))
    }

    async fn remote_logout(&self, config: &CliConfig, local_only: bool) -> Value {
        if local_only {
            return json!({
                "attempted": false,
                "success": null,
                "mode": "local",
                "reason": "local_only_requested",
            });
        }

        let Some(auth) = config.auth.as_ref() else {
            return json!({
                "attempted": false,
                "success": null,
                "mode": "local",
                "reason": "no_stored_auth",
            });
        };

        let mut url = self.effective_auth_url();
        url.set_path("/api/v1/web/auth/logout");
        url.set_query(None);
        let response = reqwest::Client::new()
            .post(url)
            .bearer_auth(&auth.access_token)
            .json(&json!({}))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                return json!({
                    "attempted": true,
                    "success": false,
                    "mode": "remote",
                    "endpoint": "/api/v1/web/auth/logout",
                    "error": error.to_string(),
                });
            }
        };

        let status = response.status();
        let request_id = request_id_from_headers(response.headers());
        if status.is_success() {
            return json!({
                "attempted": true,
                "success": true,
                "mode": "remote",
                "endpoint": "/api/v1/web/auth/logout",
                "http_status": status.as_u16(),
                "request_id": request_id,
            });
        }

        let details = refresh_error_details(response).await;
        json!({
            "attempted": true,
            "success": false,
            "mode": "remote",
            "endpoint": "/api/v1/web/auth/logout",
            "http_status": status.as_u16(),
            "request_id": request_id,
            "error_code": details.code,
            "error": details.message,
        })
    }

    async fn sleep_before_next_poll(
        deadline: Instant,
        attempts: u32,
        session_id: &Uuid,
    ) -> Result<u32, AuthError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AuthError::Timeout(attempts));
        }
        let delay = std::cmp::min(poll_backoff_delay(attempts, session_id), remaining);
        sleep(delay).await;
        Ok(attempts.saturating_add(1))
    }

    /// Display success message after authentication
    fn display_success_message(config: &CliConfig) -> Result<(), AuthError> {
        let auth = config
            .auth
            .as_ref()
            .ok_or_else(|| AuthError::InvalidAuthData("Missing auth after update".to_string()))?;
        println!(
            "{}\n🔗 {}\n",
            "Authentication successful! 🎉".green().bold(),
            format!("Connected as: {}", auth.display_name()).white()
        );
        Ok(())
    }

    /// Remove authentication data from configuration
    fn logout(config: &mut CliConfig) {
        config.auth = None;
    }

    /// Display current authentication status
    fn status(&self, config: &CliConfig, json_output: bool) -> Result<(), AuthError> {
        let output = self.status_envelope(config);
        if output
            .pointer("/data/token_expired")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let auth = config.auth.as_ref().expect("expired status requires auth");
            return Err(AuthError::StoredTokenExpired {
                user: auth.display_name(),
                expires_at: auth.expires_at,
                platform_url: self.effective_auth_url().as_str().to_string(),
            });
        }

        Self::print_output(&output, json_output)
    }

    fn status_envelope(&self, config: &CliConfig) -> Value {
        let Some(auth) = &config.auth else {
            return with_envelope_metadata(json!({
                "status": "ok",
                "data": {
                    "authenticated": false,
                    "token_present": false,
                    "token_valid": false,
                    "token_expired": false,
                    "refresh_token_present": false,
                    "refresh_expires_at": null,
                    "refresh_seconds_remaining": null,
                    "expires_soon": false,
                    "expired": false,
                    "seconds_remaining": null,
                    "expires_in_seconds": null,
                    "platform_url": self.effective_auth_url().as_str(),
                },
                "next_actions": ["pcl auth login"],
            }));
        };

        let now = chrono::Utc::now();
        let token_expired = auth.expires_at <= now;
        let seconds_remaining = (auth.expires_at - now).num_seconds();
        let expires_soon = !token_expired && seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS;
        let refresh_seconds_remaining = auth
            .refresh_expires_at
            .map(|expires_at| (expires_at - now).num_seconds());
        with_envelope_metadata(json!({
            "status": "ok",
            "data": {
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
                "platform_url": self.effective_auth_url().as_str(),
            },
            "next_actions": if token_expired {
                json!(["pcl auth refresh --json", "pcl auth login --force", "pcl auth logout"])
            } else if expires_soon {
                json!(["pcl auth refresh --json", "pcl account"])
            } else {
                json!(["pcl account", "pcl projects --home"])
            },
        }))
    }

    fn print_output(value: &Value, json_output: bool) -> Result<(), AuthError> {
        let value = with_envelope_metadata(value.clone());
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&value)
                    .map_err(|error| AuthError::InvalidAuthData(error.to_string()))?
            );
        } else {
            print!("{}", toon_string(&value));
        }
        Ok(())
    }

    fn print_json_event(value: &Value) -> Result<(), AuthError> {
        println!(
            "{}",
            serde_json::to_string(&with_envelope_metadata(value.clone()))
                .map_err(|error| AuthError::InvalidAuthData(error.to_string()))?
        );
        Ok(())
    }
}

pub async fn refresh_stored_auth(
    config: &mut CliConfig,
    auth_url: &url::Url,
    cli_args: &CliArgs,
    force: bool,
) -> Result<RefreshOutcome, AuthError> {
    let _lock = RefreshLock::acquire(cli_args).await?;
    let mut disk_config = CliConfig::read_from_file(cli_args).map_err(AuthError::ConfigError)?;
    disk_config.normalize_auth_expiry_from_access_token();
    if disk_config.auth.is_some() || config.auth.is_none() {
        *config = disk_config;
    }

    let auth = config
        .auth
        .as_ref()
        .ok_or(AuthError::NoRefreshableSession)?;
    let now = chrono::Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    if !force && auth.expires_at > now && seconds_remaining > AUTH_EXPIRES_SOON_SECONDS {
        return Ok(RefreshOutcome {
            refreshed: false,
            reason: "token_still_valid",
            request_id: None,
        });
    }
    if auth.refresh_token.trim().is_empty() {
        return Err(AuthError::MissingRefreshToken);
    }

    let user_id = auth.user_id;
    let wallet_address = auth.wallet_address;
    let email = auth.email.clone();
    let refresh_token = auth.refresh_token.clone();
    let response = AuthCommand::refresh_request(auth_url, &refresh_token).await?;
    let status = response.status();
    let request_id = request_id_from_headers(response.headers());

    match status.as_u16() {
        200 => {
            let body = response
                .json::<RefreshResponse>()
                .await
                .map_err(|error| AuthError::InvalidAuthData(error.to_string()))?;
            persist_refreshed_auth(
                config,
                cli_args,
                body,
                user_id,
                wallet_address,
                email,
                request_id,
            )
        }
        401 => {
            let details = refresh_error_details(response).await;
            config.auth = None;
            config
                .write_to_file(cli_args)
                .map_err(AuthError::ConfigError)?;
            Err(AuthError::RefreshRejected {
                status: status.as_u16(),
                code: details.code,
                request_id,
                message: details.message,
            })
        }
        404 => {
            let details = refresh_error_details(response).await;
            Err(AuthError::RefreshEndpointNotFound {
                request_id,
                message: details.message,
            })
        }
        429 => {
            let retry_after_seconds = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            let details = refresh_error_details(response).await;
            Err(AuthError::RefreshRateLimited {
                retry_after_seconds,
                request_id,
                message: details.message,
            })
        }
        500..=599 => {
            let details = refresh_error_details(response).await;
            Err(AuthError::RefreshServerError {
                status: status.as_u16(),
                request_id,
                message: details.message,
            })
        }
        _ => {
            let details = refresh_error_details(response).await;
            Err(AuthError::InvalidAuthData(format!(
                "Refresh endpoint returned HTTP {}{}",
                status.as_u16(),
                details
                    .message
                    .map(|message| format!(": {message}"))
                    .unwrap_or_default()
            )))
        }
    }
}

fn persist_refreshed_auth(
    config: &mut CliConfig,
    cli_args: &CliArgs,
    body: RefreshResponse,
    user_id: Option<Uuid>,
    wallet_address: Option<Address>,
    email: Option<String>,
    request_id: Option<String>,
) -> Result<RefreshOutcome, AuthError> {
    config.auth = Some(UserAuth {
        access_token: body.token,
        refresh_token: body.refresh_token,
        expires_at: body.expires_at,
        refresh_expires_at: Some(body.refresh_expires_at),
        user_id,
        wallet_address,
        email,
    });
    config
        .write_to_file(cli_args)
        .map_err(AuthError::ConfigError)?;
    Ok(RefreshOutcome {
        refreshed: true,
        reason: "refreshed",
        request_id,
    })
}

impl RefreshLock {
    async fn acquire(cli_args: &CliArgs) -> Result<Self, AuthError> {
        let path = CliConfig::config_file_path(cli_args).with_extension("toml.lock");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| AuthError::ConfigError(ConfigError::WriteError(error)))?;
        }
        let deadline = Instant::now() + REFRESH_LOCK_TIMEOUT;
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(
                        file,
                        "pid={} acquired_at={}",
                        std::process::id(),
                        chrono::Utc::now().to_rfc3339()
                    );
                    let _ = file.sync_all();
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Instant::now() >= deadline {
                        return Err(AuthError::RefreshLockTimeout);
                    }
                    sleep(REFRESH_LOCK_POLL_INTERVAL).await;
                }
                Err(error) => {
                    return Err(AuthError::ConfigError(ConfigError::WriteError(error)));
                }
            }
        }
    }
}

impl Drop for RefreshLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

async fn refresh_error_details(response: reqwest::Response) -> RefreshErrorDetails {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return RefreshErrorDetails {
                code: None,
                message: Some(error.to_string()),
            };
        }
    };
    if content_type.contains("application/json")
        && let Ok(body) = serde_json::from_slice::<Value>(&bytes)
    {
        return RefreshErrorDetails {
            code: body
                .get("code")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            message: body
                .get("error")
                .or_else(|| body.get("message"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        };
    }
    RefreshErrorDetails {
        code: None,
        message: String::from_utf8(bytes.to_vec()).ok(),
    }
}

fn request_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn finish_timeout_if_needed(spinner: &ProgressBar, json_output: bool, error: &AuthError) {
    if !matches!(error, AuthError::Timeout(_)) {
        return;
    }
    if json_output {
        spinner.finish_and_clear();
    } else {
        spinner.finish_with_message("❌ Authentication timed out");
    }
}

fn default_poll_fallback_expires_at() -> chrono::DateTime<chrono::Utc> {
    let seconds = i64::try_from(POLL_TIMEOUT.as_secs()).unwrap_or(i64::MAX);
    chrono::Utc::now() + chrono::Duration::seconds(seconds)
}

fn poll_backoff_delay(attempts: u32, session_id: &Uuid) -> Duration {
    let multiplier = 1_u32.checked_shl(attempts.min(3)).unwrap_or(8);
    let base = std::cmp::min(
        INITIAL_POLL_INTERVAL.saturating_mul(multiplier),
        MAX_POLL_INTERVAL,
    );
    std::cmp::min(
        base.saturating_add(poll_jitter(attempts, session_id)),
        MAX_POLL_INTERVAL,
    )
}

fn poll_jitter(attempts: u32, session_id: &Uuid) -> Duration {
    let seed = session_id
        .as_u128()
        .wrapping_add(u128::from(attempts).wrapping_mul(0x9e37_79b9_7f4a_7c15_u128));
    let millis = u64::try_from(seed % 250).unwrap_or(0);
    Duration::from_millis(millis)
}

#[derive(Clone, Copy)]
enum AuthChallengeReason {
    Missing,
    Expired,
    ExpiresSoon,
    Forced,
    InvalidRefresh,
    RefreshUnavailable,
}

impl AuthChallengeReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing_auth",
            Self::Expired => "expired_token",
            Self::ExpiresSoon => "expires_soon",
            Self::Forced => "forced_login",
            Self::InvalidRefresh => "invalid_refresh_token",
            Self::RefreshUnavailable => "refresh_unavailable",
        }
    }

    fn refresh_attempted(self) -> bool {
        matches!(self, Self::InvalidRefresh | Self::RefreshUnavailable)
    }

    fn refresh_supported(self) -> bool {
        !matches!(self, Self::RefreshUnavailable)
    }
}

fn auth_challenge_reason(config: &CliConfig, force: bool) -> AuthChallengeReason {
    if force {
        return AuthChallengeReason::Forced;
    }
    let Some(auth) = &config.auth else {
        return AuthChallengeReason::Missing;
    };
    let now = chrono::Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    if auth.expires_at <= now {
        AuthChallengeReason::Expired
    } else if seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS {
        AuthChallengeReason::ExpiresSoon
    } else {
        AuthChallengeReason::Missing
    }
}

fn update_config_from_verified_status(
    config: &mut CliConfig,
    status: GetCliAuthStatusResponse,
    fallback_expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), AuthError> {
    let token = status.token.ok_or_else(|| {
        AuthError::InvalidAuthData("Verified but missing access token".to_string())
    })?;
    let token_expires_at = access_token_expires_at(&token);
    let refresh_token = status.refresh_token.ok_or_else(|| {
        AuthError::InvalidAuthData("Verified but missing refresh token".to_string())
    })?;
    let user_id = status
        .user_id
        .ok_or_else(|| AuthError::InvalidAuthData("Verified but missing user_id".to_string()))?;
    let wallet_address = status
        .address
        .and_then(|a| a.to_string().parse::<Address>().ok());

    config.auth = Some(UserAuth {
        access_token: token,
        refresh_token,
        expires_at: token_expires_at.unwrap_or(fallback_expires_at),
        refresh_expires_at: None,
        user_id: Some(user_id),
        wallet_address,
        email: status.email,
    });
    Ok(())
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':' | b'=')
        })
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{
        TimeZone,
        Utc,
    };
    use clap::Parser;
    use mockito::{
        Matcher,
        Server,
    };
    use uuid::Uuid;

    fn create_test_config() -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: "test_token".to_string(),
                refresh_token: "test_refresh".to_string(),
                expires_at: Utc.with_ymd_and_hms(2099, 12, 31, 0, 0, 0).unwrap(),
                refresh_expires_at: None,
                user_id: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
                wallet_address: Some(
                    "0x1234567890123456789012345678901234567890"
                        .parse()
                        .unwrap(),
                ),
                email: None,
            }),
        }
    }

    fn test_auth_response_json() -> &'static str {
        r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2024-12-31T00:00:00Z"}"#
    }

    fn test_cli_args(config_dir: &std::path::Path) -> CliArgs {
        CliArgs {
            config_dir: Some(config_dir.to_path_buf()),
            ..Default::default()
        }
    }

    fn expired_refreshable_config() -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: "old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap(),
                refresh_expires_at: None,
                user_id: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
        }
    }

    #[test]
    fn test_display_login_instructions() {
        let cmd = AuthCommand {
            command: AuthSubcommands::Login {
                force: false,
                no_wait: false,
            },
            auth_url: Some("https://app.phylax.systems".parse().unwrap()),
        };
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();
        cmd.display_login_instructions(&auth_response);
    }

    #[test]
    fn test_login_instructions_do_not_open_browser_in_tests() {
        assert!(!AuthCommand::should_open_browser());
    }

    #[test]
    fn test_login_instructions_envelope_is_structured() {
        let cmd = AuthCommand {
            command: AuthSubcommands::Login {
                force: false,
                no_wait: false,
            },
            auth_url: Some("https://app.phylax.systems".parse().unwrap()),
        };
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let output = cmd.login_instructions_envelope(
            &auth_response,
            Some(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap()),
        );

        assert_eq!(output["status"], "pending");
        assert_eq!(output["event"], "auth.login_instructions");
        assert_eq!(output["terminal"], false);
        assert_eq!(output["output_mode"], "jsonl");
        assert_eq!(output["data"]["state"], "login_instructions");
        assert_eq!(
            output["data"]["device_url"],
            "https://app.phylax.systems/device?session_id=550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(output["data"]["code"], "123456");
        assert_eq!(output["data"]["browser_opened"], false);
        assert_eq!(output["data"]["waiting_for_verification"], true);
        assert_eq!(
            output["data"]["previous_token_expired_at"],
            "2020-01-01T00:00:00+00:00"
        );
        assert!(
            output["data"]["poll_command"]
                .as_str()
                .is_some_and(|command| {
                    command.contains("--expires-at")
                        && command.contains("2024-12-31T00:00:00+00:00")
                })
        );
    }

    #[test]
    fn test_display_success_message() {
        let config = create_test_config();
        AuthCommand::display_success_message(&config).unwrap();
    }

    #[tokio::test]
    async fn test_request_auth_code() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/code")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(test_auth_response_json())
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();

        let client = cmd.api_client();
        let result = AuthCommand::request_auth_code(&client).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(*response.code, "123456");
        mock.assert();
    }

    #[tokio::test]
    async fn refresh_stored_auth_rotates_and_persists_new_token_pair() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/refresh")
            .match_header("authorization", Matcher::Missing)
            .match_header("content-type", Matcher::Regex("application/json.*".to_string()))
            .match_body(Matcher::Json(json!({ "refresh_token": "old_refresh" })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "req_refresh_ok")
            .with_body(
                r#"{"token":"new_access","refresh_token":"new_refresh","expires_at":"2030-01-01T00:00:00Z","refresh_expires_at":"2030-02-01T00:00:00Z"}"#,
            )
            .expect(1)
            .create_async()
            .await;
        let temp_dir = tempfile::tempdir().unwrap();
        let cli_args = test_cli_args(temp_dir.path());
        let mut config = expired_refreshable_config();
        config.write_to_file(&cli_args).unwrap();

        let outcome = refresh_stored_auth(
            &mut config,
            &server.url().parse().unwrap(),
            &cli_args,
            false,
        )
        .await
        .unwrap();

        assert!(outcome.refreshed);
        assert_eq!(outcome.request_id.as_deref(), Some("req_refresh_ok"));
        let auth = config.auth.as_ref().unwrap();
        assert_eq!(auth.access_token, "new_access");
        assert_eq!(auth.refresh_token, "new_refresh");
        assert_eq!(auth.email.as_deref(), Some("agent@example.com"));
        assert_eq!(
            auth.refresh_expires_at.unwrap(),
            Utc.with_ymd_and_hms(2030, 2, 1, 0, 0, 0).unwrap()
        );
        let persisted = CliConfig::read_from_file(&cli_args).unwrap();
        assert_eq!(
            persisted.auth.as_ref().unwrap().refresh_token,
            "new_refresh"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn invalid_refresh_token_clears_local_credentials() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/refresh")
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "req_refresh_invalid")
            .with_body(r#"{"code":"INVALID_REFRESH_TOKEN","error":"invalid refresh"}"#)
            .expect(1)
            .create_async()
            .await;
        let temp_dir = tempfile::tempdir().unwrap();
        let cli_args = test_cli_args(temp_dir.path());
        let mut config = expired_refreshable_config();
        config.write_to_file(&cli_args).unwrap();

        let error = refresh_stored_auth(
            &mut config,
            &server.url().parse().unwrap(),
            &cli_args,
            false,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, AuthError::RefreshRejected { .. }));
        if let AuthError::RefreshRejected {
            status,
            code,
            request_id,
            message,
        } = error
        {
            assert_eq!(status, 401);
            assert_eq!(code.as_deref(), Some("INVALID_REFRESH_TOKEN"));
            assert_eq!(request_id.as_deref(), Some("req_refresh_invalid"));
            assert_eq!(message.as_deref(), Some("invalid refresh"));
        }
        assert!(config.auth.is_none());
        assert!(CliConfig::read_from_file(&cli_args).unwrap().auth.is_none());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn missing_refresh_endpoint_keeps_local_credentials() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/refresh")
            .with_status(404)
            .with_header("content-type", "application/json")
            .with_header("x-request-id", "req_refresh_missing")
            .with_body(r#"{"error":"Not Found"}"#)
            .expect(1)
            .create_async()
            .await;
        let temp_dir = tempfile::tempdir().unwrap();
        let cli_args = test_cli_args(temp_dir.path());
        let mut config = expired_refreshable_config();
        config.write_to_file(&cli_args).unwrap();

        let error = refresh_stored_auth(
            &mut config,
            &server.url().parse().unwrap(),
            &cli_args,
            false,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, AuthError::RefreshEndpointNotFound { .. }));
        if let AuthError::RefreshEndpointNotFound {
            request_id,
            message,
        } = error
        {
            assert_eq!(request_id.as_deref(), Some("req_refresh_missing"));
            assert_eq!(message.as_deref(), Some("Not Found"));
        }
        assert!(config.auth.is_some());
        assert!(CliConfig::read_from_file(&cli_args).unwrap().auth.is_some());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_check_auth_status_verified() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("session_id".into(), "550e8400-e29b-41d4-a716-446655440000".into()),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","address":"0x1234567890123456789012345678901234567890","token":"test_token","refresh_token":"test_refresh"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(status.verified);
        assert_eq!(status.token.as_deref(), Some("test_token"));
        assert_eq!(status.refresh_token.as_deref(), Some("test_refresh"));
        assert_eq!(
            &*status.address.unwrap(),
            "0x1234567890123456789012345678901234567890"
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status_not_verified() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":false}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(!status.verified);
        assert!(status.token.is_none());
        mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status_verified_without_address() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("session_id".into(), "550e8400-e29b-41d4-a716-446655440000".into()),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .expect(1)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","token":"test_token","refresh_token":"test_refresh"}"#)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(status.verified);
        assert_eq!(status.token.as_deref(), Some("test_token"));
        assert!(status.address.is_none());
        mock.assert();
    }

    #[test]
    fn test_logout() {
        let mut config = create_test_config();
        AuthCommand::logout(&mut config);
        assert!(config.auth.is_none());
    }

    #[tokio::test]
    async fn remote_logout_posts_to_platform_before_local_cleanup() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/web/auth/logout")
            .match_header("authorization", "Bearer test_token")
            .match_header(
                "content-type",
                Matcher::Regex("application/json.*".to_string()),
            )
            .match_body(Matcher::Json(json!({})))
            .with_status(200)
            .with_header("x-request-id", "req_logout_ok")
            .with_body(r#"{"success":true}"#)
            .expect(1)
            .create_async()
            .await;
        let config = create_test_config();
        let auth_url = server.url();
        let cmd =
            AuthCommand::try_parse_from(vec!["auth", "--auth-url", auth_url.as_str(), "logout"])
                .unwrap();

        let logout = cmd.remote_logout(&config, false).await;

        assert_eq!(logout["attempted"], true);
        assert_eq!(logout["success"], true);
        assert_eq!(logout["mode"], "remote");
        assert_eq!(logout["http_status"], 200);
        assert_eq!(logout["request_id"], "req_logout_ok");
        mock.assert_async().await;
    }

    #[test]
    fn test_status() {
        let config = create_test_config();
        let cmd = AuthCommand::try_parse_from(vec![
            "auth",
            "--auth-url",
            "https://app.phylax.systems",
            "status",
        ])
        .unwrap();
        let output = cmd.status_envelope(&config);
        assert_eq!(output["data"]["authenticated"], true);
        assert_eq!(output["data"]["token_valid"], true);
        assert_eq!(
            output["data"]["platform_url"],
            "https://app.phylax.systems/"
        );
    }

    #[test]
    fn test_status_when_logged_out() {
        let config = CliConfig::default();
        let cmd = AuthCommand::try_parse_from(vec![
            "auth",
            "--auth-url",
            "https://app.phylax.systems",
            "status",
        ])
        .unwrap();
        let output = cmd.status_envelope(&config);
        assert_eq!(output["schema_version"], "pcl.envelope.v1");
        assert_eq!(output["pcl_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(output["data"]["authenticated"], false);
        assert_eq!(output["data"]["token_valid"], false);
        assert_eq!(output["next_actions"], json!(["pcl auth login"]));
    }

    #[test]
    fn test_status_detects_expired_token() {
        let mut config = create_test_config();
        config.auth.as_mut().unwrap().expires_at =
            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
        let cmd = AuthCommand::try_parse_from(vec![
            "auth",
            "--auth-url",
            "https://app.phylax.systems",
            "status",
        ])
        .unwrap();
        let output = cmd.status_envelope(&config);
        assert_eq!(output["data"]["authenticated"], true);
        assert_eq!(output["data"]["token_valid"], false);
        assert_eq!(output["data"]["token_expired"], true);
    }

    #[tokio::test]
    async fn test_login_when_already_authenticated() {
        let mut config = create_test_config();
        let cmd = AuthCommand::try_parse_from(vec![
            "auth",
            "--auth-url",
            "https://app.phylax.systems",
            "login",
        ])
        .unwrap();

        let result = cmd.login(&mut config, false, false, false).await;
        assert!(result.is_ok());
        assert_eq!(
            config.auth.as_ref().unwrap().wallet_address,
            Some(
                "0x1234567890123456789012345678901234567890"
                    .parse::<Address>()
                    .unwrap()
            )
        );
    }

    #[tokio::test]
    async fn test_check_auth_status_verified_missing_optional_fields() {
        let mut server = Server::new_async().await;

        // verified:true but missing optional token/address fields
        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":true}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(status.verified);
        assert!(status.token.is_none());
        assert!(status.refresh_token.is_none());
        mock.assert();
    }

    #[tokio::test]
    async fn test_wait_for_verification_stops_when_session_expired() {
        let server = Server::new_async().await;

        // No mocks — the server should never be called because the session
        // is already expired before the first poll.

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let mut config = CliConfig::default();

        // Build an auth response with expiresAt in the past
        let expired_response: GetCliAuthCodeResponse = serde_json::from_str(
            r#"{"code":"999999","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2020-01-01T00:00:00Z"}"#,
        )
        .unwrap();

        let result = cmd
            .wait_for_verification(&mut config, &client, &expired_response, false)
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, AuthError::SessionExpired),
            "Expected SessionExpired, got {err:?}"
        );
        // Config should remain unauthenticated
        assert!(config.auth.is_none());
    }

    #[tokio::test]
    async fn test_check_auth_status_session_expired_returns_typed_error() {
        let mut server = Server::new_async().await;

        // Server returns 400 with SESSION_EXPIRED error code
        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Session has expired","code":"SESSION_EXPIRED"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::SessionExpired)),
            "Expected SessionExpired, got {result:?}"
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status_session_not_found_returns_typed_error() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Session not found","code":"SESSION_NOT_FOUND"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::SessionNotFound)),
            "Expected SessionNotFound, got {result:?}"
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status_user_not_found_returns_typed_error() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"User not found. Request a new code and try again.","code":"USER_NOT_FOUND"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::UserNotFound)),
            "Expected UserNotFound, got {result:?}"
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status_server_error_returns_typed_error() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Internal server error","code":"INTERNAL_ERROR"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(
            matches!(result, Err(AuthError::ServerError(_))),
            "Expected ServerError, got {result:?}"
        );
        mock.assert();
    }

    #[tokio::test]
    async fn test_polling_stops_on_session_expired() {
        let mut server = Server::new_async().await;

        // First poll: pending. Second poll: session expired.
        let pending_mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":false}"#)
            .expect(1)
            .create();

        let expired_mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Session has expired","code":"SESSION_EXPIRED"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let mut config = CliConfig::default();

        // Use a far-future expiresAt so the client-side check doesn't trigger
        let auth_response: GetCliAuthCodeResponse = serde_json::from_str(
            r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2099-12-31T00:00:00Z"}"#,
        )
        .unwrap();

        let result = cmd
            .wait_for_verification(&mut config, &client, &auth_response, false)
            .await;

        assert!(
            matches!(result, Err(AuthError::SessionExpired)),
            "Expected SessionExpired, got {result:?}"
        );
        assert!(config.auth.is_none());
        pending_mock.assert();
        expired_mock.assert();
    }

    #[tokio::test]
    async fn test_polling_retries_on_server_error() {
        let mut server = Server::new_async().await;

        // First poll: 500 (transient). Second poll: success.
        let error_mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(500)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Internal server error","code":"INTERNAL_ERROR"}"#)
            .expect(1)
            .create();

        let success_mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":true,"user_id":"550e8400-e29b-41d4-a716-446655440000","token":"test_token","refresh_token":"test_refresh","address":"0x1234567890123456789012345678901234567890"}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let mut config = CliConfig::default();

        let auth_response: GetCliAuthCodeResponse = serde_json::from_str(
            r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2099-12-31T00:00:00Z"}"#,
        )
        .unwrap();

        let result = cmd
            .wait_for_verification(&mut config, &client, &auth_response, false)
            .await;

        assert!(
            result.is_ok(),
            "Expected success after retry, got {result:?}"
        );
        assert!(config.auth.is_some());
        error_mock.assert();
        success_mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status_invalid_json() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r"not valid json")
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let auth_response: GetCliAuthCodeResponse =
            serde_json::from_str(test_auth_response_json()).unwrap();

        let result = AuthCommand::check_auth_status(
            &client,
            &auth_response.device_secret,
            &auth_response.session_id,
        )
        .await;
        assert!(result.is_err());
        mock.assert();
    }

    #[tokio::test]
    async fn test_polling_stops_on_verified_missing_tokens() {
        let mut server = Server::new_async().await;

        // Server returns verified:true but without tokens — wait_for_verification
        // should bail with InvalidAuthData instead of silently continuing.
        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded(
                    "session_id".into(),
                    "550e8400-e29b-41d4-a716-446655440000".into(),
                ),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":true}"#)
            .expect(1)
            .create();

        let cmd = AuthCommand::try_parse_from(vec!["auth", "--auth-url", &server.url(), "login"])
            .unwrap();
        let client = cmd.api_client();
        let mut config = CliConfig::default();

        let auth_response: GetCliAuthCodeResponse = serde_json::from_str(
            r#"{"code":"123456","sessionId":"550e8400-e29b-41d4-a716-446655440000","deviceSecret":"test_secret","expiresAt":"2099-12-31T00:00:00Z"}"#,
        )
        .unwrap();

        let result = cmd
            .wait_for_verification(&mut config, &client, &auth_response, false)
            .await;

        assert!(
            matches!(result, Err(AuthError::InvalidAuthData(_))),
            "Expected InvalidAuthData, got {result:?}"
        );
        assert!(config.auth.is_none());
        mock.assert();
    }
}
