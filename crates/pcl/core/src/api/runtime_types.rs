use super::{
    ApiCommandError,
    HttpMethod,
    WorkflowOperation,
};
use reqwest::header::HeaderMap;
use serde_json::Value;
use std::path::PathBuf;

pub(in crate::api) struct ApiRequestInput<'a> {
    pub(in crate::api) method: HttpMethod,
    pub(in crate::api) path: &'a str,
    pub(in crate::api) query: &'a [String],
    pub(in crate::api) header: &'a [String],
    pub(in crate::api) body: Option<&'a str>,
    pub(in crate::api) body_file: Option<&'a PathBuf>,
    pub(in crate::api) field: &'a [String],
    pub(in crate::api) require_auth: bool,
}

pub(in crate::api) struct PreparedApiRequest<'a> {
    pub(in crate::api) attach_auth: bool,
    pub(in crate::api) method: HttpMethod,
    pub(in crate::api) url: &'a url::Url,
    pub(in crate::api) headers: &'a HeaderMap,
    pub(in crate::api) query: &'a [(String, String)],
    pub(in crate::api) body: Option<&'a Value>,
}

#[derive(Clone, Copy)]
pub(in crate::api) struct RawPaginationOptions<'a> {
    pub(in crate::api) item_field: &'a str,
    pub(in crate::api) start_page: u64,
    pub(in crate::api) limit: u64,
    pub(in crate::api) page_param: &'a str,
    pub(in crate::api) limit_param: &'a str,
    pub(in crate::api) max_pages: u64,
}

#[derive(Clone, Copy)]
pub(in crate::api) struct WorkflowPaginationOptions<'a> {
    pub(in crate::api) item_field: &'a str,
    pub(in crate::api) start_page: u64,
    pub(in crate::api) limit: u64,
    pub(in crate::api) max_pages: u64,
}

#[derive(Debug)]
pub(in crate::api) struct WorkflowCallResult {
    pub(in crate::api) body: Value,
    pub(in crate::api) request: Value,
    pub(in crate::api) response: Value,
}

#[derive(Clone, Debug)]
pub(in crate::api) struct WorkflowRequest {
    pub(in crate::api) method: HttpMethod,
    pub(in crate::api) operation_id: &'static str,
    pub(in crate::api) path: String,
    pub(in crate::api) query: Vec<(String, String)>,
    pub(in crate::api) body: Option<String>,
    pub(in crate::api) require_auth: bool,
    pub(in crate::api) attach_auth: bool,
    pub(in crate::api) next_actions: Vec<String>,
}

impl WorkflowRequest {
    pub(in crate::api) fn with_optional_auth(mut self) -> Self {
        self.attach_auth = true;
        self
    }

    pub(in crate::api) fn from_operation(
        operation: WorkflowOperation,
        query: Vec<(String, String)>,
        body: Option<String>,
        require_auth: bool,
        next_actions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, ApiCommandError> {
        Ok(Self {
            method: operation.method,
            operation_id: operation.operation_id,
            path: operation.path()?,
            query,
            body,
            require_auth,
            attach_auth: require_auth,
            next_actions: next_actions.into_iter().map(Into::into).collect(),
        })
    }
}
