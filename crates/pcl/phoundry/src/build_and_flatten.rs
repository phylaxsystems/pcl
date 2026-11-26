use clap::{
    Parser,
    ValueHint,
};
use foundry_cli::opts::{
    BuildOpts,
    ProjectPathOpts,
};
use foundry_compilers::{
    Graph,
    ProjectCompileOutput,
    ProjectPathsConfig,
    artifacts::sources::Source,
    error::SolcError,
    flatten::{
        Flattener,
        FlattenerError,
    },
    info::ContractInfo,
    resolver::parse::SolParser,
    solc::SolcLanguage,
};
use foundry_config::Config;

use alloy_json_abi::JsonAbi;
use alloy_primitives::keccak256;
use serde_json::Value;

use foundry_compilers::artifacts::{
    BytecodeHash,
    MetadataSources,
    remappings::{
        RelativeRemapping,
        Remapping,
    },
};
use foundry_config::find_project_root;
use std::{
    collections::{
        BTreeMap,
        BTreeSet,
        HashMap,
    },
    path::{
        Path,
        PathBuf,
    },
    str::FromStr,
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

        let (build, config) = self.build()?;
        let info = ContractInfo::new(&self.assertion_contract);
        let root = self.project_root()?;
        let contract_path = select_contract_path(&build, &info.name, info.path.as_deref(), &root)?;
        let artifact = build
            .find(contract_path.as_path(), &info.name)
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;

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

        let rel_source_path = metadata
            .settings
            .compilation_target
            .iter()
            .find_map(|(path, name)| (name == &info.name).then_some(path))
            .ok_or_else(|| PhoundryError::ContractNotFound(self.assertion_contract.clone()))?;
        check_compilation_target(
            &root,
            &info.name,
            &contract_path,
            Path::new(rel_source_path),
        )?;
        let path = root.join(rel_source_path);
        let flattened =
            Self::flatten_verified(&root, &metadata.sources, &path, &settings, &config)?;
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

    /// Builds the project and returns the output plus the config that compiled it.
    fn build(&self) -> Result<(ProjectCompileOutput, Config), Box<PhoundryError>> {
        let build_opts = BuildOpts {
            project_paths: ProjectPathOpts {
                root: self.root.clone(),
                contracts: Some(self.contracts.clone()),
                ..Default::default()
            },
            ..Default::default()
        };

        crate::compile::compile_with_config(build_opts)
    }

    /// The canonical project root: Foundry reports canonical artifact paths
    /// (`/private/var` on macOS), so source paths are compared against it.
    fn project_root(&self) -> Result<PathBuf, Box<PhoundryError>> {
        let root = match &self.root {
            Some(root) => root.clone(),
            None => {
                find_project_root(None)
                    .map_err(|_| Box::new(PhoundryError::DirectoryNotFound(PathBuf::from("."))))?
            }
        };
        root.canonicalize()
            .map_err(|_| Box::new(PhoundryError::DirectoryNotFound(root)))
    }

    /// Flattens `path` only when every compiled source still matches the artifact.
    ///
    /// The flattener reads the target and its imports from disk. The check runs
    /// before that read and again after it, so a file that changed once
    /// compilation finished is not uploaded with the bytecode just verified.
    /// Resolution uses the config captured at compilation, with the artifact's
    /// remappings, so a later `foundry.toml` cannot select a different import.
    fn flatten_verified(
        root: &Path,
        sources: &MetadataSources,
        path: &PathBuf,
        settings: &Value,
        config: &Config,
    ) -> Result<String, Box<PhoundryError>> {
        verify_compiled_sources(root, sources, &config.include_paths, &config.libs)?;
        let flattened = Self::flatten(path, settings, sources, root, config)?;
        verify_compiled_sources(root, sources, &config.include_paths, &config.libs)?;
        Ok(flattened)
    }

    /// Flattens the contract with the compiler config captured for this artifact.
    fn flatten(
        path: &PathBuf,
        settings: &Value,
        sources: &MetadataSources,
        root: &Path,
        config: &Config,
    ) -> Result<String, Box<PhoundryError>> {
        let mut config = config.clone();
        let mut remappings =
            closure_remappings(root, sources, &config.include_paths, &config.libs)?;
        remappings.extend(compiler_remappings(settings, &config.root)?);
        config.remappings = remappings;
        config.auto_detect_remappings = false;
        let project = config
            .ephemeral_project()
            .map_err(|e| Box::new(PhoundryError::SolcError(e)))?;
        let can_path = std::fs::canonicalize(path)
            .map_err(|e| Box::new(PhoundryError::CanonicalizePathError(e)))?;
        imports_stay_in_source_closure(
            &project.paths.clone().with_language::<SolcLanguage>(),
            &can_path,
            root,
            sources,
            &config.include_paths,
            &config.libs,
        )?;

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

/// Pins source-unit names that are not files under the project root to the
/// file the captured include paths and libs compiled.
fn closure_remappings(
    root: &Path,
    sources: &MetadataSources,
    include_paths: &[PathBuf],
    libs: &[PathBuf],
) -> Result<Vec<RelativeRemapping>, Box<PhoundryError>> {
    let mut pinned = Vec::new();
    for key in sources.inner.keys() {
        let direct = resolve_source(root, Path::new(key));
        if direct.is_file() {
            continue;
        }
        let located = locate_compiled_source(root, Path::new(key), include_paths, libs);
        if !located.is_file() {
            return Err(source_hash_mismatch(root, Path::new(key)));
        }
        let raw = format!("{key}={}", located.display());
        let parsed = Remapping::from_str(&raw).map_err(|_| {
            Box::new(PhoundryError::InvalidForgeOutput(
                "artifact metadata source cannot be pinned",
            ))
        })?;
        pinned.push(RelativeRemapping::new(parsed, root));
    }
    Ok(pinned)
}

/// Remappings solc used for this artifact, resolved against the project root.
///
/// An empty list means solc was given no remappings. A missing or unreadable
/// list fails closed.
fn compiler_remappings(
    settings: &Value,
    root: &Path,
) -> Result<Vec<RelativeRemapping>, Box<PhoundryError>> {
    let values = settings
        .get("remappings")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Box::new(PhoundryError::InvalidForgeOutput(
                "artifact metadata has no remappings",
            ))
        })?;
    values
        .iter()
        .map(|value| {
            let raw = value.as_str().ok_or_else(|| {
                Box::new(PhoundryError::InvalidForgeOutput(
                    "artifact metadata remapping is not a string",
                ))
            })?;
            let parsed = Remapping::from_str(raw).map_err(|_| {
                Box::new(PhoundryError::InvalidForgeOutput(
                    "artifact metadata remapping is invalid",
                ))
            })?;
            Ok(RelativeRemapping::new(parsed, root))
        })
        .collect()
}

