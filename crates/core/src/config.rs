use crate::error::ConfigError;
use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use clap::Parser;
use colored::Colorize;
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

/// Directory name for storing PCL configuration
pub const CONFIG_DIR: &str = ".pcl";
/// Configuration file name
pub const CONFIG_FILE: &str = "config.toml";

/// Main configuration structure for PCL
///
/// This struct holds all the configuration data for the PCL tool,
/// including authentication details and pending assertions.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CliConfig {
    /// Optional authentication details
    pub auth: Option<UserAuth>,
    /// Map of assertions pending submission, keyed by contract name
    pub assertions_for_submission: HashMap<String, AssertionForSubmission>,
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
}

impl From<std::io::Error> for ConfigError {
    fn from(error: std::io::Error) -> Self {
        ConfigError::WriteError(error)
    }
}

impl ConfigArgs {
    /// Executes the configuration command
    ///
    /// # Arguments
    /// * `config` - The configuration to operate on
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    pub fn run(&self, config: &mut CliConfig) -> Result<(), ConfigError> {
        match self.command {
            ConfigCommand::Show => {
                println!("{}", config);
                Ok(())
            }
            ConfigCommand::Delete => {
                *config = CliConfig::default();
                Ok(())
            }
        }
    }
}

impl CliConfig {
    /// Writes the configuration to the default config file
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    pub fn write_to_file(&self) -> Result<(), ConfigError> {
        self.write_to_file_at_dir(Self::get_config_dir())
    }

    /// Writes the configuration to a specific directory
    ///
    /// # Arguments
    /// * `config_dir` - Directory to write the config file to
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn write_to_file_at_dir(&self, config_dir: PathBuf) -> Result<(), ConfigError> {
        std::fs::create_dir_all(&config_dir).map_err(ConfigError::WriteError)?;
        let config_file = config_dir.join(CONFIG_FILE);
        let config_str = toml::to_string(self).unwrap();
        std::fs::write(config_file, config_str).map_err(ConfigError::WriteError)?;
        Ok(())
    }

    /// Gets the default configuration directory path
    ///
    /// # Returns
    /// * `PathBuf` - Path to the config directory
    pub fn get_config_dir() -> PathBuf {
        home_dir().unwrap().join(CONFIG_DIR)
    }

    /// Reads configuration from a specific directory
    ///
    /// # Arguments
    /// * `config_dir` - Directory to read the config file from
    ///
    /// # Returns
    /// * `Result<Self, ConfigError>` - Configuration or error
    fn read_from_file_at_dir(config_dir: PathBuf) -> Result<Self, ConfigError> {
        let config_file = config_dir.join(CONFIG_FILE);
        let config_str = std::fs::read_to_string(config_file).map_err(ConfigError::ReadError)?;
        Ok(toml::from_str(&config_str).unwrap())
    }

    /// Reads configuration from the default config file
    ///
    /// # Returns
    /// * `Result<Self, ConfigError>` - Configuration or error
    pub fn read_from_file() -> Result<Self, ConfigError> {
        Self::read_from_file_at_dir(Self::get_config_dir())
    }

    /// Adds an assertion to the pending submissions
    ///
    /// # Arguments
    /// * `assertion_for_submission` - The assertion to add
    pub fn add_assertion_for_submission(
        &mut self,
        assertion_for_submission: AssertionForSubmission,
    ) {
        self.assertions_for_submission.insert(
            assertion_for_submission.assertion_contract.clone(),
            assertion_for_submission,
        );
    }
}

impl fmt::Display for CliConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config_path = Self::get_config_dir().join(CONFIG_FILE);

        writeln!(f, "PCL Configuration")?;
        writeln!(f, "==================")?;
        writeln!(f, "Config path: {}", config_path.display())?;

        match &self.auth {
            Some(auth) => writeln!(f, "{}", auth)?,
            None => writeln!(f, "Authentication: Not authenticated")?,
        }
        if !self.assertions_for_submission.is_empty() {
            writeln!(f, "\nPending Assertions for Submission")?;
            writeln!(f, "--------------------------------")?;
            for (i, assertion) in self.assertions_for_submission.values().enumerate() {
                writeln!(f, "Assertion #{}:\n{}", i + 1, assertion)?;
            }
        } else {
            writeln!(f, "\nNo pending assertions for submission")?;
        }

        Ok(())
    }
}

