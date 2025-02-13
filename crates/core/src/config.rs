use crate::error::ConfigError;


#[derive(Debug, Default)]
pub struct CliConfig {
    pub auth: Option<UserAuth>,
    pub assertions_for_submission: Vec<AssertionForSubmission>
}

impl CliConfig {
}


#[derive(Debug, Default)]
pub struct UserAuth {
    pub access_token: String,
    pub refresh_token: String,
    pub user_address: String,
}

#[derive(Debug, Default)]
pub struct AssertionForSubmission {
    assertion: String,
    id: String,
}