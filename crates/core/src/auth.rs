use crate::config::{CliConfig, UserAuth};
use crate::error::AuthError;
use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use eyre::Result;
use reqwest::Client;
use serde::Deserialize;
use tokio::time::{sleep, Duration};

/// Interval between authentication status checks
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Maximum number of retry attempts (5 minutes worth of 2-second intervals)
const MAX_RETRIES: u32 = 150;

/// ASCII art logo displayed after successful authentication
const PHYLAX_ASCII: &str = r#"
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@BD>"            "<8@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@BP"     _.               '4@@@@@@@@@@@@@@@
@@@@@@@@@@@@D    _e@@B  ,_,   __  t@g_    `G@@@@@@@@@@@@
@@@@@@@@@@P   _g@@@@P  /@@@  [@@@  \@@@@_    %@@@@@@@@@@
@@@@@@@@B   _B@@@@@W  {@@@@  [@@@@  T@@@@@a   `@@@@@@@@@
@@@@@@@P   g@@@@@@@  ;@@@@@  [@@@@A  @@@@@@@_   f@@@@@@@
@@@@@@P  ,@@@@@@@@F  @@@@@@  g@@@@@  !@@@@@@@L   V@@@@@@
@@@@@B   @@@@@@@@@  ;@@@@@@  B@@@@@|  @@@@@@@@L   @@@@@@
@@@@@'  [@@@@@@@@@  g@@BBD>  <4B@@@@  @@@@@@@@@   '@@@@@
@@@@@   @@@@@@@@@@  BW  __    __ `8@  B@@@@@@@@j   @@@@@
@@@@@                 ;@@@@  B@@@;                 @@@@@
@@@@@   qgg@@@@@@g  __ "B@@  @BB" __  g@@@@@@gq;   @@@@@
@@@@@   @@@@@@@@@@  @@@q___  ___g@@B  @@@@@@@@@   .@@@@@
@@@@@@   @@@@@@@@@  [@@@@@@  @@@@@@|  @@@@@@@@P   g@@@@@
@@@@@@\  '@@@@@@@@,  @@@@@@  @@@@@@  |@@@@@@@W   /@@@@@@
@@@@@@@L  `@@@@@@@@  0@@@@g  @@@@@F  @@@@@@@P   /@@@@@@@
@@@@@@@@p   \@@@@@@,  @@@@8  @@@@W  A@@@@@B    j@@@@@@@@
@@@@@@@@@@_   "@@@@@,  @@@]  @@@D  /@@@@D    _@@@@@@@@@@
@@@@@@@@@@@@_    <B@@_  <=   "8"  /@BP"    _@@@@@@@@@@@@
@@@@@@@@@@@@@@@_     ""          "      _g@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@g__              __g@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
"#;

/// Response from the initial authentication request
#[derive(Deserialize)]
struct AuthResponse {
    code: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "deviceSecret")]
    device_secret: String,
    #[serde(rename = "expiresAt")]
    expires_at: String,
}

/// Response from the authentication status check
#[derive(Deserialize)]
struct StatusResponse {
    verified: bool,
    address: Option<String>,
    token: Option<String>,
    refresh_token: Option<String>,
}

/// Command-line interface for authentication operations
#[derive(clap::Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommands,
}

/// Available authentication subcommands
#[derive(clap::Subcommand)]
pub enum AuthSubcommands {
    /// Login to the CLI
    Login,
    /// Logout from the CLI
    Logout,
    /// Check current auth status
    Status,
}

impl AuthCommand {
    /// Execute the authentication command
    pub async fn run(&self, config: &mut CliConfig) -> Result<(), AuthError> {
        match &self.command {
            AuthSubcommands::Login => self.login(config).await,
            AuthSubcommands::Logout => self.logout(config),
            AuthSubcommands::Status => self.status(config),
        }
    }

    /// Initiate the login process and wait for user authentication
    async fn login(&self, config: &mut CliConfig) -> Result<(), AuthError> {
        let auth_response = self.request_auth_code().await?;
        self.display_login_instructions(&auth_response);
        self.wait_for_verification(config, &auth_response).await
    }