/// Authentication details for a user
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UserAuth {
    /// Access token for API authentication
    pub access_token: String,
    /// Refresh token for obtaining new access tokens
    pub refresh_token: String,
    /// Ethereum address of the user
    pub user_address: Address,
    /// Token expiration timestamp
    #[serde(with = "chrono::serde::ts_seconds")]
    pub expires_at: DateTime<Utc>,
}

impl fmt::Display for UserAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Authentication:")?;
        writeln!(f, "  User Address: {}", self.user_address)?;
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

/// An assertion that is pending submission to the DA layer
#[derive(Debug, Default, Serialize, Deserialize, Hash, Eq, PartialEq, Clone)]
pub struct AssertionForSubmission {
    /// Name of the assertion contract
    pub assertion_contract: String,
    /// Unique identifier for the assertion
    pub assertion_id: String,
    /// Cryptographic signature of the assertion
    pub signature: String,
}

impl fmt::Display for AssertionForSubmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Contract: {}", self.assertion_contract)?;
        writeln!(f, "  ID: {}", self.assertion_id)?;
        write!(
            f,
            "  Signature: {}...",
            &self.signature.chars().take(10).collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;
    use std::fs;

    /// Helper function to set up a temporary config directory
    fn setup_config_dir() -> (PathBuf, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        env::set_var("HOME", temp_dir.path());
        (temp_dir.path().join(CONFIG_DIR), temp_dir)
    }

    #[test]
    fn test_write_and_read_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Use a fixed timestamp for testing
        let fixed_timestamp = DateTime::from_timestamp(1672502400, 0).unwrap(); // 2022-12-31 16:00:00 UTC

        let config = CliConfig {
            auth: Some(UserAuth {
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                user_address: Address::from_slice(&[0; 20]),
                expires_at: fixed_timestamp,
            }),
            assertions_for_submission: vec![(
                "contract1".to_string(),
                AssertionForSubmission {
                    assertion_contract: "contract1".to_string(),
                    assertion_id: "id1".to_string(),
                    signature: "sig1".to_string(),
                },
            )]
            .into_iter()
            .collect(),
        };

        // Test writing
        assert!(config.write_to_file_at_dir(config_dir.clone()).is_ok());

        // Test reading
        let read_config = CliConfig::read_from_file_at_dir(config_dir.clone()).unwrap();
        assert_eq!(
            read_config.auth.as_ref().unwrap().access_token,
            "test_access"
        );
        assert_eq!(
            read_config.auth.as_ref().unwrap().refresh_token,
            "test_refresh"
        );
        assert_eq!(
            read_config.auth.as_ref().unwrap().user_address,
            Address::from_slice(&[0; 20])
        );
        assert_eq!(read_config.assertions_for_submission.len(), 1);
        assert_eq!(
            read_config
                .assertions_for_submission
                .get("contract1")
                .unwrap()
                .assertion_contract,
            "contract1"
        );

        // Test display format without colors
        let formatted_cfg = format!("{}", read_config);
        let expected_cfg = format!(
            r"PCL Configuration
==================
Config path: {}
Authentication:
  User Address: 0x0000000000000000000000000000000000000000
  Token Expired at 2022-12-31 16:00:00 UTC
  Access Token: [Set]
  Refresh Token: [Set]


Pending Assertions for Submission
--------------------------------
Assertion #1:
Contract: contract1
  ID: id1
  Signature: sig1...
",
            config_dir.join(CONFIG_FILE).display()
        );
        assert_eq!(formatted_cfg, expected_cfg);
    }

    #[test]
    fn test_read_nonexistent_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Try reading without creating a file
        let result = CliConfig::read_from_file_at_dir(config_dir);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::ReadError(_)));
    }

    #[test]
    fn test_add_assertion_for_submission() {
        let mut config = CliConfig::default();
        let assertion = AssertionForSubmission {
            assertion_contract: "test_contract".to_string(),
            assertion_id: "test_id".to_string(),
            signature: "test_signature".to_string(),
        };

        config.add_assertion_for_submission(assertion.clone());
        assert_eq!(config.assertions_for_submission.len(), 1);
        assert_eq!(
            config
                .assertions_for_submission
                .get("test_contract")
                .unwrap(),
            &assertion
        );
    }

    #[test]
    fn test_user_auth_display() {
        let auth = UserAuth {
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            user_address: Address::from_slice(&[0; 20]),
            expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(), // 2022-12-31 16:00:00 UTC
        };

        let display = format!("{}", auth);
        assert!(display.contains("User Address: 0x0000000000000000000000000000000000000000"));
        assert!(display.contains("Token Expired at"));
        assert!(display.contains("Access Token: [Set]"));
        assert!(display.contains("Refresh Token: [Set]"));
    }

    #[test]
    fn test_assertion_for_submission_display() {
        let assertion = AssertionForSubmission {
            assertion_contract: "test_contract".to_string(),
            assertion_id: "test_id".to_string(),
            signature: "test_signature".to_string(),
        };

        let display = format!("{}", assertion);
        assert!(display.contains("Contract: test_contract"));
        assert!(display.contains("ID: test_id"));
        assert!(display.contains("Signature: test_signa..."));
    }

    #[test]
    fn test_config_args_show() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::Show,
        };
        assert!(args.run(&mut config).is_ok());
    }

    #[test]
    fn test_config_args_delete() {
        let mut config = CliConfig {
            auth: Some(UserAuth {
                access_token: "test".to_string(),
                refresh_token: "test".to_string(),
                user_address: Address::from_slice(&[0; 20]),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
            }),
            assertions_for_submission: HashMap::new(),
        };
        let args = ConfigArgs {
            command: ConfigCommand::Delete,
        };
        assert!(args.run(&mut config).is_ok());
        assert!(config.auth.is_none());
        assert!(config.assertions_for_submission.is_empty());
    }

    #[test]
    fn test_write_to_file_permission_error() {
        let (config_dir, _temp_dir) = setup_config_dir();
        // Make the directory read-only
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o444)).unwrap();
        
        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(config_dir);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::WriteError(_)));
    }

    #[test]
    fn test_read_from_file_invalid_toml() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_file, "invalid toml content").unwrap();
        
        let result = CliConfig::read_from_file_at_dir(config_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_display_empty_config() {
        let config = CliConfig::default();
        let display = format!("{}", config);
        assert!(display.contains("Not authenticated"));
        assert!(display.contains("No pending assertions for submission"));
    }

    #[test]
    fn test_display_config_with_multiple_assertions() {
        let mut config = CliConfig::default();
        config.add_assertion_for_submission(AssertionForSubmission {
            assertion_contract: "contract1".to_string(),
            assertion_id: "id1".to_string(),
            signature: "sig1".to_string(),
        });
        config.add_assertion_for_submission(AssertionForSubmission {
            assertion_contract: "contract2".to_string(),
            assertion_id: "id2".to_string(),
            signature: "sig2".to_string(),
        });

        let display = format!("{}", config);
        assert!(display.contains("Assertion #1"));
        assert!(display.contains("Assertion #2"));
        assert!(display.contains("contract1"));
        assert!(display.contains("contract2"));
    }

    #[test]
    fn test_user_auth_serialization() {
        let auth = UserAuth {
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            user_address: Address::from_slice(&[0; 20]),
            expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
        };

        let serialized = toml::to_string(&auth).unwrap();
        let deserialized: UserAuth = toml::from_str(&serialized).unwrap();
        
        assert_eq!(auth.access_token, deserialized.access_token);
        assert_eq!(auth.refresh_token, deserialized.refresh_token);
        assert_eq!(auth.user_address, deserialized.user_address);
        assert_eq!(auth.expires_at, deserialized.expires_at);
    }

    #[test]
    fn test_assertion_for_submission_serialization() {
        let assertion = AssertionForSubmission {
            assertion_contract: "test_contract".to_string(),
            assertion_id: "test_id".to_string(),
            signature: "test_signature".to_string(),
        };

        let serialized = toml::to_string(&assertion).unwrap();
        let deserialized: AssertionForSubmission = toml::from_str(&serialized).unwrap();
        
        assert_eq!(assertion.assertion_contract, deserialized.assertion_contract);
        assert_eq!(assertion.assertion_id, deserialized.assertion_id);
        assert_eq!(assertion.signature, deserialized.signature);
    }
}
