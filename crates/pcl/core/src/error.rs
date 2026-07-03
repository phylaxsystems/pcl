#[cfg(feature = "credible")]
use crate::verify::VerificationSummary;
use crate::{
    abi::ConstructorAbiError,
    credible_config::CredibleConfigError,
};
use chrono::{
    DateTime,
    Utc,
};
use dapp_api_client::generated::client::{
    Error as GeneratedApiError,
    types::ApiError as DappApiError,
};
use pcl_phoundry::error::PhoundryError;
use thiserror::Error;

/// Errors that can occur during declarative apply.
#[derive(Error, Debug)]
pub enum ApplyError {
    #[error("Run `pcl auth login` first")]
    NoAuthToken,

    #[error(
        "Stored auth token expired at {0}. Run `pcl auth refresh --json` or `pcl auth login` again."
    )]
    ExpiredAuthToken(DateTime<Utc>),

    #[error("Failed to refresh stored auth before applying release changes: {0}")]
    AuthRefresh(#[source] AuthError),

    #[error(transparent)]
    PlatformMismatch(AuthError),

    #[error("{message}: {source}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse credible.toml: {0}")]
    Toml(#[source] toml::de::Error),

    #[error("Invalid credible.toml: {0}")]
    InvalidConfig(String),

    #[error("Project selection failed: {0}")]
    ProjectSelectionFailed(#[source] inquire::InquireError),

    #[error("No projects found for the authenticated user")]
    NoProjectsFound,

    #[error("Build failed: {0}")]
    BuildFailed(#[source] Box<PhoundryError>),

    #[error("API request to {endpoint} failed{}: {body}", status.map_or(String::new(), |s| format!(" with status {s}")))]
    Api {
        endpoint: String,
        status: Option<u16>,
        body: String,
    },

    #[cfg(feature = "credible")]
    #[error("Assertions failed verification")]
    AssertionsFailed(Box<VerificationSummary>),

    #[error("Apply cancelled")]
    ApplyCancelled,

    #[error("JSON mode with pending changes requires `--yes`")]
    JsonConfirmationRequiresYes,

    #[error(transparent)]
    ConstructorAbi(#[from] ConstructorAbiError),

    #[error("Failed to encode JSON output: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Failed to write structured output: {0}")]
    Output(#[from] crate::output::OutputError),
}

impl From<CredibleConfigError> for ApplyError {
    fn from(e: CredibleConfigError) -> Self {
        Self::InvalidConfig(e.to_string())
    }
}

/// Errors that can occur during assertion verification.
#[cfg(feature = "credible")]
#[derive(Error, Debug)]
pub enum VerifyError {
    #[error(transparent)]
    Config(#[from] CredibleConfigError),

    #[error("{message}: {source}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Build failed: {0}")]
    BuildFailed(#[source] Box<PhoundryError>),

    #[error("Invalid deployment bytecode hex: {0}")]
    BytecodeHex(String),

    #[error(transparent)]
    ConstructorAbi(#[from] ConstructorAbiError),

    #[error("Failed to encode JSON output: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Assertions failed verification")]
    AssertionsFailed(Box<VerificationSummary>),

    #[error("Failed to write structured output: {0}")]
    Output(#[from] crate::output::OutputError),
}

/// Errors that can occur during the end-to-end `pcl deploy` orchestration.
#[derive(Error, Debug)]
pub enum DeployError {
    #[error(transparent)]
    Apply(#[from] ApplyError),

    #[error(transparent)]
    Api(#[from] crate::api::ApiCommandError),

    #[error(transparent)]
    Wallet(#[from] crate::wallet::WalletError),

    #[error(
        "Project {project_id} protocol manager is {current}, but the wallet is {wallet}. The deploy transaction must come from the manager wallet. Transfer it with `pcl protocol-manager --project {project_id} --transfer-calldata --new-manager {wallet} --broadcast`, or pass the manager's key."
    )]
    ManagerMismatch {
        project_id: uuid::Uuid,
        current: String,
        wallet: String,
    },

    #[error(
        "credible.toml has no project_id and no project can be created without a project name (set project_name in credible.toml or pass --project-name) and --chain-id"
    )]
    MissingProjectInfo,

    #[error("Unexpected {endpoint} response: {reason}")]
    UnexpectedResponse {
        endpoint: &'static str,
        reason: String,
    },

    #[error(
        "Release checks did not reach `all_passed` within {timeout_secs}s (last deploy-blocking status: {status}). Re-run `pcl deploy` to resume once checks finish."
    )]
    ChecksTimeout { timeout_secs: u64, status: String },

    #[error(
        "Release deploy-blocking checks failed (status: {status}). Inspect with `pcl releases show {project_id} {release_id}` and retry with `pcl releases retry-check`."
    )]
    ChecksFailed {
        project_id: uuid::Uuid,
        release_id: String,
        status: String,
    },

    #[error("Failed to write project_id back to {path}: {reason}")]
    TomlWriteBack { path: String, reason: String },

    #[error(
        "Machine output requires `--yes` (pcl deploy mutates the project and broadcasts transactions)"
    )]
    MachineYesRequired,

    #[error("Deploy cancelled")]
    Cancelled,

    #[error("Failed to encode JSON output: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Failed to write structured output: {0}")]
    Output(#[from] crate::output::OutputError),
}

/// Errors that can occur during configuration operations
#[derive(Error, Debug)]
pub enum ConfigError {
    /// Error when reading the config file from ~/.config/pcl/config.toml fails
    #[error("Failed to read config file: {0}")]
    ReadError(std::io::Error),

    /// Error when writing to the config file at ~/.config/pcl/config.toml fails
    #[error("Failed to write config file: {0}")]
    WriteError(std::io::Error),

    /// Error when deserializing the config file fails
    #[error("Failed to parse config file: {0}")]
    ParseError(#[source] toml::de::Error),

    /// Error when serializing the config file fails
    #[error("Failed to serialize config file: {0}")]
    SerializeError(#[source] toml::ser::Error),

    /// Error when serializing structured CLI output fails
    #[error("Failed to serialize JSON output: {0}")]
    JsonError(#[source] serde_json::Error),

    /// Error when attempting an operation that requires authentication
    /// but no authentication token is present in the config
    #[error("No Authentication Token Found")]
    NotAuthenticated,

    /// Error when a config value supplied on the command line is invalid
    #[error("Invalid config value: {0}")]
    InvalidValue(String),
}

/// Errors that can occur during authentication operations
#[derive(Error, Debug)]
pub enum AuthError {
    /// Error when the auth code request fails
    #[error(
        "Authentication request failed. Please check your connection and try again.\nError: {0}"
    )]
    AuthRequestFailed(String),

    /// Error when the auth status check fails due to network/transport issues
    #[error(
        "Authentication status request failed. Please check your connection and try again.\nError: {0}"
    )]
    StatusRequestFailed(String),

    /// Error when the auth session is no longer valid
    #[error("Invalid session: {0}. Please run `pcl auth login` again.")]
    InvalidSession(String),

    /// Error when the locally stored access token has expired
    #[error(
        "Stored auth token for {user} expired at {expires_at}. Run `pcl auth refresh`; if refresh fails, run `pcl auth login --force`."
    )]
    StoredTokenExpired {
        user: String,
        expires_at: DateTime<Utc>,
        platform_url: String,
    },

    /// Error when there are no stored credentials that can be refreshed.
    #[error("No refreshable CLI session found. Run `pcl auth login`.")]
    NoRefreshableSession,

    /// Error when the stored refresh token is missing or empty.
    #[error("Stored CLI session is missing a refresh token. Run `pcl auth login --force`.")]
    MissingRefreshToken,

    /// Error when the platform rejected the refresh token.
    #[error("Stored CLI session expired or was already rotated. Run `pcl auth login --force`.")]
    RefreshRejected {
        status: u16,
        code: Option<String>,
        request_id: Option<String>,
        message: Option<String>,
    },

    /// Error when the platform does not expose the CLI refresh endpoint.
    #[error("Token refresh endpoint was not found on the platform. Run `pcl auth login --force`.")]
    RefreshEndpointNotFound {
        request_id: Option<String>,
        message: Option<String>,
    },

    /// Error when the platform rate-limits token refresh.
    #[error("Token refresh was rate limited. Retry after {retry_after_seconds:?} seconds.")]
    RefreshRateLimited {
        retry_after_seconds: Option<u64>,
        request_id: Option<String>,
        message: Option<String>,
    },

    /// Error when refresh failed due to a server-side issue.
    #[error("Token refresh failed with server status {status}. Try again later.")]
    RefreshServerError {
        status: u16,
        request_id: Option<String>,
        message: Option<String>,
    },

    /// Error when refresh failed due to local transport or response issues.
    #[error(
        "Token refresh request failed. Please check your connection and try again.\nError: {0}"
    )]
    RefreshRequestFailed(String),

    /// Error when another local process is holding the refresh lock too long.
    #[error("Timed out waiting for another PCL process to finish refreshing auth.")]
    RefreshLockTimeout,

    /// Error when a command would send stored credentials to a platform
    /// that did not issue them.
    #[error(
        "Stored credentials belong to {credential_platform}, but this command targets {requested}. Run `pcl auth login --auth-url {requested}` to log into that platform, or pass --allow-unauthenticated where supported."
    )]
    PlatformMismatch {
        credential_platform: String,
        requested: String,
    },

    /// Error when the session has expired server-side
    #[error("Session expired. Please run `pcl auth login` to start a new session.")]
    SessionExpired,

    /// Error when the session is not found (bad `session_id` or `device_secret`)
    #[error("Session not found. Please run `pcl auth login` to start a new session.")]
    SessionNotFound,

    /// Error when the user is not found in the platform
    #[error("User not found. Please ensure your account exists on the Credible Layer Platform.")]
    UserNotFound,

    /// Error when the server encounters an internal error
    #[error("Server error. Please try again later.\nDetails: {0}")]
    ServerError(String),

    /// Error when authentication times out
    #[error(
        "Authentication timed out after {0} attempts. Please try again and approve the wallet connection promptly."
    )]
    Timeout(u32),

    /// Error when authentication verification fails
    #[error("Authentication failed: {0}")]
    InvalidAuthData(String),

    /// Error when config operations fail during auth
    #[error("Config error: {0}")]
    ConfigError(#[source] ConfigError),
}