    /// Request an authentication code from the server
    async fn request_auth_code(&self) -> Result<AuthResponse, AuthError> {
        let client = Client::new();
        let url = format!("{}/api/v1/cli/auth/code", get_base_url());
        Ok(client.get(url).send().await?.json().await?)
    }

    /// Display login URL and code to the user
    fn display_login_instructions(&self, auth_response: &AuthResponse) {
        let url = format!(
            "{}/device?session_id={}",
            get_base_url(),
            auth_response.session_id
        );
        println!(
            "\nTo authenticate, please visit:\n\n🔗 {url}\n📝 Code: {code}\n\nWaiting for authentication...",
            url = url,
            code = auth_response.code
        );
    }

    /// Poll for authentication verification
    async fn wait_for_verification(
        &self,
        config: &mut CliConfig,
        auth_response: &AuthResponse,
    ) -> Result<(), AuthError> {
        let client = Client::new();

        for _ in 0..MAX_RETRIES {
            let status = self.check_auth_status(&client, auth_response).await?;

            if status.verified {
                self.update_config(config, status, auth_response)?;
                self.display_success_message(config);
                return Ok(());
            }

            sleep(POLL_INTERVAL).await;
        }

        Err(AuthError::Timeout(MAX_RETRIES))
    }

    /// Check the current authentication status
    async fn check_auth_status(
        &self,
        client: &Client,
        auth_response: &AuthResponse,
    ) -> Result<StatusResponse, AuthError> {
        let url = format!("{}/api/v1/cli/auth/status", get_base_url());
        Ok(client
            .get(url)
            .query(&[
                ("session_id", &auth_response.session_id),
                ("device_secret", &auth_response.device_secret),
            ])
            .send()
            .await?
            .json()
            .await?)
    }

    /// Update the configuration with authentication data
    fn update_config(
        &self,
        config: &mut CliConfig,
        status: StatusResponse,
        auth_response: &AuthResponse,
    ) -> Result<(), AuthError> {
        config.auth = Some(UserAuth {
            access_token: status
                .token
                .ok_or(AuthError::InvalidAuthData("Missing token".to_string()))?,
            refresh_token: status.refresh_token.ok_or(AuthError::InvalidAuthData(
                "Missing refresh token".to_string(),
            ))?,
            user_address: status
                .address
                .ok_or(AuthError::InvalidAuthData("Missing address".to_string()))?
                .parse::<Address>()
                .map_err(|_| AuthError::InvalidAddress)?,
            expires_at: DateTime::parse_from_rfc3339(&auth_response.expires_at)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| AuthError::InvalidTimestamp)?,
        });
        Ok(())
    }

    /// Display success message after authentication
    fn display_success_message(&self, config: &CliConfig) {
        println!(
            "\n{ascii}\n\n🎉 Authentication successful!\n🔗 Connected wallet: {address}\n",
            ascii = PHYLAX_ASCII,
            address = config.auth.as_ref().unwrap().user_address
        );
    }

    /// Remove authentication data from configuration
    fn logout(&self, config: &mut CliConfig) -> Result<(), AuthError> {
        config.auth = None;
        println!("👋 Logged out successfully");
        Ok(())
    }

    /// Display current authentication status
    fn status(&self, config: &CliConfig) -> Result<(), AuthError> {
        let (icon, message) = if config.auth.is_some() {
            (
                "✅",
                format!(
                    "Logged in as: {}",
                    config.auth.as_ref().unwrap().user_address
                ),
            )
        } else {
            ("❌", "Not logged in".to_string())
        };
        println!("{icon} {message}");
        Ok(())
    }
}

