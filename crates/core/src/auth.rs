use crate::config::CliConfig;
use crate::error::AuthError;
use eyre::Result;
use reqwest::Client;
use serde::Deserialize;
use tokio::time::{sleep, Duration};

const BASE_URL: &str = "https://credible-layer-dapp.pages.dev";
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_RETRIES: u32 = 150; // 5 minutes worth of 2-second intervals

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

#[derive(Deserialize)]
struct StatusResponse {
    verified: bool,
    address: Option<String>,
    token: Option<String>,
    refresh_token: Option<String>,
}

#[derive(clap::Parser)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: AuthSubcommands,
}

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
    pub async fn run(&self, config: &mut CliConfig) -> Result<(), AuthError> {
        match &self.command {
            AuthSubcommands::Login => self.login(config).await,
            AuthSubcommands::Logout => self.logout(config),
            AuthSubcommands::Status => self.status(config),
        }
    }

    async fn login(&self, config: &mut CliConfig) -> Result<(), AuthError> {
        let client = Client::new();
        let auth_response: AuthResponse = client
            .get(format!("{}/api/v1/cli/auth/code", BASE_URL))
            .send()
            .await?
            .json()
            .await?;

        let url = format!(
            "{}/device?session_id={}",
            BASE_URL, auth_response.session_id
        );
        println!("\nTo authenticate, please visit:\n\n🔗 {url}\n📝 Code: {code}\n\nWaiting for authentication...", 
            url = url,
            code = auth_response.code
        );

        // Poll for authentication status
        let mut attempts = 0;
        while attempts < MAX_RETRIES {
            let status: StatusResponse = client
                .get(format!("{}/api/v1/cli/auth/status", BASE_URL))
                .query(&[
                    ("session_id", &auth_response.session_id),
                    ("device_secret", &auth_response.device_secret),
                ])
                .send()
                .await?
                .json()
                .await?;

            if status.verified {
                config.auth = Some(crate::config::UserAuth {
                    access_token: status.token.unwrap(),
                    refresh_token: status.refresh_token.unwrap(),
                    user_address: status.address.unwrap(),
                    expires_at: auth_response.expires_at,
                });

                println!(
                    "\n{ascii}\n\n🎉 Authentication successful!\n🔗 Connected wallet: {address}\n",
                    ascii = PHYLAX_ASCII,
                    address = config.auth.as_ref().unwrap().user_address
                );
                return Ok(());
            }

            attempts += 1;
            sleep(POLL_INTERVAL).await;
        }

        if attempts >= MAX_RETRIES {
            return Err(AuthError::Timeout(MAX_RETRIES));
        }

        Ok(())
    }

    fn logout(&self, config: &mut CliConfig) -> Result<(), AuthError> {
        config.auth = None;
        println!("👋 Logged out successfully");
        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliConfig, UserAuth};

    fn create_test_config() -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: "test_token".to_string(),
                refresh_token: "test_refresh".to_string(),
                user_address: "0xtest".to_string(),
                expires_at: "2024-12-31".to_string(),
            }),
            ..Default::default()
        }
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
    fn test_status_when_logged_in() {
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

    #[test]
    fn test_logout_when_already_logged_out() {
        let mut config = CliConfig::default();
        let cmd = AuthCommand {
            command: AuthSubcommands::Logout,
        };

        let result = cmd.logout(&mut config);

        assert!(result.is_ok());
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_auth_data_is_complete_after_logout() {
        let mut config = create_test_config();
        let cmd = AuthCommand {
            command: AuthSubcommands::Logout,
        };

        cmd.logout(&mut config).unwrap();

        assert!(config.auth.is_none());
        // Verify no leftover data
        assert!(matches!(config, CliConfig { auth: None, .. }));
    }
}
