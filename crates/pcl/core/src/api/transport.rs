use super::ApiCommandError;
use reqwest::header::HeaderMap;
use serde_json::{
    Map,
    Value,
    json,
};
use std::path::Path;

pub(in crate::api) struct ApiResponsePayload {
    pub(in crate::api) status: reqwest::StatusCode,
    pub(in crate::api) request_id: Option<String>,
    pub(in crate::api) headers: Map<String, Value>,
    pub(in crate::api) body: Value,
}

pub(crate) fn request_id_from_headers(headers: &HeaderMap) -> Option<String> {
    [
        "x-request-id",
        "x-correlation-id",
        "x-amzn-requestid",
        "cf-ray",
        "request-id",
    ]
    .into_iter()
    .find_map(|name| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

pub(in crate::api) fn write_request_log(
    request_log_path: &Path,
    kind: &str,
    method: &str,
    path: &str,
    status: u16,
    request_id: Option<&str>,
    operation_id: Option<&str>,
) {
    #[cfg(not(test))]
    {
        let _ = crate::request_log::append_request_record_at(
            request_log_path,
            &json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "kind": kind,
                "method": method,
                "path": path,
                "status": status,
                "success": (200..=299).contains(&status),
                "request_id": request_id,
                "operation_id": operation_id,
            }),
        );
    }
    #[cfg(test)]
    let _ = (
        request_log_path,
        kind,
        method,
        path,
        status,
        request_id,
        operation_id,
    );
}

pub(in crate::api) async fn read_api_response(
    response: reqwest::Response,
) -> Result<ApiResponsePayload, ApiCommandError> {
    let status = response.status();
    let request_id = request_id_from_headers(response.headers());
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), json!(value)))
        })
        .collect::<Map<_, _>>();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.bytes().await?;
    let body = response_body_value(&content_type, &bytes);

    Ok(ApiResponsePayload {
        status,
        request_id,
        headers,
        body,
    })
}

pub(crate) fn response_body_value(content_type: &str, bytes: &[u8]) -> Value {
    if content_type.contains("application/json") {
        return serde_json::from_slice(bytes).unwrap_or_else(|_| {
            json!({
                "parse_error": "response declared JSON but could not be parsed",
                "raw": String::from_utf8_lossy(bytes),
            })
        });
    }

    serde_json::from_slice(bytes)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(bytes).to_string()))
}
