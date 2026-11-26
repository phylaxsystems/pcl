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
    ArtifactId,
    Graph,
    Project,
    ProjectCompileOutput,
    artifacts::{
        ConfigurableContractArtifact,
        Source,
    },
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
        let artifact = match self.source {
            Some(_) => self.select(&build)?.1,
            None => {
                build
                    .find_contract(ContractInfo::new(&self.assertion_contract))
                    .ok_or_else(|| {
                        PhoundryError::ContractNotFound(self.assertion_contract.clone())
                    })?
            }
        };
        self.output(artifact)
    }

    /// Like [`Self::run`], plus the original compiler input of the selected artifact's job,
    /// keeping only the sources the artifact was compiled from.
    pub fn run_for_release(&self) -> Result<BuildAndFlatOutput, Box<PhoundryError>> {
        foundry_cli::utils::load_dotenv();
        let build = crate::compile::compile_with_build_info(self.build_opts())?;
        let (id, artifact) = self.select(&build)?;
        // Newly compiled: this build holds the job's full build info.
        if let Some(job) = compiled_job(&build, &id.build_id) {
            return self.release_output(artifact, &job);
        }
        // Cached: the build info Foundry retained for the job has its input when the build
        // that compiled it kept full build info.
        let project = self
            .build_opts()
            .load_config()?
            .project()
            .map_err(PhoundryError::SolcError)?;
        if let Some(job) = retained_job(&project, &id.build_id) {
            return self.release_output(artifact, &job);
        }
        // Builds without full build info (older PCL, `forge build`) retained no input. Without
        // its cache index, Foundry rebuilds the whole project through its normal paths and
        // writes artifacts, cache and full build info that this and later releases reuse, so
        // the artifact and its job both come from that build.
        let cache = project.cache_path();
        if let Err(error) = std::fs::remove_file(cache)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(Box::new(PhoundryError::ReleaseInputNotReproducible {
                contract: format!("{}:{}", id.source.display(), id.name),
                reason: format!("cannot rebuild the cache {}: {error}", cache.display()),
            }));
        }
        let build = crate::compile::compile_with_build_info(self.build_opts())?;
        let (id, artifact) = self.select(&build)?;
        let job = compiled_job(&build, &id.build_id).ok_or_else(|| {
            Box::new(PhoundryError::ReleaseInputNotReproducible {
                contract: format!("{}:{}", id.source.display(), id.name),
                reason: "the rebuild kept no compiler input; rebuild and retry".into(),
            })
        })?;
        self.release_output(artifact, &job)
    }

    fn release_output(
        &self,
        artifact: &ConfigurableContractArtifact,
        job: &Value,
    ) -> Result<BuildAndFlatOutput, Box<PhoundryError>> {
        let mut output = self.output(artifact)?;
        output.compiler_input = Some(self.release_input(artifact, &output, job)?);
        Ok(output)
    }

    /// Selects the artifact declaring the contract in `source`. A bare file name is tried at
    /// the root, then in the contracts directory.
    fn select<'a>(
        &self,
        build: &'a ProjectCompileOutput,
    ) -> Result<(ArtifactId, &'a ConfigurableContractArtifact), Box<PhoundryError>> {
        let name = &self.assertion_contract;
        let source = self.source.as_deref().unwrap_or_else(|| Path::new(name));
        let root = self.project_root()?;
        let mut candidates = vec![root.join(source)];
        if source.parent().is_some_and(|p| p.as_os_str().is_empty()) {
            candidates.push(root.join(&self.contracts).join(source));
        }
        let mut matches: Vec<_> = build
            .artifact_ids()
            .filter(|(id, _)| &id.name == name)
            .collect();
        for path in &candidates {
            let exact = foundry_compilers::utils::canonicalize(path).ok();
            if let Some(found) = matches
                .iter()
                .position(|(id, _)| Some(&id.source) == exact.as_ref())
            {
                return Ok(matches.swap_remove(found));
            }
        }
        // Entries naming a missing or different file built by contract name before file
        // matching; keep that when exactly one file declares the contract.
        matches.sort_by(|a, b| a.0.source.cmp(&b.0.source));
        matches.dedup_by(|a, b| a.0.source == b.0.source);
        let path = candidates.swap_remove(0);
        match matches.len() {
            1 => Ok(matches.swap_remove(0)),
            0 => {
                Err(Box::new(PhoundryError::AssertionNotFound {
                    path,
                    contract: name.clone(),
                }))
            }
            _ => {
                Err(Box::new(PhoundryError::AmbiguousAssertion {
                    path,
                    contract: name.clone(),
                    candidates: matches.into_iter().map(|(id, _)| id.source).collect(),
                }))
            }
        }
    }

    /// The job's original input with only the artifact's sources; the job's settings are kept
    /// as compiled. A source solc loaded through an include path is read the way Foundry and
    /// solc read it. The input must recompile alone to the same bytecode.
    fn release_input(
        &self,
        artifact: &ConfigurableContractArtifact,
        output: &BuildAndFlatOutput,
        job: &Value,
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
            let hash = source["keccak256"].as_str().unwrap_or_default();
            let content = job["sources"][name]["content"]
                .as_str()
                .filter(|content| keccak_hex(content) == hash)
                .map(ToString::to_string)
                .or_else(|| {
                    std::iter::once(&project.paths.root)
                        .chain(
                            include_paths
                                .get_or_insert_with(|| solc_include_paths(&project))
                                .iter(),
                        )
                        .find_map(|dir| read_source(&dir.join(name), hash))
                })
                .ok_or_else(|| {
                    not_reproducible(format!(
                        "source {name} no longer matches the build; rebuild and retry"
                    ))
                })?;
            sources.insert(name.clone(), serde_json::json!({ "content": content }));
        }
        // Drop Foundry's machine-specific CLI paths (basePath, includePaths, allowPaths).
        let input = serde_json::json!({
            "language": job["language"],
            "sources": sources,
            "settings": job["settings"],
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

fn keccak_hex(content: &str) -> String {
    alloy_primitives::keccak256(content.as_bytes()).to_string()
}

/// The job input Foundry retained in `build_id`'s build info, if that build kept full build info.
fn retained_job(project: &Project, build_id: &str) -> Option<Value> {
    let path = project
        .paths
        .build_infos
        .join(build_id)
        .with_extension("json");
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(|info| info["id"] == build_id)
        .and_then(|mut info| info.get_mut("input").map(Value::take))
}

/// The full input of `build_id` when this build compiled it with full build info.
fn compiled_job(build: &ProjectCompileOutput, build_id: &str) -> Option<Value> {
    build
        .output()
        .build_infos
        .iter()
        .find(|info| info.id == build_id)
        .and_then(|info| info.build_info.get("input").cloned())
}

/// Reads `path` when its content matches the hash the compiler recorded: as Foundry passes
/// sources (line endings normalized) or as solc reads an include-path import (raw).
fn read_source(path: &Path, keccak256: &str) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let normalized = Source::read(path).ok()?.content.to_string();
    [normalized, raw]
        .into_iter()
        .find(|content| keccak_hex(content) == keccak256)
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
        // The build ran this path from the current directory; replay runs elsewhere.
        Some(SolcReq::Local(path)) => {
            Solc::new(std::path::absolute(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        }
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

        // A bare file name at the root wins over the contracts directory, as it did before.
        write(
            &root,
            "B.sol",
            "pragma solidity 0.8.28; contract Dup { function root() external {} }",
        );
        write(
            &root,
            "src/UsesRoot.sol",
            "pragma solidity 0.8.28; import {Dup as RootDup} from \"B.sol\"; contract UsesRoot {}",
        );
        assert_eq!(release("Dup", "B.sol").unwrap().compilation_target, "B.sol");
        assert_eq!(
            release("Dup", "src/B.sol").unwrap().compilation_target,
            "src/B.sol"
        );
    }

    /// Releases `source` and returns its `compilerInput`, which must recompile alone to the
    /// same bytecode.
    fn release_input_of(root: &Path, source: &str, name: &str) -> Value {
        let output = release_args(root, name, Some(source))
            .run_for_release()
            .unwrap();
        assert_eq!(recompile_alone(&output, name), output.bytecode);
        let input: Value = serde_json::from_str(output.compiler_input.as_deref().unwrap()).unwrap();
        // Standard JSON fields only, without Foundry's machine-specific paths.
        assert_eq!(
            input.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["language", "sources", "settings"]
        );
        input
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_input_keeps_the_original_job_settings_on_every_cache_state() {
        let (_temp_dir, root) = setup_test_project();
        write(
            &root,
            "foundry.toml",
            r#"
[profile.default]
solc = "0.8.28"
offline = true
via_ir = false
optimizer = true
optimizer_runs = 321
[profile.default.model_checker]
engine = "none"
"#,
        );
        write(
            &root,
            "src/Vault.sol",
            "pragma solidity 0.8.28; import {TestContract} from \"./TestContract.sol\"; contract Vault is TestContract {}",
        );
        write(
            &root,
            "src/Other.sol",
            "pragma solidity 0.8.28; contract Other {}",
        );

        // An older warm cache: a regular build retains build info without its input.
        crate::compile::compile(release_args(&root, "Vault", None).build_opts()).unwrap();
        let older = release_input_of(&root, "src/Vault.sol", "Vault");
        // Cold: the release build keeps the full job and retains it as build info.
        fs::remove_dir_all(root.join("out")).unwrap();
        fs::remove_dir_all(root.join("assertions/cache")).unwrap();
        let cold = release_input_of(&root, "src/Vault.sol", "Vault");
        // Warm with that retained build info.
        let warm = release_input_of(&root, "src/Vault.sol", "Vault");
        assert_eq!(older, cold);
        assert_eq!(warm, cold);

        let retained: Vec<Value> = fs::read_dir(root.join("out/build-info"))
            .unwrap()
            .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
            .filter(|info: &Value| info.get("input").is_some())
            .collect();
        let job = &retained
            .iter()
            .find(|info| info["input"]["sources"].get("src/Vault.sol").is_some())
            .unwrap()["input"];
        assert_eq!(cold["settings"], job["settings"]);
        assert_eq!(cold["language"], job["language"]);
        assert_eq!(cold["settings"]["viaIR"], false);
        assert_eq!(
            cold["settings"]["modelChecker"],
            serde_json::json!({ "engine": "none", "targets": ["assert"] })
        );
        // Only the artifact's sources, with the job's contents.
        let sources = cold["sources"].as_object().unwrap();
        assert_eq!(
            sources.keys().collect::<Vec<_>>(),
            ["src/TestContract.sol", "src/Vault.sol"]
        );
        for (name, source) in sources {
            assert_eq!(source, &job["sources"][name]);
        }
    }

    /// The build info Foundry's cache records for `name` in `source`: its path and contents.
    fn recorded_job(root: &Path, source: &str, name: &str) -> (PathBuf, Value) {
        let cache: Value = serde_json::from_slice(
            &fs::read(root.join("assertions/cache/solidity-files-cache.json")).unwrap(),
        )
        .unwrap();
        let mut ids = vec![];
        let mut pending = vec![&cache["files"][source]["artifacts"][name]];
        while let Some(value) = pending.pop() {
            if let Value::Object(map) = value {
                ids.extend(map.get("build_id").and_then(Value::as_str));
                pending.extend(map.values());
            }
        }
        assert_eq!(ids.len(), 1, "{cache}");
        let path = root.join("out/build-info").join(format!("{}.json", ids[0]));
        let info = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        (path, info)
    }

    /// Strips the retained input of `source`'s job, as a build without full build info leaves it.
    fn forget_job_input(root: &Path, source: &str, name: &str) {
        let (path, mut info) = recorded_job(root, source, name);
        let info = info.as_object_mut().unwrap();
        info.remove("input");
        info.remove("output");
        fs::write(path, serde_json::to_vec(info).unwrap()).unwrap();
    }

    /// Build info files and their modification times; a recompile writes new ones.
    fn build_outputs(root: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
        let mut entries: Vec<_> = fs::read_dir(root.join("out/build-info"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.path(), entry.metadata().unwrap().modified().unwrap())
            })
            .collect();
        entries.sort();
        entries
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_without_retained_input_sends_the_rebuilt_artifact_and_its_job() {
        let (_temp_dir, root) = setup_test_project();
        write(
            &root,
            "foundry.toml",
            "[profile.default]\nsolc = \"0.8.28\"\noffline = true\n",
        );
        write(
            &root,
            "src/Vault.sol",
            "pragma solidity 0.8.28; import {TestContract} from \"./TestContract.sol\"; contract Vault is TestContract {}",
        );
        write(
            &root,
            "src/Other.sol",
            "pragma solidity 0.8.28; contract Other {}",
        );
        write(
            &root,
            "src/Second.sol",
            "pragma solidity 0.8.28; contract Second { function s() external {} }",
        );
        // A test importing Vault is recompiled with it and keeps artifacts from both jobs.
        write(
            &root,
            "test/Vault.t.sol",
            "pragma solidity 0.8.28; import {Vault} from \"../src/Vault.sol\"; contract VaultTest {}",
        );
        release_input_of(&root, "src/Vault.sol", "Vault");
        // An incremental change compiles Vault alone while Other stays cached; then the job's
        // retained build info loses its input, as with a build that kept no full build info.
        write(
            &root,
            "src/Vault.sol",
            "pragma solidity 0.8.28; import {TestContract} from \"./TestContract.sol\"; contract Vault is TestContract { function v() external {} }",
        );
        let incremental = release_input_of(&root, "src/Vault.sol", "Vault");
        let (_, job) = recorded_job(&root, "src/Vault.sol", "Vault");
        assert_eq!(incremental["settings"], job["input"]["settings"]);
        forget_job_input(&root, "src/Vault.sol", "Vault");
        forget_job_input(&root, "src/Second.sol", "Second");

        let sent = release_input_of(&root, "src/Vault.sol", "Vault");
        // The sent input is the job that built the artifact now recorded for Vault.
        let (_, job) = recorded_job(&root, "src/Vault.sol", "Vault");
        assert_eq!(sent["settings"], job["input"]["settings"]);
        assert_eq!(sent["language"], job["input"]["language"]);
        for (name, source) in sent["sources"].as_object().unwrap() {
            assert_eq!(source, &job["input"]["sources"][name]);
        }
        // That rebuild retained full build info for every artifact: a second assertion and
        // later releases reuse it without rebuilding.
        let (_, second_job) = recorded_job(&root, "src/Second.sol", "Second");
        assert!(second_job.get("input").is_some());
        let before = build_outputs(&root);
        release_input_of(&root, "src/Second.sol", "Second");
        assert_eq!(release_input_of(&root, "src/Vault.sol", "Vault"), sent);
        assert_eq!(build_outputs(&root), before);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_input_accepts_crlf_sources_cold_and_cached() {
        let (_temp_dir, root) = setup_test_project();
        write(
            &root,
            "foundry.toml",
            "[profile.default]\nsolc = \"0.8.28\"\noffline = true\n",
        );
        write(
            &root,
            "src/TestContract.sol",
            "// SPDX-License-Identifier: MIT\r\npragma solidity 0.8.28;\r\ncontract TestContract {}\r\n",
        );
        write(
            &root,
            "src/Vault.sol",
            "pragma solidity 0.8.28;\r\nimport {TestContract} from \"./TestContract.sol\";\r\ncontract Vault is TestContract {}\r\n",
        );
        for _ in ["cold", "cached"] {
            let input = release_input_of(&root, "src/Vault.sol", "Vault");
            assert!(!input["sources"].to_string().contains("\\r"), "{input}");
        }
        // An older warm cache takes the clean-rebuild path.
        fs::remove_dir_all(root.join("out/build-info")).unwrap();
        crate::compile::compile(release_args(&root, "Vault", None).build_opts()).unwrap();
        release_input_of(&root, "src/Vault.sol", "Vault");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn release_replays_a_relative_local_solc() {
        let Some(installed) = Solc::find_svm_installed_version(&"0.8.28".parse().unwrap()).unwrap()
        else {
            return;
        };
        let (_temp_dir, root) = setup_test_project();
        fs::copy(&installed.solc, root.join("solc")).unwrap();
        // Foundry resolves a local solc from the current directory, which tests cannot change.
        let cwd = std::env::current_dir().unwrap();
        let relative = PathBuf::from("../".repeat(cwd.components().count() - 1))
            .join(root.join("solc").strip_prefix("/").unwrap());
        write(
            &root,
            "foundry.toml",
            &format!("[profile.default]\nsolc = \"{}\"\n", relative.display()),
        );
        release_input_of(&root, "src/TestContract.sol", "TestContract");
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
        let build = crate::compile::compile_with_build_info(args.build_opts()).unwrap();
        let (id, artifact) = args.select(&build).unwrap();
        let output = args.output(artifact).unwrap();
        let job = compiled_job(&build, &id.build_id).unwrap();
        assert!(args.release_input(artifact, &output, &job).is_ok());

        let other_bytes = BuildAndFlatOutput {
            bytecode: "0x00".into(),
            ..args.output(artifact).unwrap()
        };
        let error = args
            .release_input(artifact, &other_bytes, &job)
            .unwrap_err();
        assert!(error.to_string().contains("different bytecode"), "{error}");

        // Sources come from the compiler job, not from the disk after the build.
        write(
            &root,
            "src/TestContract.sol",
            "pragma solidity 0.8.28; contract TestContract {}",
        );
        let input = args.release_input(artifact, &output, &job).unwrap();
        assert!(input.contains("function test()"), "{input}");
        // Without the job's copy, a source whose file changed is refused.
        let mut without_sources = job.clone();
        without_sources["sources"] = Value::Object(serde_json::Map::new());
        let error = args
            .release_input(artifact, &output, &without_sources)
            .unwrap_err();
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