/// The flattener resolves imports again. Every file it would read has to be
/// one of the artifact's compiled sources.
fn imports_stay_in_source_closure(
    paths: &ProjectPathsConfig<SolcLanguage>,
    target: &Path,
    root: &Path,
    sources: &MetadataSources,
    include_paths: &[PathBuf],
    libs: &[PathBuf],
) -> Result<(), Box<PhoundryError>> {
    let closure = sources
        .inner
        .keys()
        .map(|path| locate_compiled_source(root, Path::new(path), include_paths, libs))
        .collect::<BTreeSet<_>>();
    let read = Source::read_all([target.to_path_buf()])
        .map_err(|err| Box::new(PhoundryError::SolcError(SolcError::msg(err.to_string()))))?;
    let graph = Graph::<SolParser>::resolve_sources(paths, read)
        .map_err(|err| Box::new(PhoundryError::SolcError(err)))?;
    for path in graph.files().keys() {
        let resolved = locate_compiled_source(root, path, include_paths, libs);
        if !closure.contains(&resolved) {
            return Err(source_hash_mismatch(root, &resolved));
        }
    }
    Ok(())
}

/// Metadata names are project paths, or source-unit names found through the
/// include paths and libs that compiled the artifact.
fn locate_compiled_source(
    root: &Path,
    path: &Path,
    include_paths: &[PathBuf],
    libs: &[PathBuf],
) -> PathBuf {
    let key = path.strip_prefix(root).unwrap_or(path);
    let direct = resolve_source(root, key);
    if direct.is_file() {
        return direct;
    }
    for base in include_paths.iter().chain(libs.iter()) {
        let candidate = resolve_source(root, &base.join(key));
        if candidate.is_file() {
            return candidate;
        }
    }
    direct
}

