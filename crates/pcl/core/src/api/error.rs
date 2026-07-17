use crate::{
    error::AuthError,
    output::with_envelope_metadata,
};
use serde_json::{
    Map,
    Value,
    json,
};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ApiCommandError {
    #[error("Run `pcl auth login` first, or pass `--allow-unauthenticated`")]
    NoAuthToken,

    #[error(
        "Stored auth token expired at {0}. Run `pcl auth refresh --json` or `pcl auth login` again, or pass `--allow-unauthenticated` for public endpoints."
    )]
    ExpiredAuthToken(chrono::DateTime<chrono::Utc>),

    #[error("Failed to refresh stored auth before retrying the API request: {0}")]
    AuthRefresh(#[source] AuthError),

    #[error(transparent)]
    PlatformMismatch(AuthError),

    #[error("Invalid {kind} `{input}`. Expected KEY=VALUE.")]
    InvalidKeyValue { kind: &'static str, input: String },

    #[error("Invalid header name `{name}`: {source}")]
    InvalidHeaderName {
        name: String,
        #[source]
        source: reqwest::header::InvalidHeaderName,
    },

    #[error("Invalid header value for `{name}`: {source}")]
    InvalidHeaderValue {
        name: String,
        #[source]
        source: reqwest::header::InvalidHeaderValue,
    },

    #[error("Invalid API path `{0}`. Paths must start with `/`.")]
    InvalidPath(String),

    #[error("Failed to build API URL: {0}")]
    Url(#[from] url::ParseError),

    #[error("Failed to read body file `{path}`: {source}")]
    BodyFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read request log `{path}`: {source}")]
    RequestLog {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to write output file `{path}`: {source}")]
    OutputFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to read request body from stdin: {0}")]
    Stdin(std::io::Error),

    #[error("Invalid JSON body: {0}")]
    Json(#[from] serde_json::Error),

    #[error("API request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("API request failed with status {status} for {method} {path}")]
    HttpStatus {
        method: &'static str,
        path: String,
        status: u16,
        request_id: Option<String>,
        body: Box<Value>,
    },

    #[error("OpenAPI spec does not contain a paths object")]
    MissingPaths,

    #[error("No API operation matched `{0}`")]
    OperationNotFound(String),

    #[error("{message}")]
    InvalidWorkflow { message: String },

    #[error("{message}")]
    InvalidWorkflowWithActions {
        message: String,
        next_actions: Vec<String>,
    },

    #[error(transparent)]
    Wallet(#[from] crate::wallet::WalletError),

    #[error(transparent)]
    Onchain(#[from] crate::onchain::OnchainError),

    #[error("Broadcast cancelled")]
    BroadcastCancelled,

    #[error(
        "Transaction {tx_hash} confirmed on-chain but the follow-up API confirmation failed: {source}. The chain state is updated; re-run the command to reconcile the platform."
    )]
    ConfirmAfterTx {
        tx_hash: alloy_primitives::B256,
        #[source]
        source: Box<ApiCommandError>,
    },

    #[error("Unexpected {endpoint} response shape: {reason}")]
    UnexpectedResponse {
        endpoint: &'static str,
        reason: String,
    },
}

impl ApiCommandError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoAuthToken => "auth.no_token",
            Self::ExpiredAuthToken(_) => "auth.expired_token",
            Self::AuthRefresh(_) => "auth.refresh_failed",
            Self::PlatformMismatch(_) => "auth.platform_mismatch",
            Self::InvalidKeyValue { .. } => "input.invalid_key_value",
            Self::InvalidHeaderName { .. } => "input.invalid_header_name",
            Self::InvalidHeaderValue { .. } => "input.invalid_header_value",
            Self::InvalidPath(_) => "input.invalid_path",
            Self::Url(_) => "input.invalid_url",
            Self::BodyFile { .. } => "input.body_file_read_failed",
            Self::RequestLog { .. } => "request_log.read_failed",
            Self::OutputFile { .. } => "output.file_write_failed",
            Self::Stdin(_) => "input.stdin_read_failed",
            Self::Json(_) => "input.invalid_json",
            Self::Request(source) => {
                match source.status().map(|status| status.as_u16()) {
                    Some(400) => "api.bad_request",
                    Some(401) => "auth.unauthorized",
                    Some(403) => "auth.forbidden",
                    Some(404) => "api.not_found",
                    Some(422) => "api.validation_failed",
                    Some(500..=599) => "api.server_error",
                    _ => "network.request_failed",
                }
            }
            Self::HttpStatus { status, .. } => {
                match *status {
                    400 => "api.bad_request",
                    401 => "auth.unauthorized",
                    403 => "auth.forbidden",
                    404 => "api.not_found",
                    422 => "api.validation_failed",
                    500..=599 => "api.server_error",
                    _ => "api.request_failed",
                }
            }
            Self::MissingPaths => "openapi.missing_paths",
            Self::OperationNotFound(_) => "openapi.operation_not_found",
            Self::InvalidWorkflow { .. } | Self::InvalidWorkflowWithActions { .. } => {
                "workflow.invalid_arguments"
            }
            Self::Wallet(_) => "wallet.failed",
            Self::Onchain(source) => {
                match source {
                    crate::onchain::OnchainError::RpcUrlMissing { .. }
                    | crate::onchain::OnchainError::InvalidRpcUrl { .. } => "onchain.rpc_missing",
                    crate::onchain::OnchainError::ChainIdMismatch { .. } => {
                        "onchain.chain_mismatch"
                    }
                    crate::onchain::OnchainError::Reverted { .. } => "onchain.tx_reverted",
                    _ => "onchain.failed",
                }
            }
            Self::BroadcastCancelled => "broadcast.cancelled",
            Self::ConfirmAfterTx { .. } => "broadcast.confirm_failed",
            Self::UnexpectedResponse { .. } => "api.unexpected_response",
        }
    }

    pub fn recoverable(&self) -> bool {
        !matches!(self, Self::MissingPaths)
    }

    pub fn next_actions(&self) -> Vec<String> {
        match self {
            Self::NoAuthToken | Self::ExpiredAuthToken(_) | Self::AuthRefresh(_) => {
                vec![
                    "pcl auth refresh --json".to_string(),
                    "pcl auth login".to_string(),
                    "pcl api list --allow-unauthenticated --json".to_string(),
                ]
            }
            Self::PlatformMismatch(_) => {
                vec![
                    "pcl auth login --auth-url <platform-url>".to_string(),
                    "pcl auth status --json".to_string(),
                    "pcl api list --allow-unauthenticated --json".to_string(),
                ]
            }
            Self::InvalidPath(_) => {
                vec![
                    "pcl api list --json".to_string(),
                    "pcl api call get /views/public/incidents --allow-unauthenticated --json"
                        .to_string(),
                ]
            }
            Self::InvalidKeyValue { kind, .. } => {
                vec![format!(
                    "Use --{kind} key=value, for example: pcl api call get /views/public/incidents --{kind} limit=5"
                )]
            }
            Self::InvalidHeaderName { .. } | Self::InvalidHeaderValue { .. } => {
                vec![
                    "Use --header name=value, for example: --header x-cl-dev-mode=true".to_string(),
                ]
            }
            Self::Json(_) => {
                vec![
                    "Use --field key=value for simple request bodies".to_string(),
                    "Use --body-file request.json for nested request bodies".to_string(),
                ]
            }
            Self::OperationNotFound(_) => {
                vec![
                    "pcl api list --json".to_string(),
                    "pcl api inspect get /views/public/incidents --json".to_string(),
                ]
            }
            Self::InvalidWorkflowWithActions { next_actions, .. } => next_actions.clone(),
            Self::InvalidWorkflow { .. } => {
                vec![
                    "pcl projects mine".to_string(),
                    "pcl schema list".to_string(),
                    "pcl workflows".to_string(),
                ]
            }
            Self::Request(source)
                if matches!(
                    source.status().map(|status| status.as_u16()),
                    Some(401 | 403)
                ) =>
            {
                vec![
                    "pcl auth login".to_string(),
                    "Use --allow-unauthenticated only for public endpoints".to_string(),
                ]
            }
            Self::HttpStatus { status: 401, .. } => {
                vec![
                    "pcl auth refresh --json".to_string(),
                    "pcl auth login".to_string(),
                    "Use --allow-unauthenticated only for public endpoints".to_string(),
                ]
            }
            Self::HttpStatus { status: 403, .. } => {
                vec![
                    "Read error.http.body for the API-provided reason".to_string(),
                    "Check whether the endpoint is enabled and your user has permission"
                        .to_string(),
                    "Use --allow-unauthenticated only for endpoints documented as public"
                        .to_string(),
                ]
            }
            Self::HttpStatus {
                method,
                path,
                status: 400 | 422,
                ..
            } => {
                vec![
                    format!(
                        "pcl api inspect {} {} --json",
                        method.to_ascii_lowercase(),
                        path
                    ),
                    "pcl api manifest --json".to_string(),
                    "Read error.http.body for the rejected field details".to_string(),
                ]
            }
            Self::HttpStatus { status: 404, .. } => {
                vec![
                    "Check the project ID, slug, or API path and retry".to_string(),
                    "pcl projects mine".to_string(),
                ]
            }
            Self::HttpStatus {
                method,
                status: status @ 500..=599,
                request_id,
                path,
                ..
            } => {
                let mut actions = if mutation_outcome_ambiguous(method, *status) {
                    vec![
                        format!(
                            "Do not retry immediately; inspect the target resource for {} {} to confirm whether the mutation applied",
                            method.to_ascii_lowercase(),
                            path
                        ),
                        "pcl requests list --json".to_string(),
                        "Read error.http.body for API-provided failure details".to_string(),
                    ]
                } else {
                    vec![
                        "Retry the same command once; server errors can be transient".to_string(),
                        "pcl api manifest --json".to_string(),
                        "Read error.http.body for API-provided failure details".to_string(),
                    ]
                };
                if let Some(request_id) = request_id {
                    actions.push(format!(
                        "Include request_id {request_id} when reporting this server error"
                    ));
                }
                actions
            }
            Self::HttpStatus { .. } => {
                vec![
                    "pcl api manifest --json".to_string(),
                    "Read error.http.body for API-provided failure details".to_string(),
                ]
            }
            Self::Request(source) if source.status().map(|status| status.as_u16()) == Some(404) => {
                vec![
                    "Check the project ID, slug, or API path and retry".to_string(),
                    "pcl projects mine".to_string(),
                ]
            }
            Self::Request(_) | Self::Url(_) => {
                vec!["Check --api-url and your network connection, then retry".to_string()]
            }
            Self::BodyFile { .. } => {
                vec!["Check --body-file path or pass --body directly".to_string()]
            }
            Self::RequestLog { .. } => {
                vec![
                    "pcl requests path --json".to_string(),
                    "Check request log permissions or move the PCL state directory".to_string(),
                ]
            }
            Self::OutputFile { .. } => {
                vec!["Check --output path permissions or choose a writable file".to_string()]
            }
            Self::Stdin(_) => vec!["Pipe a JSON body into --body-file -".to_string()],
            Self::MissingPaths => {
                vec!["Check that /api/v1/openapi returns an OpenAPI document".to_string()]
            }
            Self::Wallet(_) => {
                vec![
                    "Pass --private-key or set PCL_PRIVATE_KEY".to_string(),
                    "Or pass --account <keystore-name> with PCL_KEYSTORE_PASSWORD".to_string(),
                ]
            }
            Self::Onchain(crate::onchain::OnchainError::RpcUrlMissing { chain_id }) => {
                vec![
                    format!("pcl config set-rpc {chain_id} <rpc-url>"),
                    "Or pass --rpc-url / set PCL_RPC_URL".to_string(),
                ]
            }
            Self::Onchain(_) => {
                vec![
                    "Check --rpc-url, the target chain id, and wallet funds, then retry"
                        .to_string(),
                ]
            }
            Self::BroadcastCancelled => {
                vec!["Re-run with --yes to skip the confirmation prompt".to_string()]
            }
            Self::ConfirmAfterTx { tx_hash, .. } => {
                vec![
                    format!(
                        "Re-run the same command to reconcile; transaction {tx_hash} already landed on-chain"
                    ),
                    "pcl requests list --toon".to_string(),
                ]
            }
            Self::UnexpectedResponse { endpoint, .. } => {
                vec![format!("pcl api inspect get {endpoint} --toon")]
            }
        }
    }

    pub fn suggested_next_actions(&self) -> Vec<&'static str> {
        match self {
            Self::NoAuthToken | Self::ExpiredAuthToken(_) | Self::AuthRefresh(_) => {
                vec!["refresh_or_login", "retry"]
            }
            Self::PlatformMismatch(_) => vec!["refresh_or_login"],
            Self::InvalidKeyValue { .. }
            | Self::InvalidHeaderName { .. }
            | Self::InvalidHeaderValue { .. }
            | Self::InvalidPath(_)
            | Self::Json(_)
            | Self::InvalidWorkflow { .. }
            | Self::InvalidWorkflowWithActions { .. } => vec!["fix_input", "retry"],
            Self::OperationNotFound(_) | Self::MissingPaths => vec!["inspect_manifest"],
            Self::Request(source) if source.status().map(|status| status.as_u16()) == Some(404) => {
                vec!["check_ids", "retry"]
            }
            Self::Request(_) | Self::Url(_) => vec!["check_network", "retry"],
            Self::BodyFile { .. } | Self::Stdin(_) => vec!["fix_body_input", "retry"],
            Self::RequestLog { .. } => vec!["inspect_request_log", "retry"],
            Self::OutputFile { .. } => vec!["fix_output_path", "retry"],
            Self::HttpStatus { status: 401, .. } => vec!["refresh_or_login", "retry"],
            Self::HttpStatus { status: 403, .. } => {
                vec!["check_permissions", "inspect_response_body"]
            }
            Self::HttpStatus {
                status: 400 | 422, ..
            } => vec!["inspect_operation", "fix_request", "retry"],
            Self::HttpStatus { status: 404, .. } => vec!["inspect_manifest", "check_ids"],
            Self::HttpStatus { status: 429, .. } => vec!["retry_later", "reduce_request_rate"],
            Self::HttpStatus {
                method,
                status: status @ 500..=599,
                ..
            } if mutation_outcome_ambiguous(method, *status) => {
                vec!["reconcile_mutation", "contact_platform_with_request_id"]
            }
            Self::HttpStatus {
                status: 500..=599, ..
            } => {
                vec![
                    "retry_later",
                    "export_project_incidents_with_errors",
                    "contact_platform_with_request_id",
                ]
            }
            Self::HttpStatus { .. } => vec!["inspect_response_body", "retry"],
            Self::Wallet(_) => vec!["fix_wallet", "retry"],
            Self::Onchain(_) => vec!["check_chain_config", "retry"],
            Self::BroadcastCancelled => vec!["rerun_with_yes"],
            Self::ConfirmAfterTx { .. } => vec!["reconcile_mutation", "retry"],
            Self::UnexpectedResponse { .. } => vec!["inspect_response_body"],
        }
    }

    pub fn json_envelope(&self) -> Value {
        let mut error = Map::new();
        error.insert("code".to_string(), json!(self.code()));
        error.insert("message".to_string(), json!(self.to_string()));
        error.insert("recoverable".to_string(), json!(self.recoverable()));

        if let Self::HttpStatus {
            method,
            path,
            status,
            request_id,
            body,
        } = self
        {
            let outcome_ambiguous = mutation_outcome_ambiguous(method, *status);
            if let Some(request_id) = request_id {
                error.insert("request_id".to_string(), json!(request_id));
            }
            error.insert(
                "http".to_string(),
                json!({
                    "method": method,
                    "path": path,
                    "status": status,
                    "request_id": request_id,
                    "body": body.as_ref(),
                }),
            );
            error.insert(
                "mutation".to_string(),
                json!({
                    "side_effecting": method_side_effecting(method),
                    "outcome_ambiguous": outcome_ambiguous,
                }),
            );
        }

        // A landed transaction whose follow-up API confirmation failed needs
        // both chain and API provenance: the structured tx hash plus the
        // nested source's full envelope (HTTP status/path/body, request id).
        if let Self::ConfirmAfterTx { tx_hash, source } = self {
            error.insert("tx_hash".to_string(), json!(tx_hash));
            if let Self::HttpStatus {
                request_id: Some(request_id),
                ..
            } = source.as_ref()
            {
                error.insert("request_id".to_string(), json!(request_id));
            }
            error.insert(
                "mutation".to_string(),
                json!({
                    "side_effecting": true,
                    "onchain_landed": true,
                    "platform_confirmed": false,
                }),
            );
            error.insert("source".to_string(), source.json_envelope());
        }

        let mut envelope = json!({
            "status": "error",
            "error": error,
            "suggested_next_actions": self.suggested_next_actions(),
            "next_actions": self.next_actions(),
        });

        if let Self::HttpStatus {
            method,
            path,
            status,
            request_id,
            ..
        } = self
            && let Some(object) = envelope.as_object_mut()
        {
            object.insert("http_status".to_string(), json!(status));
            object.insert("method".to_string(), json!(method));
            object.insert("path".to_string(), json!(path));
            object.insert("request_id".to_string(), json!(request_id));
            object.insert(
                "outcome_ambiguous".to_string(),
                json!(mutation_outcome_ambiguous(method, *status)),
            );
        }

        if let Self::ConfirmAfterTx { tx_hash, .. } = self
            && let Some(object) = envelope.as_object_mut()
        {
            object.insert("tx_hash".to_string(), json!(tx_hash));
        }

        with_envelope_metadata(envelope)
    }
}

pub(in crate::api) fn method_side_effecting(method: &str) -> bool {
    !method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD")
}

fn mutation_outcome_ambiguous(method: &str, status: u16) -> bool {
    method_side_effecting(method) && status >= 500
}