/// Structured error codes returned by the dapp CLI auth status endpoint.
///
/// Only codes that drive distinct polling behavior are listed here.
#[derive(Debug, PartialEq, Eq)]
enum DappErrorCode {
    SessionExpired,
    SessionNotFound,
    UserNotFound,
    InternalError,
    /// Catch-all for codes that don't need distinct handling.
    Other,
}

impl DappErrorCode {
    fn from_api_code(code: Option<&str>) -> Self {
        match code {
            Some("SESSION_EXPIRED") => Self::SessionExpired,
            Some("SESSION_NOT_FOUND") => Self::SessionNotFound,
            Some("USER_NOT_FOUND") => Self::UserNotFound,
            Some("INTERNAL_ERROR") => Self::InternalError,
            Some(_) | None => Self::Other,
        }
    }
}

impl From<GeneratedApiError<DappApiError>> for AuthError {
    /// Convert a progenitor API error into a typed `AuthError`.
    fn from(err: GeneratedApiError<DappApiError>) -> Self {
        match err {
            GeneratedApiError::ErrorResponse(response) => {
                let status = response.status();
                let body = response.into_inner();
                match DappErrorCode::from_api_code(body.code.as_deref()) {
                    DappErrorCode::SessionExpired => Self::SessionExpired,
                    DappErrorCode::SessionNotFound => Self::SessionNotFound,
                    DappErrorCode::UserNotFound => Self::UserNotFound,
                    DappErrorCode::InternalError => Self::ServerError(body.error),
                    DappErrorCode::Other if status.is_server_error() => {
                        Self::ServerError(body.error)
                    }
                    DappErrorCode::Other => Self::InvalidSession(body.error),
                }
            }
            GeneratedApiError::InvalidResponsePayload(bytes, _) => {
                if let Ok(body) = serde_json::from_slice::<DappApiError>(&bytes) {
                    return match DappErrorCode::from_api_code(body.code.as_deref()) {
                        DappErrorCode::SessionExpired => Self::SessionExpired,
                        DappErrorCode::SessionNotFound => Self::SessionNotFound,
                        DappErrorCode::UserNotFound => Self::UserNotFound,
                        DappErrorCode::InternalError => Self::ServerError(body.error),
                        DappErrorCode::Other => Self::InvalidSession(body.error),
                    };
                }
                Self::StatusRequestFailed(format!(
                    "Invalid response payload: {}",
                    String::from_utf8_lossy(&bytes)
                ))
            }
            GeneratedApiError::UnexpectedResponse(response) => {
                let status = response.status();
                if status.is_server_error() {
                    Self::ServerError(format!("HTTP {status}"))
                } else {
                    Self::InvalidSession(format!("HTTP {status}"))
                }
            }
            GeneratedApiError::CommunicationError(error)
            | GeneratedApiError::InvalidUpgrade(error)
            | GeneratedApiError::ResponseBodyError(error) => {
                Self::StatusRequestFailed(error.to_string())
            }
            GeneratedApiError::InvalidRequest(message) | GeneratedApiError::Custom(message) => {
                Self::StatusRequestFailed(message)
            }
        }
    }
}
