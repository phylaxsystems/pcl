//! File-injected credentials are never persisted in the interactive session store.
use crate::error::AuthError;
use reqwest::header::HeaderValue;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    platform_url: url::Url,
    token: String,
}

pub fn configured() -> bool {
    std::env::var_os("PCL_API_TOKEN_FILE").is_some()
}

fn invalid(message: &str) -> AuthError {
    AuthError::ApiToken(message.to_owned())
}

pub fn header(platform: &url::Url) -> Result<Option<HeaderValue>, AuthError> {
    let Some(path) = std::env::var_os("PCL_API_TOKEN_FILE") else {
        return Ok(None);
    };
    let bytes = std::fs::read(path).map_err(|_| invalid("Cannot read PCL_API_TOKEN_FILE"))?;
    parse_header(&bytes, platform).map(Some)
}

fn parse_header(bytes: &[u8], platform: &url::Url) -> Result<HeaderValue, AuthError> {
    let credential: Credential = serde_json::from_slice(bytes)
        .map_err(|_| invalid("PCL_API_TOKEN_FILE must contain platform_url and token"))?;
    let issuer = &credential.platform_url;
    if issuer.origin() != platform.origin()
        || !issuer.username().is_empty()
        || issuer.password().is_some()
        || issuer.query().is_some()
        || issuer.fragment().is_some()
        || issuer.path() != "/"
        || !(issuer.scheme() == "https"
            || (issuer.scheme() == "http"
                && matches!(issuer.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
    {
        return Err(invalid(
            "Injected credential platform mismatch or insecure platform URL",
        ));
    }
    let Some(secret) = credential.token.strip_prefix("cl_api_") else {
        return Err(invalid("Invalid injected API token format"));
    };
    if secret.len() != 43
        || !secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(invalid("Invalid injected API token format"));
    }
    let mut value = HeaderValue::from_str(&format!("Bearer {}", credential.token))
        .map_err(|_| invalid("Invalid injected API token format"))?;
    value.set_sensitive(true);
    Ok(value)
}

pub async fn status(platform: &url::Url) -> Result<serde_json::Value, AuthError> {
    let value = header(platform)?.ok_or_else(|| invalid("PCL_API_TOKEN_FILE is not configured"))?;
    let mut url = platform.clone();
    url.set_path("/api/v1/auth/api-tokens/current");
    url.set_query(None);
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| invalid("Cannot build token validation client"))?
        .get(url)
        .header(reqwest::header::AUTHORIZATION, value)
        .send()
        .await
        .map_err(|_| invalid("API token validation request failed"))?;
    if !response.status().is_success() {
        return Err(invalid(&format!(
            "API token validation failed (HTTP {})",
            response.status().as_u16()
        )));
    }
    response
        .json()
        .await
        .map_err(|_| invalid("Invalid API token validation response"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_is_bound_to_secure_origin_and_redacted() {
        let bytes = serde_json::to_vec(&serde_json::json!({"platform_url":"https://example.com", "token": format!("cl_api_{}", "a".repeat(43))})).unwrap();
        let value = parse_header(&bytes, &"https://example.com".parse().unwrap()).unwrap();
        assert!(value.is_sensitive());
        assert!(parse_header(&bytes, &"https://other.com".parse().unwrap()).is_err());
        assert!(parse_header(&bytes, &"http://example.com".parse().unwrap()).is_err());
        assert!(parse_header(b"broken secret", &"https://example.com".parse().unwrap()).is_err());
    }
}
