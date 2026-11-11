//! Shared types and parsing for `credible.toml` deployment configuration files.

use pcl_phoundry::DEFAULT_ASSERTION_CONTRACTS_DIR;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{
        BTreeMap,
        HashMap,
    },
    path::{
        Path,
        PathBuf,
    },
};
use thiserror::Error;
use uuid::Uuid;

/// Errors from reading or validating `credible.toml`.
#[derive(Error, Debug)]
pub enum CredibleConfigError {
    #[error("{message}: {source}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Failed to parse credible.toml: {0}")]
    Toml(#[source] toml::de::Error),

    #[error("Invalid credible.toml: {0}")]
    Invalid(String),
}

/// Root structure of a `credible.toml` file.
#[derive(Debug, Deserialize)]
pub struct CredibleToml {
    pub environment: String,
    #[serde(default)]
    pub project_id: Option<Uuid>,
    #[serde(default)]
    pub project_name: Option<String>,
    pub contracts: BTreeMap<String, CredibleContract>,
}

impl CredibleToml {
    /// Reads and validates a `credible.toml` file at the given path.
    pub fn from_path(path: &Path) -> Result<Self, CredibleConfigError> {
        let contents = std::fs::read_to_string(path).map_err(|e| {
            CredibleConfigError::Io {
                message: format!("credible.toml not found at {}", path.display()),
                source: e,
            }
        })?;
        let credible: Self = toml::from_str(&contents).map_err(CredibleConfigError::Toml)?;
        credible.validate()?;
        Ok(credible)
    }

    /// Runs all config validations.
    fn validate(&self) -> Result<(), CredibleConfigError> {
        self.validate_unique_addresses()?;
        for assertion in self.contracts.values().flat_map(|c| &c.assertions) {
            AssertionIdentity::from_config(&assertion.file)?;
        }
        Ok(())
    }

    /// Ensures no two contracts share the same address.
    pub(crate) fn validate_unique_addresses(&self) -> Result<(), CredibleConfigError> {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for (key, contract) in &self.contracts {
            if let Some(existing_key) = seen.get(contract.address.as_str()) {
                return Err(CredibleConfigError::Invalid(format!(
                    "duplicate contract address {}: used by both `{}` and `{}`",
                    contract.address, existing_key, key
                )));
            }
            seen.insert(&contract.address, key);
        }
        Ok(())
    }
}

/// A contract entry within `credible.toml`.
#[derive(Debug, Deserialize)]
pub struct CredibleContract {
    pub address: String,
    pub name: String,
    pub assertions: Vec<CredibleAssertion>,
}

/// An assertion entry within a contract.
#[derive(Debug, Deserialize)]
pub struct CredibleAssertion {
    pub file: String,
    #[serde(default, deserialize_with = "deserialize_args")]
    pub args: Vec<String>,
}

fn deserialize_args<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Array(values) => Ok(values.into_iter().map(value_to_string).collect()),
        Value::Null => Ok(vec![]),
        other => Ok(vec![value_to_string(other)]),
    }
}

