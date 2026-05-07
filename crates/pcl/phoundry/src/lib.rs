#![allow(clippy::missing_errors_doc)]
#![allow(clippy::must_use_candidate)]
pub mod build;
pub mod build_and_flatten;
pub mod compile;
pub mod error;
pub mod phorge_test;

pub const DEFAULT_ASSERTION_CONTRACTS_DIR: &str = "assertions/src";
