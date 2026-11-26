use clap::{
    Parser,
    ValueHint,
};
use foundry_cli::{
    opts::{
        BuildOpts,
        ProjectPathOpts,
    },
    utils::LoadConfig,
};
use foundry_compilers::{
    artifacts::{
        ConfigurableContractArtifact,
        SolcInput,
    },
    flatten::{
        Flattener,
        FlattenerError,
    },
    info::ContractInfo,
    solc::SolcLanguage,
};

use alloy_json_abi::JsonAbi;
use serde::Deserialize;
use serde_json::Value;

use foundry_compilers::artifacts::BytecodeHash;
use foundry_config::find_project_root;
use std::{
    collections::HashMap,
    path::{
        Path,
        PathBuf,
    },
};

use crate::{
    DEFAULT_ASSERTION_CONTRACTS_DIR,
    error::PhoundryError,
};

/// Output from building and flattening a Solidity contract.
/// Contains the compiler version used and the flattened source code.
#[derive(Debug, Default)]
pub struct BuildAndFlatOutput {
    /// Full compiler version (e.g. "v0.8.28+commit.7893614a")
    pub compiler_version: String,
    /// Flattened source code of the contract
    pub flattened_source: String,
    /// Abi of the contract
    pub abi: JsonAbi,
    /// Deployment bytecode of the contract
    pub bytecode: String,
    /// Whether the optimizer was enabled during compilation
    pub optimizer_enabled: bool,
    /// Number of optimizer runs used during compilation
    pub optimizer_runs: u64,
    /// Target EVM version
    pub evm_version: String,
    /// Metadata bytecode hash strategy
    pub metadata_bytecode_hash: BytecodeHash,
    /// Solidity remappings used during compilation
    pub remappings: Vec<String>,
    /// Linked libraries keyed by fully-qualified library name
    pub libraries: HashMap<String, String>,
    /// Source path used as the compilation target
    pub compilation_target: String,
}

impl BuildAndFlatOutput {
    /// Creates a new `BuildAndFlatOutput` instance.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        compiler_version: String,
        flattened_source: String,
        abi: JsonAbi,
        bytecode: String,
        optimizer_enabled: bool,
        optimizer_runs: u64,
        evm_version: String,
        metadata_bytecode_hash: BytecodeHash,
        remappings: Vec<String>,
        libraries: HashMap<String, String>,
        compilation_target: String,
    ) -> Self {
        Self {
            compiler_version,
            flattened_source,
            abi,
            bytecode,
            optimizer_enabled,
            optimizer_runs,
            evm_version,
            metadata_bytecode_hash,
            remappings,
            libraries,
            compilation_target,
        }
    }

    /// Validates the build output.
    ///
    /// Returns an error if any field has an unexpected format
    pub fn validate(&self) -> Result<(), Box<PhoundryError>> {
        if !self.compiler_version.contains("+commit.") {
            return Err(Box::new(PhoundryError::InvalidForgeOutput(
                "Invalid solc version format: expected 'vX.Y.Z+commit.hash'",
            )));
        }
        Ok(())
    }

    /// Returns the short compiler version (e.g. "0.8.28") by stripping
    /// the "v" prefix and "+commit.xxx" suffix from the full version.
    pub fn compiler_version_short(&self) -> Result<&str, Box<PhoundryError>> {
        let v = self
            .compiler_version
            .strip_prefix('v')
            .unwrap_or(&self.compiler_version);
        v.split_once('+').map(|(short, _)| short).ok_or_else(|| {
            Box::new(PhoundryError::InvalidForgeOutput(
                "Invalid solc version format",
            ))
        })
    }
}

/// Command-line arguments for building and flattening Solidity contracts.
/// This is used to prepare contracts for submission to the assertion DA layer.
#[derive(Debug, Default, Parser)]
#[clap(about = "Build and flatten contracts using Phorge")]
pub struct BuildAndFlattenArgs {
    /// Root directory of the project
    #[clap(
        long,
        value_hint = ValueHint::DirPath,
        help = "Root directory of the project"
    )]
    pub root: Option<PathBuf>,

    /// Name of the assertion contract to build and flatten
    #[clap(help = "Name of the assertion contract to build and flatten")]
    pub assertion_contract: String,

    /// Directory containing assertion contracts, relative to root unless absolute
    #[clap(
        long,
        value_hint = ValueHint::DirPath,
        default_value = DEFAULT_ASSERTION_CONTRACTS_DIR,
        help = "Directory containing assertion contracts"
    )]
    pub contracts: PathBuf,
}

impl BuildAndFlattenArgs {
    /// Builds and flattens the specified contract.
    ///
    /// # Returns
    ///
    /// - `Ok(BuildAndFlatOutput)` containing the compiler version and flattened source
    /// - `Err(PhoundryError)` if any step in the process fails
    pub fn run(&self) -> Result<BuildAndFlatOutput, Box<PhoundryError>> {
        foundry_cli::utils::load_dotenv();

        let build = crate::compile::compile(self.build_opts())?;
        let info = ContractInfo::new(&self.assertion_contract);

        // Find the contract artifact
        let artifact = build
            .find_contract(info)
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;
        self.output(artifact)
    }