fn value_to_string(value: Value) -> String {
    match value {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

/// An assertion's source file and contract name, parsed once from a
/// `credible.toml` `file` entry or a `pcl verify` argument so that artifact
/// selection and the release payload use the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionIdentity {
    /// Source path relative to the project root; `None` for a bare name.
    pub source: Option<String>,
    pub contract_name: String,
}

impl AssertionIdentity {
    /// Parses `path:Contract`, `path/Contract.a.sol` (contract inferred from the
    /// file name) or a bare `Contract`. The error is a bare message so each
    /// caller can say where the input came from.
    pub fn parse(file: &str) -> Result<Self, String> {
        let file = file.trim().replace('\\', "/");
        let (source, contract_name) = match file.split_once(':') {
            Some((source, name)) => (Some(source.trim().to_string()), name.trim().to_string()),
            None if file.contains('/') || has_sol_extension(&file) => {
                let name = infer_contract_name(&file)?;
                (Some(file.clone()), name)
            }
            None => (None, file.clone()),
        };
        if source.as_deref().is_some_and(str::is_empty) {
            return Err(format!("assertion `{file}` has an empty source path"));
        }
        if !is_contract_name(&contract_name) {
            return Err(format!(
                "assertion `{file}` must be `path/to/File.a.sol:Contract` with one contract name"
            ));
        }
        Ok(Self {
            source,
            contract_name,
        })
    }

    /// Parses a `credible.toml` entry, which must name its source file: the
    /// deploy circuit-breaker scan reads that file, so a bare name would skip it.
    pub fn from_config(file: &str) -> Result<Self, CredibleConfigError> {
        let identity = Self::parse(file).map_err(CredibleConfigError::Invalid)?;
        if identity.source.is_none() {
            return Err(CredibleConfigError::Invalid(format!(
                "assertion `{}` must name its source file, such as assertions/src/{0}.a.sol:{0}",
                identity.contract_name
            )));
        }
        Ok(identity)
    }

    /// Directory Foundry compiles for this assertion: the source's parent, or
    /// the default assertions directory for a bare name or dir-less path.
    pub fn contracts_dir(&self) -> PathBuf {
        Path::new(self.source.as_deref().unwrap_or_default())
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(
                || PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR),
                Path::to_path_buf,
            )
    }

    /// The `path:Contract` (or bare name) key that selects the Foundry artifact.
    pub fn lookup_key(&self) -> String {
        match &self.source {
            Some(source) => format!("{source}:{}", self.contract_name),
            None => self.contract_name.clone(),
        }
    }
}

fn infer_contract_name(file: &str) -> Result<String, String> {
    let file_name = Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid assertion file path: {file}"))?;

    for suffix in [".a.sol", ".sol"] {
        if let Some(contract_name) = file_name.strip_suffix(suffix) {
            return Ok(contract_name.to_string());
        }
    }

    Err(format!(
        "Could not infer assertion contract from file {file}"
    ))
}

fn has_sol_extension(file: &str) -> bool {
    Path::new(file)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sol"))
}