fn get_base_url() -> String {
    std::env::var("AUTH_BASE_URL")
        .unwrap_or_else(|_| "https://credible-layer-dapp.pages.dev".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use mockito::Server;

    fn create_test_config() -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: "test_token".to_string(),
                refresh_token: "test_refresh".to_string(),
                user_address: "0x1234567890123456789012345678901234567890"
                    .parse()
                    .unwrap(),
                expires_at: Utc.with_ymd_and_hms(2024, 12, 31, 0, 0, 0).unwrap(),
            }),
            ..Default::default()
        }
    }

    fn create_test_auth_response() -> AuthResponse {
        AuthResponse {
            code: "123456".to_string(),
            session_id: "test_session".to_string(),
            device_secret: "test_secret".to_string(),
            expires_at: "2024-12-31T00:00:00Z".to_string(),
        }
    }

    fn create_test_status_response() -> StatusResponse {
        StatusResponse {
            verified: true,
            address: Some("0x1234567890123456789012345678901234567890".to_string()),
            token: Some("test_token".to_string()),
            refresh_token: Some("test_refresh".to_string()),
        }
    }

    #[test]
    fn test_display_login_instructions() {
        let cmd = AuthCommand {
            command: AuthSubcommands::Login,
        };
        let auth_response = create_test_auth_response();

        // Can't easily test stdout, but we can verify it doesn't panic
        cmd.display_login_instructions(&auth_response);
    }

    #[test]
    fn test_update_config() {
        let mut config = CliConfig::default();
        let cmd = AuthCommand {
            command: AuthSubcommands::Login,
        };
        let auth_response = create_test_auth_response();
        let status = create_test_status_response();

        let result = cmd.update_config(&mut config, status, &auth_response);

        if let Err(e) = &result {
            println!("Error: {:?}", e);
        }
        assert!(result.is_ok());
        assert!(config.auth.is_some());
        let auth = config.auth.as_ref().unwrap();
        assert_eq!(
            auth.user_address,
            "0x1234567890123456789012345678901234567890"
                .parse::<Address>()
                .unwrap()
        );
        assert_eq!(auth.access_token, "test_token");
        assert_eq!(auth.refresh_token, "test_refresh");
        assert_eq!(
            auth.expires_at,
            Utc.with_ymd_and_hms(2024, 12, 31, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_display_success_message() {
        let config = create_test_config();
        let cmd = AuthCommand {
            command: AuthSubcommands::Login,
        };

        // Can't easily test stdout, but we can verify it doesn't panic
        cmd.display_success_message(&config);
    }

    #[tokio::test]
    async fn test_request_auth_code() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/code")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"code":"123456","sessionId":"test_session","deviceSecret":"test_secret","expiresAt":"2024-12-31"}"#)
            .create();

        let cmd = AuthCommand {
            command: AuthSubcommands::Login,
        };
        std::env::set_var("AUTH_BASE_URL", server.url());
        let result = cmd.request_auth_code().await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.code, "123456");
        assert_eq!(response.session_id, "test_session");
        mock.assert();
    }

    #[tokio::test]
    async fn test_check_auth_status() {
        let mut server = Server::new_async().await;

        let mock = server
            .mock("GET", "/api/v1/cli/auth/status")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("session_id".into(), "test_session".into()),
                mockito::Matcher::UrlEncoded("device_secret".into(), "test_secret".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"verified":true,"address":"0xtest","token":"test_token","refresh_token":"test_refresh"}"#)
            .create();

        let cmd = AuthCommand {
            command: AuthSubcommands::Login,
        };
        let client = Client::new();
        let auth_response = create_test_auth_response();
        std::env::set_var("AUTH_BASE_URL", server.url());

        let result = cmd.check_auth_status(&client, &auth_response).await;

        assert!(result.is_ok());
        let status = result.unwrap();
        assert!(status.verified);
        assert_eq!(status.address.unwrap(), "0xtest");
        mock.assert();
    }

    #[test]
    fn test_logout() {
        let mut config = create_test_config();
        let cmd = AuthCommand {
            command: AuthSubcommands::Logout,
        };

        let result = cmd.logout(&mut config);

        assert!(result.is_ok());
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_status() {
        let config = create_test_config();
        let cmd = AuthCommand {
            command: AuthSubcommands::Status,
        };

        let result = cmd.status(&config);

        assert!(result.is_ok());
    }

    #[test]
    fn test_status_when_logged_out() {
        let config = CliConfig::default();
        let cmd = AuthCommand {
            command: AuthSubcommands::Status,
        };

        let result = cmd.status(&config);

        assert!(result.is_ok());
    }
}
