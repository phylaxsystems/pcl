use crate::error::ConfigError;
use dirs::home_dir;
use serde::{Deserialize, Serialize};
pub const CONFIG_DIR: &str = ".pcl";
pub const CONFIG_FILE: &str = "config.toml";


#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CliConfig{
    pub auth: Option<UserAuth>,
    pub assertions_for_submission: Vec<AssertionForSubmission>
}

impl CliConfig{ 
    pub fn write_to_file(&self) -> Result<(), ConfigError> {
        let config_dir = home_dir().unwrap().join(CONFIG_DIR);
        let config_file = config_dir.join(CONFIG_FILE);
        let config_str = toml::to_string(self).unwrap();
        std::fs::write(config_file, config_str).map_err(|e| ConfigError::WriteError(e))?;
        Ok(())
    }

    pub fn read_or_default() -> Self {
        Self::read_from_file().unwrap_or_default()
    }

    pub fn read_from_file() -> Result<Self, ConfigError> {
        let config_dir = home_dir().unwrap().join(CONFIG_DIR);
        let config_file = config_dir.join(CONFIG_FILE);
        let config_str = std::fs::read_to_string(config_file).map_err(|e| ConfigError::ReadError(e))?;
        Ok(toml::from_str(&config_str).unwrap())
    }

    pub fn must_be_authenticated(&self) -> Result<(), ConfigError> {
        if self.auth.is_none() {
            return Err(ConfigError::NotAuthenticated);
        }
        Ok(())
    }
}


#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UserAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub user_address: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AssertionForSubmission {
    assertion: String,
    id: String,
}