fn is_contract_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const VALID_CREDIBLE_TOML: &str = r#"
        environment = "production"
        project_name = "mock-protocol"
        [contracts.my_contract]
        address = "0x1234567890abcdef1234567890abcdef12345678"
        name = "MockProtocol"
        [[contracts.my_contract.assertions]]
        file = "src/NoArgsAssertion.a.sol"
    "#;

    #[test]
    fn identity_keeps_the_source_path_and_one_contract_name() {
        let parse = |file: &str| AssertionIdentity::parse(file).map(|id| id.lookup_key());
        for (file, key) in [
            (
                "assertions/src/ZStrong.a.sol:Guard",
                "assertions/src/ZStrong.a.sol:Guard",
            ),
            (
                " assertions/src/ZStrong.a.sol : Guard ",
                "assertions/src/ZStrong.a.sol:Guard",
            ),
            (
                "assertions/src/MockAssertion.a.sol",
                "assertions/src/MockAssertion.a.sol:MockAssertion",
            ),
            (
                "assertions\\src\\Guard.a.sol",
                "assertions/src/Guard.a.sol:Guard",
            ),
            ("Guard.a.sol", "Guard.a.sol:Guard"),
            ("Guard", "Guard"),
        ] {
            assert_eq!(parse(file).unwrap(), key, "{file}");
        }
        for file in [
            "a.sol:Ignored:Guard",
            "a.sol:",
            "a.sol: ",
            ":Guard",
            "",
            "Guard.t",
            "assertions/src/Guard",
        ] {
            assert!(parse(file).is_err(), "{file} should be rejected");
        }
    }

    #[test]
    fn contracts_dir_is_the_source_parent_or_the_default() {
        let dir = |file: &str| AssertionIdentity::parse(file).unwrap().contracts_dir();
        assert_eq!(
            dir("custom/src/Guard.a.sol:Guard"),
            PathBuf::from("custom/src")
        );
        assert_eq!(
            dir("Guard.a.sol:Guard"),
            PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR)
        );
        assert_eq!(dir("Guard"), PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR));
    }

    #[test]
    fn toml_rejects_duplicate_contract_keys() {
        let toml_str = r#"
            environment = "production"
            [contracts.ownable]
            address = "0xD1f444eA1D2d9fA567F8fD73b15199F90e630074"
            name = "Ownable"
            [[contracts.ownable.assertions]]
            file = "src/OwnableAssertion.a.sol"

            [contracts.ownable]
            address = "0xC9734723aAD51626dC9244fed32668ccb280856A"
            name = "Ownable2"
            [[contracts.ownable.assertions]]
            file = "src/OwnableAssertion.a.sol"
        "#;
        let result = toml::from_str::<CredibleToml>(toml_str);
        assert!(result.is_err(), "TOML should reject duplicate keys");
    }

    #[test]
    fn rejects_duplicate_contract_addresses() {
        let toml_str = r#"
            environment = "production"
            [contracts.ownable]
            address = "0xD1f444eA1D2d9fA567F8fD73b15199F90e630074"
            name = "Ownable"
            [[contracts.ownable.assertions]]
            file = "src/OwnableAssertion.a.sol"

            [contracts.ownable2]
            address = "0xD1f444eA1D2d9fA567F8fD73b15199F90e630074"
            name = "Ownable2"
            [[contracts.ownable2.assertions]]
            file = "src/OwnableAssertion.a.sol"
        "#;
        let credible: CredibleToml = toml::from_str(toml_str).unwrap();
        let err = credible.validate_unique_addresses().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("duplicate contract address"),
            "expected duplicate address error, got: {msg}"
        );
        assert!(msg.contains("0xD1f444eA1D2d9fA567F8fD73b15199F90e630074"));
    }

    #[test]
    fn accepts_distinct_contract_addresses() {
        let toml_str = r#"
            environment = "production"
            [contracts.ownable]
            address = "0xD1f444eA1D2d9fA567F8fD73b15199F90e630074"
            name = "Ownable"
            [[contracts.ownable.assertions]]
            file = "src/OwnableAssertion.a.sol"

            [contracts.ownable2]
            address = "0xC9734723aAD51626dC9244fed32668ccb280856A"
            name = "Ownable2"
            [[contracts.ownable2.assertions]]
            file = "src/OwnableAssertion.a.sol"
        "#;
        let credible: CredibleToml = toml::from_str(toml_str).unwrap();
        credible.validate_unique_addresses().unwrap();
    }

    #[test]
    fn reads_credible_toml_from_path() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let assertions_dir = root.join("assertions");
        fs::create_dir_all(&assertions_dir).unwrap();
        fs::write(assertions_dir.join("credible.toml"), VALID_CREDIBLE_TOML).unwrap();

        let credible = CredibleToml::from_path(&root.join("assertions/credible.toml")).unwrap();
        assert_eq!(credible.environment, "production");
        assert_eq!(credible.project_name.as_deref(), Some("mock-protocol"));
        assert_eq!(
            credible.contracts.get("my_contract").unwrap().name,
            "MockProtocol"
        );
    }

    #[test]
    fn project_name_defaults_to_none() {
        let toml_str = r#"
            environment = "staging"
            [contracts.ownable]
            address = "0xD1f444eA1D2d9fA567F8fD73b15199F90e630074"
            name = "Ownable"
            [[contracts.ownable.assertions]]
            file = "src/OwnableAssertion.a.sol"
        "#;
        let credible = toml::from_str::<CredibleToml>(toml_str).unwrap();
        assert_eq!(credible.project_name, None);
    }

    #[test]
    fn from_path_returns_error_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let err =
            CredibleToml::from_path(&tmp.path().join("nonexistent/credible.toml")).unwrap_err();
        assert!(matches!(err, CredibleConfigError::Io { .. }));
    }
}