    /// Captures the exact compiler input for a release, restricted to its assertion and imports.
    pub fn run_for_release(
        &self,
        source: &Path,
    ) -> Result<(BuildAndFlatOutput, String), Box<PhoundryError>> {
        foundry_cli::utils::load_dotenv();
        let source =
            foundry_compilers::utils::canonicalize(source).map_err(std::io::Error::from)?;
        let build = crate::compile::compile_target(self.build_opts(), Some(&source))?;
        let (id, artifact) = build
            .artifact_ids()
            .find(|(id, _)| id.source == source && id.name == self.assertion_contract)
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;
        let input = build
            .output()
            .build_infos
            .iter()
            .find(|info| info.id == id.build_id)
            .and_then(|info| info.build_info.get("input"))
            .ok_or(PhoundryError::InvalidForgeOutput(
                "Missing release compiler input",
            ))?;
        // Build info also contains Foundry's local CLI options, which solc does not accept.
        let input = SolcInput::deserialize(input)
            .map_err(|_| PhoundryError::InvalidForgeOutput("Invalid Solidity compiler input"))?;
        let compiler_input = serde_json::to_string(&input)
            .map_err(|_| PhoundryError::InvalidForgeOutput("Failed to serialize compiler input"))?;
        Ok((self.output(artifact)?, compiler_input))
    }

