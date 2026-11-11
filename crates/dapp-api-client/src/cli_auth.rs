use crate::generated::client::{
    Client as GeneratedClient,
    Error,
    ResponseValue,
    types::{
        ApiError,
        CliAuthStatusResponse,
    },
};
use progenitor_client::ClientInfo;
use reqwest::header::{
    ACCEPT,
    CONTENT_TYPE,
    HeaderValue,
};
use uuid::Uuid;

pub enum CliAuthStatusPostError {
    RouteMissing,
    Call(Box<Error<ApiError>>),
}

/// Send `session_id` and `device_secret` in a JSON body, never in the URL.
pub async fn post_cli_auth_status(
    client: &GeneratedClient,
    session_id: &Uuid,
    device_secret: &str,
) -> Result<CliAuthStatusResponse, CliAuthStatusPostError> {
    let url = format!("{}/cli/auth/status", client.baseurl);
    let response = client
        .client
        .post(url)
        .header(ACCEPT, HeaderValue::from_static("application/json"))
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header(
            "api-version",
            HeaderValue::from_static(<GeneratedClient as ClientInfo<()>>::api_version()),
        )
        .json(&serde_json::json!({
            "session_id": session_id,
            "device_secret": device_secret,
        }))
        .send()
        .await
        .map_err(|error| {
            CliAuthStatusPostError::Call(Box::new(Error::CommunicationError(error)))
        })?;

    match response.status().as_u16() {
        404 | 405 => Err(CliAuthStatusPostError::RouteMissing),
        200 => {
            match ResponseValue::<CliAuthStatusResponse>::from_response::<ApiError>(response).await
            {
                Ok(parsed) => Ok(parsed.into_inner()),
                Err(error) => Err(CliAuthStatusPostError::Call(Box::new(error))),
            }
        }
        400 | 500 => {
            match ResponseValue::<ApiError>::from_response::<ApiError>(response).await {
                Ok(parsed) => {
                    Err(CliAuthStatusPostError::Call(Box::new(
                        Error::ErrorResponse(parsed),
                    )))
                }
                Err(error) => Err(CliAuthStatusPostError::Call(Box::new(error))),
            }
        }
        _ => {
            Err(CliAuthStatusPostError::Call(Box::new(
                Error::UnexpectedResponse(response),
            )))
        }
    }
}
