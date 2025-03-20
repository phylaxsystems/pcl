use crate::error::ConfigError;
use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use colored::Colorize;
use dirs::home_dir;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

pub const CONFIG_DIR: &str = ".pcl";
pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CliConfig {
    pub auth: Option<UserAuth>,
    pub assertions_for_submission: Vec<AssertionForSubmission>,
}

impl CliConfig {
    pub fn write_to_file(&self) -> Result<(), ConfigError> {
        self.write_to_file_at_dir(Self::get_config_dir())
    }

    fn write_to_file_at_dir(&self, config_dir: PathBuf) -> Result<(), ConfigError> {
        std::fs::create_dir_all(&config_dir).map_err(ConfigError::WriteError)?;
        let config_file = config_dir.join(CONFIG_FILE);
        let config_str = toml::to_string(self).unwrap();
        std::fs::write(config_file, config_str).map_err(ConfigError::WriteError)?;
        Ok(())
    }

    fn get_config_dir() -> PathBuf {
        home_dir().unwrap().join(CONFIG_DIR)
    }

    fn read_from_file_at_dir(config_dir: PathBuf) -> Result<Self, ConfigError> {
        let config_file = config_dir.join(CONFIG_FILE);
        let config_str = std::fs::read_to_string(config_file).map_err(ConfigError::ReadError)?;
        Ok(toml::from_str(&config_str).unwrap())
    }

    pub fn read_from_file() -> Result<Self, ConfigError> {
        Self::read_from_file_at_dir(Self::get_config_dir())
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
            for (i, assertion) in self.assertions_for_submission.iter().enumerate() {
                writeln!(f, "Assertion #{}: {}", i + 1, assertion)?;
            }
        } else {
            writeln!(f, "\nNo pending assertions for submission")?;
        }

        Ok(())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UserAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub user_address: Address,
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
        writeln!(f, "  Refresh Token: [Set]")
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AssertionForSubmission {
    pub assertion_contract: String,
    pub assertion_id: String,
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
    use tempfile::TempDir;

    // Helper function to set up a temporary config directory
    fn setup_config_dir() -> (PathBuf, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        env::set_var("HOME", temp_dir.path());
        (temp_dir.path().join(CONFIG_DIR), temp_dir)
    }

    #[test]
    fn test_write_and_read_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        let config = CliConfig {
            auth: Some(UserAuth {
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                user_address: Address::from_slice(&[0; 20]),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
            }),
            assertions_for_submission: vec![AssertionForSubmission {
                assertion_contract: "contract1".to_string(),
                assertion_id: "id1".to_string(),
                signature: "sig1".to_string(),
            }],
        };

        // Test writing
        assert!(config.write_to_file_at_dir(config_dir.clone()).is_ok());

        // Test reading
        let read_config = CliConfig::read_from_file_at_dir(config_dir).unwrap();
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
            read_config.assertions_for_submission[0].assertion_contract,
            "contract1"
        );
    }

    #[test]
    fn test_read_nonexistent_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Try reading without creating a file
        let result = CliConfig::read_from_file_at_dir(config_dir);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ConfigError::ReadError(_)));
    }

}