fn select_contract_path(
    build: &ProjectCompileOutput,
    contract_name: &str,
    requested_path: Option<&str>,
    root: &Path,
) -> Result<PathBuf, Box<PhoundryError>> {
    // A configured source has a path. Foundry's lookup uses that resolved path
    // and, when the artifact key is project-relative, the path under the root.
    // The walk below is for a bare name, a spelling that is not the artifact
    // key, and a miss that has to name the files that were built.
    if let Some(requested) = requested_path {
        let resolved = resolve_source(root, Path::new(requested));
        if let Some(path) = foundry_source_key(build, contract_name, &resolved, root) {
            return Ok(path);
        }
    }
    select_contract_path_from_scan(build, contract_name, requested_path, root)
}

/// Exact artifact keys for `resolved`: the canonical path, then its
/// project-relative form. `ProjectCompileOutput::find` compares those keys
/// as stored and does not canonicalize.
fn foundry_source_key(
    build: &ProjectCompileOutput,
    contract_name: &str,
    resolved: &Path,
    root: &Path,
) -> Option<PathBuf> {
    let mut candidates = Vec::with_capacity(2);
    candidates.push(resolved.to_path_buf());
    if let Ok(relative) = resolved.strip_prefix(root)
        && relative != resolved
    {
        candidates.push(relative.to_path_buf());
    }
    candidates
        .into_iter()
        .find(|candidate| build.find(candidate, contract_name).is_some())
}

fn select_contract_path_from_scan(
    build: &ProjectCompileOutput,
    contract_name: &str,
    requested_path: Option<&str>,
    root: &Path,
) -> Result<PathBuf, Box<PhoundryError>> {
    // Keyed by resolved path, so every spelling of one file is one entry.
    let mut paths: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    for (path, name, _) in build.artifacts_with_files() {
        if name == contract_name {
            paths
                .entry(resolve_source(root, path))
                .or_insert_with(|| path.clone());
        }
    }

    if let Some(requested) = requested_path {
        let requested = resolve_source(root, Path::new(requested));
        return match paths.get(&requested) {
            Some(path) => Ok(path.clone()),
            None if paths.is_empty() => {
                Err(Box::new(PhoundryError::ContractNotFound(format!(
                    "{}:{contract_name}",
                    display_source(root, &requested)
                ))))
            }
            None => {
                Err(Box::new(PhoundryError::ContractNotAtPath {
                    name: contract_name.to_string(),
                    requested: display_source(root, &requested),
                    paths: display_sources(root, &paths),
                }))
            }
        };
    }

    let mut matches = paths.iter();
    match (matches.next(), matches.next()) {
        (Some((_, path)), None) => Ok(path.clone()),
        (None, _) => {
            Err(Box::new(PhoundryError::ContractNotFound(
                contract_name.to_string(),
            )))
        }
        (Some(_), Some(_)) => {
            Err(Box::new(PhoundryError::AmbiguousContract {
                name: contract_name.to_string(),
                paths: display_sources(root, &paths),
            }))
        }
    }
}

/// Fails closed unless the artifact was compiled from the requested source.
fn check_compilation_target(
    root: &Path,
    contract_name: &str,
    requested: &Path,
    target: &Path,
) -> Result<(), Box<PhoundryError>> {
    let requested = resolve_source(root, requested);
    let target = resolve_source(root, target);
    if requested == target {
        return Ok(());
    }
    Err(Box::new(PhoundryError::ArtifactTargetMismatch {
        requested: format!("{}:{contract_name}", display_source(root, &requested)),
        actual: format!("{}:{contract_name}", display_source(root, &target)),
    }))
}

/// Resolves `path` against the canonical root so `./`, `//`, `..` and symlink
/// spellings of one file compare equal. A missing file stays unresolved and so
/// matches no compiled source.
fn resolve_source(root: &Path, path: &Path) -> PathBuf {
    let joined = root.join(path);
    joined.canonicalize().unwrap_or(joined)
}

