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
        405 => Err(CliAuthStatusPostError::RouteMissing),
        404 => Err(not_found_error(response).await),
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

/// A server without the POST route answers through the ts-rest catch-all with
/// exactly `{"message":"Not Found"}`. Any other 404 comes from a server that
/// has the route (or a proxy) and must not trigger the GET that puts the
/// secret in the URL.
async fn not_found_error(response: reqwest::Response) -> CliAuthStatusPostError {
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return CliAuthStatusPostError::Call(Box::new(Error::ResponseBodyError(error)));
        }
    };
    if serde_json::from_slice::<serde_json::Value>(&bytes).ok()
        == Some(serde_json::json!({ "message": "Not Found" }))
    {
        return CliAuthStatusPostError::RouteMissing;
    }
    let error = match serde_json::from_slice::<ApiError>(&bytes) {
        Ok(body) => Error::ErrorResponse(ResponseValue::new(body, status, headers)),
        Err(error) => Error::InvalidResponsePayload(bytes, error),
    };
    CliAuthStatusPostError::Call(Box::new(error))
}