    fn output(
        &self,
        artifact: &ConfigurableContractArtifact,
    ) -> Result<BuildAndFlatOutput, Box<PhoundryError>> {
        let abi = artifact.abi.clone().ok_or_else(|| {
            PhoundryError::InvalidForgeOutput("Failed to parse ABI from artifact")
        })?;

        // Extract metadata and compiler version
        let metadata = artifact
            .metadata
            .clone()
            .ok_or_else(|| PhoundryError::InvalidForgeOutput("Missing contract metadata"))?;
        let settings = serde_json::to_value(&metadata.settings).map_err(|_| {
            PhoundryError::InvalidForgeOutput("Failed to serialize compiler settings")
        })?;
        let bytecode = extract_bytecode(&artifact.bytecode)
            .ok_or_else(|| PhoundryError::InvalidForgeOutput("Missing contract bytecode"))?;

        let solc_version = format!("v{}", metadata.compiler.version);

        // Find the source path for the contract
        let rel_source_path = metadata
            .settings
            .compilation_target
            .iter()
            .find_map(|(path, name)| {
                if name == &self.assertion_contract {
                    Some(path)
                } else {
                    None
                }
            })
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;

        // Determine the full path to the contract
        let path = match &self.root {
            Some(root) => root.join(rel_source_path),
            None => {
                find_project_root(None)
                    .map_err(|_| PhoundryError::DirectoryNotFound(PathBuf::from(".")))?
                    .join(rel_source_path)
            }
        };

        // Flatten the contract
        let flattened = self.flatten(&path)?;
        let output = BuildAndFlatOutput::new(
            solc_version,
            flattened,
            abi,
            bytecode,
            settings
                .pointer("/optimizer/enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            settings
                .pointer("/optimizer/runs")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            settings
                .get("evmVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            settings
                .pointer("/metadata/bytecodeHash")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<BytecodeHash>().ok())
                .unwrap_or_default(),
            settings
                .get("remappings")
                .and_then(Value::as_array)
                .map(|remappings| {
                    remappings
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            flatten_libraries(&settings),
            rel_source_path.clone(),
        );
        output.validate()?;
        Ok(output)
    }

    fn build_opts(&self) -> BuildOpts {
        BuildOpts {
            project_paths: ProjectPathOpts {
                root: self.root.clone(),
                contracts: Some(self.contracts.clone()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Flattens the contract source code.
    fn flatten(&self, path: &PathBuf) -> Result<String, Box<PhoundryError>> {
        let build = BuildOpts {
            project_paths: ProjectPathOpts {
                root: self.root.clone(),
                ..Default::default()
            },
            ..Default::default()
        };

        let config = build.load_config()?;
        let project = config
            .ephemeral_project()
            .map_err(|e| Box::new(PhoundryError::SolcError(e)))?;

        let can_path = std::fs::canonicalize(path)
            .map_err(|e| Box::new(PhoundryError::CanonicalizePathError(e)))?;

        // Try the new flattener first
        let flattener = Flattener::new(project.clone(), &can_path);
        let flattened_source = match flattener {
            Ok(flattener) => Ok(flattener.flatten()),
            Err(FlattenerError::Compilation(_)) => {
                // Fallback to the old flattening implementation for invalid syntax
                project
                    .paths
                    .with_language::<SolcLanguage>()
                    .flatten(path)
                    .map_err(|e| Box::new(PhoundryError::SolcError(e)))
            }
            Err(FlattenerError::Other(err)) => Err(Box::new(PhoundryError::SolcError(err))),
        }?;

        Ok(flattened_source)
    }
}

fn extract_bytecode<T: serde::Serialize>(bytecode: &T) -> Option<String> {
    let value = serde_json::to_value(bytecode).ok()?;
    value
        .pointer("/object")
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(ToString::to_string)
}

fn flatten_libraries(settings: &Value) -> HashMap<String, String> {
    settings
        .get("libraries")
        .and_then(Value::as_object)
        .map(|files| {
            files
                .iter()
                .flat_map(|(file, libraries)| {
                    libraries
                        .as_object()
                        .into_iter()
                        .flat_map(move |libraries| {
                            libraries.iter().filter_map(move |(name, address)| {
                                address
                                    .as_str()
                                    .map(|address| (format!("{file}:{name}"), address.to_string()))
                            })
                        })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Helper function to create a temporary Solidity project
    fn setup_test_project() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let project_root = temp_dir.path().join("test_project");
        fs::create_dir_all(&project_root).unwrap();

        // Create a simple test contract
        let contract_path = project_root.join("src").join("TestContract.sol");
        fs::create_dir_all(contract_path.parent().unwrap()).unwrap();
        fs::write(
            &contract_path,
            r"// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

contract TestContract {
    function test() public pure returns (bool) {
        return true;
    }
}",
        )
        .unwrap();

        (temp_dir, project_root)
    }

    #[test]
    fn test_build_and_flatten_args_new() {
        let args = BuildAndFlattenArgs {
            root: None,
            assertion_contract: "TestContract".to_string(),
            contracts: PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR),
        };

        assert_eq!(args.assertion_contract, "TestContract");
        assert!(args.root.is_none());
        assert_eq!(
            args.contracts,
            PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR)
        );
    }

    #[test]
    fn test_build_and_flat_output_new() {
        let output = BuildAndFlatOutput::new(
            "0.8.0".to_string(),
            "contract Test { }".to_string(),
            JsonAbi::default(),
            "0x6000".to_string(),
            true,
            200,
            "prague".to_string(),
            BytecodeHash::Ipfs,
            vec!["@openzeppelin/=lib/openzeppelin/".to_string()],
            HashMap::new(),
            "assertions/src/TestContract.sol".to_string(),
        );

        assert_eq!(output.compiler_version, "0.8.0");
        assert_eq!(output.flattened_source, "contract Test { }");
        assert_eq!(output.bytecode, "0x6000");
    }

    #[test]
    fn test_compiler_version_short_strips_prefix_and_commit() {
        let output = BuildAndFlatOutput::new(
            "v0.8.28+commit.7893614a".to_string(),
            "contract Test { }".to_string(),
            JsonAbi::default(),
            "0x6000".to_string(),
            true,
            200,
            "prague".to_string(),
            BytecodeHash::Ipfs,
            vec![],
            HashMap::new(),
            "assertions/src/TestContract.sol".to_string(),
        );

        assert_eq!(output.compiler_version_short().unwrap(), "0.8.28");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_input_rebuilds_the_selected_artifact_with_only_its_imports() {
        let (_temp_dir, project_root) = setup_test_project();
        fs::write(
            project_root.join("foundry.toml"),
            r#"
[profile.default]
solc = "0.8.28"
via_ir = true
optimizer = true
optimizer_runs = 731
bytecode_hash = "ipfs"
[profile.default.optimizer_details]
cse = false
"#,
        )
        .unwrap();
        let target = project_root.join("src/Vault.sol");
        fs::write(
            &target,
            r#"// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;
import {TestContract} from "./TestContract.sol";
contract Vault is TestContract {}"#,
        )
        .unwrap();
        fs::write(
            project_root.join("src/Other.sol"),
            "pragma solidity 0.8.28; contract Vault { /* unrelated */ }",
        )
        .unwrap();
        let args = BuildAndFlattenArgs {
            root: Some(project_root),
            assertion_contract: "Vault".to_string(),
            contracts: PathBuf::from("src"),
        };
        let (artifact, json) = args.run_for_release(&target).unwrap();
        assert_eq!(artifact.compilation_target, "src/Vault.sol");
        let input: Value = serde_json::from_str(&json).unwrap();
        let sources = input["sources"].as_object().unwrap();
        assert_eq!(sources.len(), 2);
        assert!(sources.contains_key("src/TestContract.sol"));
        assert!(sources.contains_key(&artifact.compilation_target));
        assert_eq!(input["settings"]["viaIR"], true);
        assert_eq!(input["settings"]["optimizer"]["details"]["cse"], false);
        let version = artifact.compiler_version_short().unwrap().parse().unwrap();
        let solc = foundry_compilers::solc::Solc::find_or_install(&version).unwrap();
        let rebuilt: Value = solc.compile_as(&input).unwrap();
        let bytes = rebuilt["contracts"][&artifact.compilation_target]["Vault"]["evm"]["bytecode"]
            ["object"]
            .as_str()
            .unwrap();
        assert_eq!(artifact.bytecode, format!("0x{bytes}"));
    }
}