fn display_sources(root: &Path, paths: &BTreeMap<PathBuf, PathBuf>) -> String {
    paths
        .keys()
        .map(|path| display_source(root, path))
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_source(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Fails closed unless each metadata source is still the bytes solc hashed.
///
/// When the artifact carries the compiler input (`content`), that snapshot is
/// the source that has to be on disk. The hash covers the import closure, not
/// only the file named in the config.
fn verify_compiled_sources(
    root: &Path,
    sources: &MetadataSources,
    include_paths: &[PathBuf],
    libs: &[PathBuf],
) -> Result<(), Box<PhoundryError>> {
    if sources.inner.is_empty() {
        return Err(Box::new(PhoundryError::InvalidForgeOutput(
            "artifact metadata has no source hashes",
        )));
    }
    for (path, source) in &sources.inner {
        let key = Path::new(path);
        let file = locate_compiled_source(root, key, include_paths, libs);
        let on_disk =
            std::fs::read_to_string(&file).map_err(|_| source_hash_mismatch(root, &file))?;
        let consumed = consumed_source(root, key, on_disk);
        if let Some(snapshot) = source.content.as_deref()
            && consumed != snapshot
        {
            return Err(source_hash_mismatch(root, &file));
        }
        if !source_hashes_match(&source_keccak(&consumed), &source.keccak256) {
            return Err(source_hash_mismatch(root, &file));
        }
    }
    Ok(())
}

fn source_hash_mismatch(root: &Path, path: &Path) -> Box<PhoundryError> {
    Box::new(PhoundryError::SourceHashMismatch {
        path: display_source(root, path),
    })
}

/// The text solc hashed for this metadata source unit.
///
/// Foundry rewrites CRLF to LF for every file it puts in standard JSON, and
/// those units are files under the project root. A unit solc loaded itself
/// through an include path, such as `import "Lib.sol"`, is not one of those
/// files. The compiler hashes that callback read unchanged, CRLF included.
fn consumed_source(root: &Path, key: &Path, on_disk: String) -> String {
    if resolve_source(root, key).is_file() {
        normalized_source(&on_disk)
    } else {
        on_disk
    }
}

/// Foundry's rewrite of a standard-JSON source before solc hashes it.
fn normalized_source(content: &str) -> String {
    if content.contains('\r') {
        content.replace("\r\n", "\n")
    } else {
        content.to_string()
    }
}

fn source_keccak(content: &str) -> String {
    format!("{:#x}", keccak256(content.as_bytes()))
}

fn source_hashes_match(actual: &str, expected: &str) -> bool {
    let actual = actual.trim().trim_start_matches("0x");
    let expected = expected.trim().trim_start_matches("0x");
    !expected.is_empty() && actual.eq_ignore_ascii_case(expected)
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
    use foundry_compilers::artifacts::MetadataSource;

    #[test]
    fn compilation_target_must_be_the_requested_source() {
        let project = tempfile::tempdir().expect("temp project");
        let root = project.path().canonicalize().expect("canonical root");
        std::fs::create_dir_all(root.join("src")).expect("src dir");
        for file in ["src/A.a.sol", "src/B.a.sol"] {
            std::fs::write(root.join(file), "").expect("source file");
        }

        check_compilation_target(
            &root,
            "Guard",
            Path::new("./src//A.a.sol"),
            Path::new("src/A.a.sol"),
        )
        .expect("same source in another spelling");
        let err = check_compilation_target(
            &root,
            "Guard",
            Path::new("src/A.a.sol"),
            Path::new("src/B.a.sol"),
        )
        .expect_err("artifact compiled from another source");
        assert!(
            matches!(
                &*err,
                PhoundryError::ArtifactTargetMismatch { requested, actual }
                    if requested == "src/A.a.sol:Guard" && actual == "src/B.a.sol:Guard"
            ),
            "{err}"
        );
    }

    fn metadata_source(keccak: &str, snapshot: Option<&str>) -> MetadataSource {
        MetadataSource {
            keccak256: keccak.to_string(),
            urls: Vec::new(),
            content: snapshot.map(str::to_string),
            license: None,
        }
    }

    fn sources_of(entries: Vec<(&str, MetadataSource)>) -> MetadataSources {
        MetadataSources {
            inner: entries
                .into_iter()
                .map(|(path, source)| (path.to_string(), source))
                .collect(),
        }
    }

    #[test]
    fn flatten_requires_the_bytes_the_artifact_hashed() {
        let project = tempfile::tempdir().expect("temp project");
        let root = project.path().canonicalize().expect("canonical root");
        let file = root.join("src/Guard.a.sol");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("src dir");
        let source =
            "contract Guard { function f() public pure returns (uint256) { return 999; } }\n";
        std::fs::write(&file, source).expect("source");

        let matching = sources_of(vec![(
            "src/Guard.a.sol",
            metadata_source(&source_keccak(source), None),
        )]);
        verify_compiled_sources(&root, &matching, &[], &[]).expect("hashed bytes match");

        let crlf = source.replace('\n', "\r\n");
        std::fs::write(&file, &crlf).expect("crlf source");
        verify_compiled_sources(&root, &matching, &[], &[])
            .expect("CRLF is the same source solc hashed");

        std::fs::write(&file, source.replace("999", "1")).expect("changed source");
        let err = verify_compiled_sources(&root, &matching, &[], &[]).expect_err("changed source");
        assert!(
            matches!(
                &*err,
                PhoundryError::SourceHashMismatch { path } if path == "src/Guard.a.sol"
            ),
            "{err}"
        );

        std::fs::write(&file, source).expect("restore source");
        let snapshot = sources_of(vec![(
            "src/Guard.a.sol",
            metadata_source(&source_keccak(source), Some("contract Other {}\n")),
        )]);
        let err = verify_compiled_sources(&root, &snapshot, &[], &[])
            .expect_err("disk left the snapshot");
        assert!(
            matches!(&*err, PhoundryError::SourceHashMismatch { .. }),
            "{err}"
        );

        std::fs::remove_file(&file).expect("remove source");
        let err = verify_compiled_sources(&root, &matching, &[], &[]).expect_err("missing source");
        assert!(
            matches!(&*err, PhoundryError::SourceHashMismatch { .. }),
            "{err}"
        );

        let empty = MetadataSources {
            inner: BTreeMap::new(),
        };
        let err = verify_compiled_sources(&root, &empty, &[], &[]).expect_err("no hashes");
        assert!(
            matches!(&*err, PhoundryError::InvalidForgeOutput(_)),
            "{err}"
        );
    }

    #[test]
    fn include_path_crlf_keeps_callback_bytes() {
        let project = tempfile::tempdir().expect("temp project");
        let root = project.path().canonicalize().expect("canonical root");
        let include = root.join("lib/original");
        std::fs::create_dir_all(&include).expect("include dir");
        let source = "pragma solidity 0.8.28;\r\nlibrary Lib {\r\n    function value() internal pure returns (uint256) {\r\n        return 999;\r\n    }\r\n}\r\n";
        let file = include.join("Lib.sol");
        std::fs::write(&file, source).expect("crlf lib");

        let matching = sources_of(vec![(
            "Lib.sol",
            metadata_source(&source_keccak(source), Some(source)),
        )]);
        verify_compiled_sources(&root, &matching, &[include.clone()], &[])
            .expect("raw CRLF is what solc hashed");

        let lf = source.replace("\r\n", "\n");
        std::fs::write(&file, &lf).expect("lf lib");
        let err = verify_compiled_sources(&root, &matching, &[include.clone()], &[])
            .expect_err("LF is not the callback bytes");
        assert!(
            matches!(&*err, PhoundryError::SourceHashMismatch { .. }),
            "{err}"
        );

        let changed = source.replace("999", "1");
        std::fs::write(&file, &changed).expect("changed lib");
        let err = verify_compiled_sources(&root, &matching, &[include], &[])
            .expect_err("changed callback source");
        assert!(
            matches!(&*err, PhoundryError::SourceHashMismatch { .. }),
            "{err}"
        );
    }

    #[test]
    fn configured_path_flattens_the_artifact_that_was_compiled() {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../cli/tests/fixtures/duplicate-guard");
        let project = tempfile::tempdir().expect("temp project");
        for file in [
            "foundry.toml",
            "assertions/src/AWeak.a.sol",
            "assertions/src/ZStrong.a.sol",
        ] {
            let target = project.path().join(file);
            std::fs::create_dir_all(target.parent().expect("fixture parent")).expect("fixture dir");
            std::fs::copy(fixture.join(file), &target).expect("copy fixture");
        }

        let built = BuildAndFlattenArgs {
            root: Some(project.path().to_path_buf()),
            assertion_contract: "./assertions/src//ZStrong.a.sol:Guard".to_string(),
            contracts: PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR),
        }
        .run()
        .expect("alias path builds the hashed source");
        assert!(
            built.flattened_source.contains("return 999"),
            "{}",
            built.flattened_source
        );
        assert!(
            !built.flattened_source.contains("return 1;"),
            "{}",
            built.flattened_source
        );

        let missed = BuildAndFlattenArgs {
            root: Some(project.path().to_path_buf()),
            assertion_contract: "ZStrong.a.sol:Guard".to_string(),
            contracts: PathBuf::from(DEFAULT_ASSERTION_CONTRACTS_DIR),
        }
        .run()
        .expect_err("a filename without its directory is not the artifact key");
        let message = missed.to_string();
        assert!(
            matches!(&*missed, PhoundryError::ContractNotAtPath { .. }),
            "{message}"
        );
        assert!(
            message.contains("assertions/src/AWeak.a.sol")
                && message.contains("assertions/src/ZStrong.a.sol"),
            "{message}"
        );
    }

    #[test]
    fn changed_remapping_flattens_the_compiled_import() {
        let project = tempfile::tempdir().expect("temp project");
        let root = project.path();
        let original_lib = root.join("lib/original/Lib.sol");
        let swapped_lib = root.join("lib/swapped/Lib.sol");
        for path in ["src", "lib/original", "lib/swapped"] {
            std::fs::create_dir_all(root.join(path)).expect("project dir");
        }
        std::fs::write(
            root.join("foundry.toml"),
            "[profile.default]\nsrc = \"src\"\nout = \"out\"\nlibs = [\"lib\"]\nsolc = \"0.8.28\"\nevm_version = \"cancun\"\noptimizer = false\nauto_detect_remappings = false\nremappings = [\"dep/=lib/original/\"]\n",
        )
        .expect("foundry.toml");
        std::fs::write(
            root.join("src/Guard.sol"),
            "pragma solidity 0.8.28; import \"dep/Lib.sol\"; contract Guard { function policy() external pure returns (uint256) { return Lib.value(); } }\n",
        )
        .expect("guard");
        let lib_source = |marker: &str| {
            format!(
                "pragma solidity 0.8.28; library Lib {{ function value() internal pure returns (uint256) {{ return {marker}; }} }}\n"
            )
        };
        std::fs::write(&original_lib, lib_source("999")).expect("original lib");
        std::fs::write(&swapped_lib, lib_source("1")).expect("swapped lib");
        let original_bytes = std::fs::read(&original_lib).expect("read original");
        let swapped_bytes = std::fs::read(&swapped_lib).expect("read swapped");

        let args = BuildAndFlattenArgs {
            root: Some(root.to_path_buf()),
            assertion_contract: "src/Guard.sol:Guard".to_string(),
            contracts: PathBuf::from("src"),
        };
        let (build, config) = args.build().expect("compile");
        let project_root = args.project_root().expect("root");
        let info = ContractInfo::new(&args.assertion_contract);
        let contract_path =
            select_contract_path(&build, &info.name, info.path.as_deref(), &project_root)
                .expect("artifact path");
        let artifact = build
            .find(contract_path.as_path(), &info.name)
            .expect("artifact");
        let metadata = artifact.metadata.clone().expect("metadata");
        let settings = serde_json::to_value(&metadata.settings).expect("settings");
        let rel_source_path = metadata
            .settings
            .compilation_target
            .iter()
            .find_map(|(path, name)| (name == &info.name).then_some(path.clone()))
            .expect("compilation target");
        let source_path = project_root.join(&rel_source_path);

        let toml_path = root.join("foundry.toml");
        let toml = std::fs::read_to_string(&toml_path).expect("read toml");
        let swapped_toml = toml.replace("dep/=lib/original/", "dep/=lib/swapped/");
        assert_ne!(toml, swapped_toml);
        std::fs::write(&toml_path, swapped_toml).expect("rewrite remapping");

        let flattened = BuildAndFlattenArgs::flatten_verified(
            &project_root,
            &metadata.sources,
            &source_path,
            &settings,
            &config,
        )
        .expect("flatten the compiled import");
        assert!(flattened.contains("return 999"), "{flattened}");
        assert!(!flattened.contains("return 1;"), "{flattened}");
        assert_eq!(
            std::fs::read(&original_lib).expect("original intact"),
            original_bytes
        );
        assert_eq!(
            std::fs::read(&swapped_lib).expect("swapped intact"),
            swapped_bytes
        );
    }

    #[test]
    fn changed_include_path_flattens_the_compiled_import() {
        let project = tempfile::tempdir().expect("temp project");
        let root = project.path();
        let original_lib = root.join("lib/original/Lib.sol");
        let swapped_lib = root.join("lib/swapped/Lib.sol");
        for path in ["src", "lib/original", "lib/swapped"] {
            std::fs::create_dir_all(root.join(path)).expect("project dir");
        }
        std::fs::write(
            root.join("foundry.toml"),
            "[profile.default]\nsrc = \"src\"\nout = \"out\"\nlibs = []\nsolc = \"0.8.28\"\nevm_version = \"cancun\"\noptimizer = false\nauto_detect_remappings = false\ninclude_paths = [\"lib/original\"]\nremappings = []\n",
        )
        .expect("foundry.toml");
        std::fs::write(
            root.join("src/Guard.sol"),
            "pragma solidity 0.8.28; import \"Lib.sol\"; contract Guard { function policy() external pure returns (uint256) { return Lib.value(); } }\n",
        )
        .expect("guard");
        let lib_source = |marker: &str| {
            format!(
                "pragma solidity 0.8.28;\r\nlibrary Lib {{\r\n    function value() internal pure returns (uint256) {{\r\n        return {marker};\r\n    }}\r\n}}\r\n"
            )
        };
        std::fs::write(&original_lib, lib_source("999")).expect("original lib");
        std::fs::write(&swapped_lib, lib_source("1")).expect("swapped lib");
        let original_bytes = std::fs::read(&original_lib).expect("read original");
        let swapped_bytes = std::fs::read(&swapped_lib).expect("read swapped");
        assert!(
            original_bytes.windows(2).any(|pair| pair == b"\r\n"),
            "include-path library is CRLF"
        );

        let args = BuildAndFlattenArgs {
            root: Some(root.to_path_buf()),
            assertion_contract: "src/Guard.sol:Guard".to_string(),
            contracts: PathBuf::from("src"),
        };
        let (build, config) = args.build().expect("compile");
        let project_root = args.project_root().expect("root");
        let info = ContractInfo::new(&args.assertion_contract);
        let contract_path =
            select_contract_path(&build, &info.name, info.path.as_deref(), &project_root)
                .expect("artifact path");
        let artifact = build
            .find(contract_path.as_path(), &info.name)
            .expect("artifact");
        let metadata = artifact.metadata.clone().expect("metadata");
        let settings = serde_json::to_value(&metadata.settings).expect("settings");
        let rel_source_path = metadata
            .settings
            .compilation_target
            .iter()
            .find_map(|(path, name)| (name == &info.name).then_some(path.clone()))
            .expect("compilation target");
        let source_path = project_root.join(&rel_source_path);

        let toml_path = root.join("foundry.toml");
        let toml = std::fs::read_to_string(&toml_path).expect("read toml");
        let swapped_toml = toml.replace(
            "include_paths = [\"lib/original\"]",
            "include_paths = [\"lib/swapped\"]",
        );
        assert_ne!(toml, swapped_toml);
        std::fs::write(&toml_path, swapped_toml).expect("rewrite include path");

        let flattened = BuildAndFlattenArgs::flatten_verified(
            &project_root,
            &metadata.sources,
            &source_path,
            &settings,
            &config,
        )
        .expect("flatten the compiled import");
        assert!(flattened.contains("return 999"), "{flattened}");
        assert!(!flattened.contains("return 1;"), "{flattened}");
        assert_eq!(
            std::fs::read(&original_lib).expect("original intact"),
            original_bytes
        );
        assert_eq!(
            std::fs::read(&swapped_lib).expect("swapped intact"),
            swapped_bytes
        );

        std::fs::write(&original_lib, lib_source("2")).expect("change compiled lib");
        let err = BuildAndFlattenArgs::flatten_verified(
            &project_root,
            &metadata.sources,
            &source_path,
            &settings,
            &config,
        )
        .expect_err("a changed include-path library is not the compiled source");
        assert!(
            matches!(&*err, PhoundryError::SourceHashMismatch { .. }),
            "{err}"
        );
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
}
