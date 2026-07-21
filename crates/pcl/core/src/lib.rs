#![allow(clippy::must_use_candidate)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::struct_field_names)]
#![allow(clippy::unreadable_literal)]
#![recursion_limit = "256"]

pub mod abi;
pub mod api;
pub mod apply;
pub mod auth;
pub mod client;
pub mod config;
pub mod credible_config;
pub mod deploy;
pub mod diff;
pub mod download;
pub mod error;
pub mod onchain;
pub mod output;
pub mod request_log;
pub mod surface;
#[cfg(feature = "credible")]
pub mod verify;
pub mod wallet;

/// Default platform url. URL suffixes added on demand.
pub const DEFAULT_PLATFORM_URL: &str = "https://app.phylax.systems";
