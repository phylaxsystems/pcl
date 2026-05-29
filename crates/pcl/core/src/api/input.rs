use super::ApiCommandError;
use reqwest::header::{
    HeaderMap,
    HeaderName,
    HeaderValue,
};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::PathBuf,
    str::FromStr,
};

pub(in crate::api) fn split_path_and_inline_query(
    input: &str,
) -> Result<(String, Vec<(String, String)>), ApiCommandError> {
    if !input.starts_with('/') {
        return Err(ApiCommandError::InvalidPath(input.to_string()));
    }
    let Some((path, query)) = input.split_once('?') else {
        return Ok((input.to_string(), Vec::new()));
    };
    if path.is_empty() || !path.starts_with('/') {
        return Err(ApiCommandError::InvalidPath(input.to_string()));
    }
    let query = url::form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    Ok((path.to_string(), query))
}

pub(in crate::api) fn parse_key_values(
    kind: &'static str,
    entries: &[String],
) -> Result<Vec<(String, String)>, ApiCommandError> {
    entries
        .iter()
        .map(|entry| {
            let (key, value) = entry.split_once('=').ok_or_else(|| {
                ApiCommandError::InvalidKeyValue {
                    kind,
                    input: entry.clone(),
                }
            })?;
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

pub(in crate::api) fn parse_headers(entries: &[String]) -> Result<HeaderMap, ApiCommandError> {
    let mut headers = HeaderMap::new();

    for entry in entries {
        let (name, value) = entry.split_once('=').ok_or_else(|| {
            ApiCommandError::InvalidKeyValue {
                kind: "header",
                input: entry.clone(),
            }
        })?;
        let header_name = HeaderName::from_str(name).map_err(|source| {
            ApiCommandError::InvalidHeaderName {
                name: name.to_string(),
                source,
            }
        })?;
        let header_value = HeaderValue::from_str(value).map_err(|source| {
            ApiCommandError::InvalidHeaderValue {
                name: name.to_string(),
                source,
            }
        })?;
        headers.insert(header_name, header_value);
    }

    Ok(headers)
}

pub(in crate::api) fn read_body(
    body: Option<&str>,
    body_file: Option<&PathBuf>,
) -> Result<Option<String>, ApiCommandError> {
    if let Some(body) = body {
        return Ok(Some(body.to_string()));
    }

    if let Some(path) = body_file {
        if path.as_os_str() == "-" {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(ApiCommandError::Stdin)?;
            return Ok(Some(body));
        }

        return fs::read_to_string(path).map(Some).map_err(|source| {
            ApiCommandError::BodyFile {
                path: path.clone(),
                source,
            }
        });
    }

    Ok(None)
}

pub(in crate::api) fn write_json_output_file(
    path: &PathBuf,
    value: &Value,
) -> Result<(), ApiCommandError> {
    let body = serde_json::to_string_pretty(value)?;
    fs::write(path, body).map_err(|source| {
        ApiCommandError::OutputFile {
            path: path.clone(),
            source,
        }
    })
}

pub(in crate::api) fn write_jsonl_items_output_file(
    path: &PathBuf,
    value: &Value,
) -> Result<(), ApiCommandError> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiCommandError::InvalidWorkflow {
                message: "--jsonl output requires paginated data with an items array".to_string(),
            }
        })?;
    let mut body = String::new();
    for item in items {
        body.push_str(&serde_json::to_string(item)?);
        body.push('\n');
    }
    fs::write(path, body).map_err(|source| {
        ApiCommandError::OutputFile {
            path: path.clone(),
            source,
        }
    })
}
