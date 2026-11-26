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
    Graph,
    Project,
    ProjectCompileOutput,
    artifacts::ConfigurableContractArtifact,
    flatten::{
        Flattener,
        FlattenerError,
    },
    info::ContractInfo,
    multi::MultiCompilerParser,
    solc::{
        Solc,
        SolcLanguage,
    },
};

use alloy_json_abi::JsonAbi;
use serde_json::Value;

use foundry_compilers::artifacts::BytecodeHash;
use foundry_config::{
    Config,
    SolcReq,
    find_project_root,
};
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
    /// Standard JSON input that reproduces `bytecode`, set by [`BuildAndFlattenArgs::run_for_release`]
    pub compiler_input: Option<String>,
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
            compiler_input: None,
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

    /// Assertion source file, relative to root unless absolute. When set, the artifact is
    /// selected by file and contract name instead of contract name alone.
    #[clap(skip)]
    pub source: Option<PathBuf>,
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
        self.output(self.select(&build)?)
    }

    /// Like [`Self::run`], plus the standard JSON input that reproduces the selected artifact
    /// from only the sources it was compiled from.
    pub fn run_for_release(&self) -> Result<BuildAndFlatOutput, Box<PhoundryError>> {
        foundry_cli::utils::load_dotenv();
        let build = crate::compile::compile(self.build_opts())?;
        let artifact = self.select(&build)?;
        let mut output = self.output(artifact)?;
        output.compiler_input = Some(self.release_input(artifact, &output)?);
        Ok(output)
    }

    /// Selects the artifact by file and contract name when `source` is set, else by name.
    fn select<'a>(
        &self,
        build: &'a ProjectCompileOutput,
    ) -> Result<&'a ConfigurableContractArtifact, Box<PhoundryError>> {
        let name = &self.assertion_contract;
        let Some(source) = &self.source else {
            return build
                .find_contract(ContractInfo::new(name))
                .ok_or_else(|| PhoundryError::ContractNotFound(name.clone()).into());
        };
        // A bare file name lives in the contracts directory.
        let path = if source.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
            self.project_root()?.join(source)
        } else {
            self.project_root()?.join(&self.contracts).join(source)
        };
        let exact = foundry_compilers::utils::canonicalize(&path).ok();
        let mut matches: Vec<_> = build
            .artifact_ids()
            .filter(|(id, _)| &id.name == name)
            .map(|(id, artifact)| (id.source, artifact))
            .collect();
        if let Some((_, artifact)) = matches
            .iter()
            .find(|(file, _)| Some(file) == exact.as_ref())
        {
            return Ok(artifact);
        }
        // Entries naming a missing or different file built by contract name before file
        // matching; keep that when exactly one file declares the contract.
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches.dedup_by(|a, b| a.0 == b.0);
        match matches.as_slice() {
            [(_, artifact)] => Ok(artifact),
            [] => {
                Err(Box::new(PhoundryError::AssertionNotFound {
                    path,
                    contract: name.clone(),
                }))
            }
            many => {
                Err(Box::new(PhoundryError::AmbiguousAssertion {
                    path,
                    contract: name.clone(),
                    candidates: many.iter().map(|(file, _)| file.clone()).collect(),
                }))
            }
        }
    }

    /// Rebuilds the standard JSON input from the artifact's own metadata: its settings and
    /// exactly the sources it was compiled from. Recompiles that input in isolation and
    /// requires the same bytecode, so the input never depends on files it does not carry.
    fn release_input(
        &self,
        artifact: &ConfigurableContractArtifact,
        output: &BuildAndFlatOutput,
    ) -> Result<String, Box<PhoundryError>> {
        let not_reproducible = |reason: String| {
            Box::new(PhoundryError::ReleaseInputNotReproducible {
                contract: format!("{}:{}", output.compilation_target, self.assertion_contract),
                reason,
            })
        };
        let metadata: Value = artifact
            .raw_metadata
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| not_reproducible(format!("invalid contract metadata: {e}")))?
            .ok_or_else(|| not_reproducible("missing contract metadata".into()))?;
        let config = self.build_opts().load_config()?;
        let project = config.project().map_err(PhoundryError::SolcError)?;

        let mut sources = serde_json::Map::new();
        let mut include_paths = None;
        for (name, source) in metadata["sources"].as_object().into_iter().flatten() {
            let content = if let Some(content) = source["content"].as_str() {
                content.to_string()
            } else {
                let hash = source["keccak256"].as_str().unwrap_or_default();
                // solc names sources by import path; Foundry reads them from the root, then
                // its include paths. Only resolve the import graph when the root misses.
                read_source(&project.paths.root, name, hash)
                    .or_else(|| {
                        include_paths
                            .get_or_insert_with(|| solc_include_paths(&project))
                            .iter()
                            .find_map(|dir| read_source(dir, name, hash))
                    })
                    .ok_or_else(|| {
                        not_reproducible(format!(
                            "source {name} no longer matches the build; rebuild and retry"
                        ))
                    })?
            };
            sources.insert(name.clone(), serde_json::json!({ "content": content }));
        }

        let mut settings = metadata["settings"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        settings.remove("compilationTarget");
        // Metadata keys libraries as `file:Library`; standard JSON nests them by file.
        if let Some(Value::Object(libraries)) = settings.remove("libraries") {
            let mut nested = serde_json::Map::new();
            for (key, address) in libraries {
                let (file, library) = key.rsplit_once(':').unwrap_or(("", &key));
                nested
                    .entry(file)
                    .or_insert_with(|| Value::Object(serde_json::Map::new()))
                    .as_object_mut()
                    .map(|libs| libs.insert(library.to_string(), address));
            }
            settings.insert("libraries".into(), Value::Object(nested));
        }
        settings.insert(
            "outputSelection".into(),
            serde_json::json!({ "*": { "*": ["abi", "evm.bytecode.object", "evm.deployedBytecode.object", "metadata"] } }),
        );
        let input = serde_json::json!({
            "language": metadata["language"],
            "sources": sources,
            "settings": settings,
        });

        let rebuilt = recompile_isolated(&config, output, &input).map_err(not_reproducible)?;
        let rebuilt = rebuilt["contracts"][&output.compilation_target][&self.assertion_contract]
            ["evm"]["bytecode"]["object"]
            .as_str()
            .map(|bytes| format!("0x{}", bytes.trim_start_matches("0x")));
        if rebuilt.as_deref() != Some(output.bytecode.as_str()) {
            return Err(not_reproducible(
                "solc emits different bytecode from only the assertion's sources; move the \
                 assertion into its own directory and point its credible.toml file there"
                    .into(),
            ));
        }
        serde_json::to_string(&input)
            .map_err(|e| not_reproducible(format!("failed to serialize compiler input: {e}")))
    }

    fn project_root(&self) -> Result<PathBuf, Box<PhoundryError>> {
        match &self.root {
            Some(root) => Ok(root.clone()),
            None => {
                find_project_root(None)
                    .map_err(|_| Box::new(PhoundryError::DirectoryNotFound(PathBuf::from("."))))
            }
        }
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
        let path = self.project_root()?.join(rel_source_path);

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

/// Reads `dir/name` when its keccak256 matches the hash the compiler recorded for it.
fn read_source(dir: &Path, name: &str, keccak256: &str) -> Option<String> {
    let content = std::fs::read_to_string(dir.join(name)).ok()?;
    (alloy_primitives::keccak256(content.as_bytes()).to_string() == keccak256).then_some(content)
}

/// Configured include paths plus the library roots Foundry adds while resolving imports.
fn solc_include_paths(project: &Project) -> Vec<PathBuf> {
    let mut paths: Vec<_> = project.paths.include_paths.iter().cloned().collect();
    if let Ok(graph) = Graph::<MultiCompilerParser>::resolve(&project.paths) {
        paths.extend(graph.into_sources().1.include_paths().iter().cloned());
    }
    paths
}

/// Compiles `input` with the build's solc, rooted in an empty directory so solc cannot read
/// any file the input does not carry.
fn recompile_isolated(
    config: &Config,
    output: &BuildAndFlatOutput,
    input: &Value,
) -> Result<Value, String> {
    let version = output
        .compiler_version_short()
        .map_err(|e| e.to_string())?
        .parse()
        .map_err(|e| format!("invalid solc version: {e}"))?;
    let mut solc = match &config.solc {
        Some(SolcReq::Local(path)) => Solc::new(path).map_err(|e| e.to_string())?,
        _ => {
            Solc::find_svm_installed_version(&version)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("solc {version} is not installed"))?
        }
    };
    let sandbox = tempfile::tempdir().map_err(|e| e.to_string())?;
    solc.base_path = Some(sandbox.path().to_path_buf());
    let rebuilt: Value = solc.compile_as(input).map_err(|e| e.to_string())?;
    if let Some(error) = rebuilt["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|error| error["severity"] == "error")
    {
        let message = error["formattedMessage"]
            .as_str()
            .or_else(|| error["message"].as_str())
            .unwrap_or_default();
        return Err(format!("solc rejects the input on its own: {message}"));
    }
    Ok(rebuilt)
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
            source: None,
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

    fn release_args(root: &Path, name: &str, source: Option<&str>) -> BuildAndFlattenArgs {
        BuildAndFlattenArgs {
            root: Some(root.to_path_buf()),
            assertion_contract: name.to_string(),
            contracts: PathBuf::from("src"),
            source: source.map(PathBuf::from),
        }
    }

    /// Compiles `input` with no readable files, as a verifier holding only the input would.
    fn recompile_alone(output: &BuildAndFlatOutput, name: &str) -> String {
        let input: Value = serde_json::from_str(output.compiler_input.as_deref().unwrap()).unwrap();
        let version = output.compiler_version_short().unwrap().parse().unwrap();
        let mut solc = Solc::find_or_install(&version).unwrap();
        let empty = TempDir::new().unwrap();
        solc.base_path = Some(empty.path().to_path_buf());
        let rebuilt: Value = solc.compile_as(&input).unwrap();
        let bytes =
            rebuilt["contracts"][&output.compilation_target][name]["evm"]["bytecode"]["object"]
                .as_str()
                .unwrap_or_else(|| panic!("no bytecode: {rebuilt}"));
        format!("0x{bytes}")
    }

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_input_rebuilds_the_selected_artifact_with_only_its_imports() {
        let (_temp_dir, project_root) = setup_test_project();
        write(
            &project_root,
            "foundry.toml",
            r#"
[profile.default]
solc = "0.8.28"
offline = true
via_ir = true
optimizer = true
optimizer_runs = 731
bytecode_hash = "ipfs"
[profile.default.optimizer_details]
cse = false
"#,
        );
        write(
            &project_root,
            "src/Vault.sol",
            r#"// SPDX-License-Identifier: MIT
pragma solidity 0.8.28;
import {TestContract} from "./TestContract.sol";
contract Vault is TestContract {}"#,
        );
        // Sorts first and shares the name, so name-only lookup would pick it.
        write(
            &project_root,
            "src/AAA.sol",
            "pragma solidity 0.8.28; contract Vault { function unrelated() external {} }",
        );
        let output = release_args(&project_root, "Vault", Some("src/Vault.sol"))
            .run_for_release()
            .unwrap();
        assert_eq!(output.compilation_target, "src/Vault.sol");
        let input: Value = serde_json::from_str(output.compiler_input.as_deref().unwrap()).unwrap();
        let sources = input["sources"].as_object().unwrap();
        assert_eq!(
            sources.keys().collect::<Vec<_>>(),
            ["src/TestContract.sol", "src/Vault.sol"]
        );
        assert_eq!(input["settings"]["viaIR"], true);
        assert_eq!(input["settings"]["optimizer"]["runs"], 731);
        assert_eq!(input["settings"]["optimizer"]["details"]["cse"], false);
        assert!(input["settings"].get("compilationTarget").is_none());
        assert_eq!(recompile_alone(&output, "Vault"), output.bytecode);

        // The regular build path selects the same file, from a warm cache.
        let verified = release_args(&project_root, "Vault", Some("src/Vault.sol"))
            .run()
            .unwrap();
        assert_eq!(verified.bytecode, output.bytecode);
        let warm = release_args(&project_root, "Vault", Some("src/Vault.sol"))
            .run_for_release()
            .unwrap();
        assert_eq!(warm.compiler_input, output.compiler_input);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_selection_matches_file_and_keeps_legacy_name_lookup() {
        let (_temp_dir, root) = setup_test_project();
        write(
            &root,
            "foundry.toml",
            "[profile.default]\nsolc = \"0.8.28\"\noffline = true\n",
        );
        write(
            &root,
            "src/A.sol",
            "pragma solidity 0.8.28; contract Dup { uint x; }",
        );
        write(
            &root,
            "src/B.sol",
            "pragma solidity 0.8.28; contract Dup { function f() external {} }",
        );
        let release =
            |name: &str, source: &str| release_args(&root, name, Some(source)).run_for_release();

        let qualified = release("Dup", "src/B.sol").unwrap();
        assert_eq!(qualified.compilation_target, "src/B.sol");
        // A bare file name resolves in the contracts directory.
        assert_eq!(
            release("Dup", "B.sol").unwrap().compilation_target,
            "src/B.sol"
        );
        // A file that does not declare the contract falls back to a unique declaration.
        assert_eq!(
            release("TestContract", "src/Missing.sol")
                .unwrap()
                .compilation_target,
            "src/TestContract.sol"
        );
        assert!(matches!(
            *release("Dup", "src/Missing.sol").unwrap_err(),
            PhoundryError::AmbiguousAssertion { ref candidates, .. } if candidates.len() == 2
        ));
        let missing = release("Typo", "src/Typo.sol").unwrap_err();
        assert!(matches!(*missing, PhoundryError::AssertionNotFound { .. }));
        assert!(missing.to_string().contains("src/Typo.sol"), "{missing}");
        // Without a file, lookup stays by name.
        assert!(release_args(&root, "TestContract", None).run().is_ok());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_keeps_the_project_build_compiler_and_errors() {
        // Offline auto-detection needs 0.8.28 installed; a newer one makes this a regression test.
        if !Solc::installed_versions()
            .iter()
            .any(|v| v.to_string() == "0.8.28")
        {
            return;
        }
        let (_temp_dir, root) = setup_test_project();
        write(&root, "foundry.toml", "[profile.default]\noffline = true\n");
        write(&root, "src/A.sol", "pragma solidity ^0.8.28; contract A {}");
        write(
            &root,
            "src/Pin.sol",
            "pragma solidity 0.8.28; contract Pin {}",
        );
        let output = release_args(&root, "A", Some("src/A.sol"))
            .run_for_release()
            .unwrap();
        // The full project resolves a version every file accepts, not the newest for A alone.
        assert!(
            output.compiler_version.starts_with("v0.8.28+"),
            "{}",
            output.compiler_version
        );
        assert_eq!(recompile_alone(&output, "A"), output.bytecode);

        write(
            &root,
            "src/Invalid.sol",
            "pragma solidity ^0.8.28; invalid syntax;",
        );
        let error = release_args(&root, "A", Some("src/A.sol"))
            .run_for_release()
            .unwrap_err();
        assert!(
            matches!(*error, PhoundryError::CompilationError(_)),
            "{error}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_input_refuses_inputs_that_do_not_reproduce_the_build() {
        let (_temp_dir, root) = setup_test_project();
        write(
            &root,
            "foundry.toml",
            "[profile.default]\nsolc = \"0.8.28\"\noffline = true\n",
        );
        let args = release_args(&root, "TestContract", Some("src/TestContract.sol"));
        let build = crate::compile::compile(args.build_opts()).unwrap();
        let artifact = args.select(&build).unwrap();
        let output = args.output(artifact).unwrap();
        assert!(args.release_input(artifact, &output).is_ok());

        let other_bytes = BuildAndFlatOutput {
            bytecode: "0x00".into(),
            ..args.output(artifact).unwrap()
        };
        let error = args.release_input(artifact, &other_bytes).unwrap_err();
        assert!(error.to_string().contains("different bytecode"), "{error}");

        // A source edited after the build no longer matches the recorded hash.
        write(
            &root,
            "src/TestContract.sol",
            "pragma solidity 0.8.28; contract TestContract {}",
        );
        let error = args.release_input(artifact, &output).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("src/TestContract.sol no longer matches"),
            "{error}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_input_carries_sources_found_through_include_paths() {
        let (_temp_dir, root) = setup_test_project();
        write(
            &root,
            "foundry.toml",
            "[profile.default]\nsolc = \"0.8.28\"\noffline = true\n",
        );
        write(
            &root,
            "lib/foo/src/Y.sol",
            "pragma solidity 0.8.28; library Y { function v() internal pure returns (uint) { return 7; } }",
        );
        write(
            &root,
            "lib/foo/src/X.sol",
            "pragma solidity 0.8.28; import {Y} from \"src/Y.sol\"; library X { function v() internal pure returns (uint) { return Y.v(); } }",
        );
        write(
            &root,
            "src/Uses.sol",
            "pragma solidity 0.8.28; import {X} from \"lib/foo/src/X.sol\"; contract Uses { function v() external pure returns (uint) { return X.v(); } }",
        );
        let output = release_args(&root, "Uses", Some("src/Uses.sol"))
            .run_for_release()
            .unwrap();
        let input: Value = serde_json::from_str(output.compiler_input.as_deref().unwrap()).unwrap();
        assert!(
            input["sources"]["src/Y.sol"]["content"]
                .as_str()
                .unwrap()
                .contains("return 7")
        );
        assert!(input["sources"].get("src/TestContract.sol").is_none());
        assert_eq!(recompile_alone(&output, "Uses"), output.bytecode);
    }
}
