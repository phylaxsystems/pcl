use crate::{
    auth::refresh_stored_auth,
    config::{
        AUTH_EXPIRES_SOON_SECONDS,
        CliConfig,
    },
    error::AuthError,
};
use chrono::{
    DateTime,
    Utc,
};
use dapp_api_client::generated::client::Client as GeneratedClient;
use pcl_common::args::CliArgs;

#[derive(Debug, thiserror::Error)]
pub enum ClientBuildError {
    #[error("Run `pcl auth login` first")]
    NoAuthToken,

    #[error(
        "Stored auth token expired at {0}. Run `pcl auth refresh --toon` or `pcl auth login` again."
    )]
    ExpiredAuthToken(DateTime<Utc>),

    #[error("Failed to refresh stored auth before building an authenticated API client: {0}")]
    AuthRefresh(#[source] AuthError),

    #[error("Invalid config: {0}")]
    InvalidConfig(String),
}

pub async fn ensure_fresh_auth(
    config: &mut CliConfig,
    auth_url: &url::Url,
    cli_args: &CliArgs,
) -> Result<(), ClientBuildError> {
    let auth = config.auth.as_ref().ok_or(ClientBuildError::NoAuthToken)?;
    let now = Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    if auth.expires_at <= now || seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS {
        refresh_stored_auth(config, auth_url, cli_args, false)
            .await
            .map_err(ClientBuildError::AuthRefresh)?;
    }
    validate_auth(config).map(|_| ())
}

pub fn authenticated_client(
    config: &CliConfig,
    api_url: &url::Url,
) -> Result<GeneratedClient, ClientBuildError> {
    let mut base = api_url.clone();
    base.set_path("/api/v1");
    let base_url = base.to_string();

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        authorization_header(config)?,
    );

    let http_client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .map_err(|e| {
            ClientBuildError::InvalidConfig(format!("Failed to build HTTP client: {e}"))
        })?;

    Ok(GeneratedClient::new_with_client(&base_url, http_client))
}

pub fn authorization_header(
    config: &CliConfig,
) -> Result<reqwest::header::HeaderValue, ClientBuildError> {
    let auth = validate_auth(config)?;
    let auth_value = format!("Bearer {}", auth.access_token);
    reqwest::header::HeaderValue::from_str(&auth_value)
        .map_err(|e| ClientBuildError::InvalidConfig(format!("Invalid auth token: {e}")))
}

fn validate_auth(config: &CliConfig) -> Result<&crate::config::UserAuth, ClientBuildError> {
    let auth = config.auth.as_ref().ok_or(ClientBuildError::NoAuthToken)?;
    if auth.access_token.trim().is_empty() {
        return Err(ClientBuildError::NoAuthToken);
    }
    if auth.expires_at <= Utc::now() {
        return Err(ClientBuildError::ExpiredAuthToken(auth.expires_at));
    }
    Ok(auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UserAuth;

    #[tokio::test]
    async fn ensure_fresh_auth_refreshes_expired_token_before_header_use() {
        let mut server = mockito::Server::new_async().await;
        let _refresh = server
            .mock("POST", "/api/v1/auth/refresh")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "refresh_token": "old-refresh"
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"token":"new-access","refresh_token":"new-refresh","expires_at":"2030-01-01T00:00:00Z","refresh_expires_at":"2030-02-01T00:00:00Z"}"#,
            )
            .create_async()
            .await;
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let mut config = CliConfig {
            auth: Some(UserAuth {
                access_token: "expired-token".to_string(),
                refresh_token: "old-refresh".to_string(),
                expires_at: DateTime::from_timestamp(1, 0).expect("valid timestamp"),
                refresh_expires_at: None,
                platform_url: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
        };
        let auth_url = url::Url::parse(&server.url()).expect("mock url");

        ensure_fresh_auth(&mut config, &auth_url, &cli_args)
            .await
            .expect("refresh auth");

        let auth = config.auth.as_ref().expect("auth present");
        assert_eq!(auth.access_token, "new-access");
        assert_eq!(auth.refresh_token, "new-refresh");
        let header = authorization_header(&config).expect("auth header");
        assert_eq!(header.to_str().expect("header utf8"), "Bearer new-access");
    }

    #[test]
    fn authorization_header_rejects_expired_tokens() {
        let config = CliConfig {
            auth: Some(UserAuth {
                access_token: "expired-token".to_string(),
                refresh_token: String::new(),
                expires_at: DateTime::from_timestamp(1, 0).expect("valid timestamp"),
                refresh_expires_at: None,
                platform_url: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
        };

        let error = authorization_header(&config).expect_err("expired auth rejected");
        assert!(matches!(error, ClientBuildError::ExpiredAuthToken(_)));
    }
}
