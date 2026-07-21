//! End-to-end deployment orchestrator: `pcl deploy`.
//!
//! Takes a foundry project with assertions (credible.toml) and drives the
//! entire Credible Layer onboarding flow that the dapp performs in the
//! browser: resolve-or-create the project, prove protocol-manager control
//! with a wallet signature, build and create a release, wait for the
//! deploy-gating checks, broadcast `StateOracle.batch` on-chain, and confirm
//! the deployment with the platform.
//!
//! Every step observes current state before acting, so re-running after any
//! failure resumes instead of duplicating work.

use crate::{
    api::{
        ApiArgs,
        BroadcastArgs,
        HttpMethod,
    },
    apply::{
        ApplyArgs,
        canonicalize_root,
        confirm_apply,
    },
    config::CliConfig,
    credible_config::CredibleToml,
    error::DeployError,
    onchain::TxArgs,
    output::{
        OutputStream,
        ok_envelope,
        print_envelope,
    },
    wallet::WalletArgs,
};
use alloy_primitives::Address;
use clap::ValueHint;
use colored::Colorize;
use pcl_common::args::{
    CliArgs,
    OutputMode,
};
use serde_json::{
    Value,
    json,
};
use std::{
    collections::BTreeSet,
    io::Write as _,
    path::{
        Path,
        PathBuf,
    },
    time::Duration,
};
use url::Url;
use uuid::Uuid;

const CHECK_POLL_INITIAL: Duration = Duration::from_secs(2);
const CHECK_POLL_MAX: Duration = Duration::from_secs(10);
const RELEASE_PAGE_SIZE: usize = 100;

#[derive(clap::Parser, Debug)]
#[command(
    name = "deploy",
    about = "End-to-end deploy: create/resolve the project, set the protocol manager, create a release, activate it on-chain, and confirm",
    after_help = "Examples:\n  pcl deploy --private-key $KEY --rpc-url https://... --yes\n  pcl deploy --project-name my-protocol --chain-id 84532 --private-key $KEY --yes --json\n  pcl deploy --dry-run"
)]
pub struct DeployArgs {
    #[arg(
        long,
        value_hint = ValueHint::DirPath,
        default_value = ".",
        help = "Project root directory"
    )]
    pub root: PathBuf,

    #[arg(
        short = 'c',
        long = "config",
        value_hint = ValueHint::FilePath,
        default_value = "assertions/credible.toml",
        help = "Path to credible.toml, relative to root or absolute"
    )]
    pub config: PathBuf,

    #[arg(
        long = "yes",
        visible_alias = "auto-approve",
        help = "Proceed without interactive confirmations"
    )]
    pub yes: bool,

    #[arg(
        long,
        help = "Plan only: build and verify locally, report which steps would act, change nothing"
    )]
    pub dry_run: bool,

    #[arg(
        short = 'u',
        long = "api-url",
        env = "PCL_API_URL",
        value_hint = ValueHint::Url,
        default_value = crate::config::default_platform_url(),
        help = "Base URL for the platform API. Defaults to the URL remembered from the last login, then production"
    )]
    pub api_url: Url,

    #[command(flatten)]
    pub wallet: WalletArgs,

    #[command(flatten)]
    pub tx: TxArgs,

    #[arg(
        long,
        help = "Project name used when credible.toml has no project_id and a project must be created (overrides project_name in credible.toml)"
    )]
    pub project_name: Option<String>,

    #[arg(
        long,
        help = "Chain ID used when creating a project (with --project-name); an existing project's chain is read from the platform"
    )]
    pub chain_id: Option<u64>,

    #[arg(long, help = "Skip the protocol-manager verification/set step")]
    pub skip_protocol_manager: bool,

    #[arg(
        long,
        default_value_t = 600,
        help = "Seconds to wait for release deploy-gating checks to pass"
    )]
    pub check_timeout_secs: u64,
}

/// What the protocol-manager step decided to do.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ManagerStep {
    Skipped,
    AlreadySet,
    NeedsSet,
    Mismatch { current: Address },
}

fn manager_step(skip: bool, current: Option<Address>, wallet: Address) -> ManagerStep {
    if skip {
        return ManagerStep::Skipped;
    }
    match current {
        None => ManagerStep::NeedsSet,
        Some(current) if current == wallet => ManagerStep::AlreadySet,
        Some(current) => ManagerStep::Mismatch { current },
    }
}

fn project_manager_address(project: &Value) -> Result<Option<Address>, DeployError> {
    match project.get("protocol_manager_address") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(address)) => {
            address.parse::<Address>().map(Some).map_err(|error| {
                DeployError::UnexpectedResponse {
                    endpoint: "/projects/{project_id}",
                    reason: format!("invalid protocol_manager_address {address:?}: {error}"),
                }
            })
        }
        Some(_) => {
            Err(DeployError::UnexpectedResponse {
                endpoint: "/projects/{project_id}",
                reason: "protocol_manager_address must be a string or null".to_string(),
            })
        }
    }
}

/// What the release step decided to do, given the preview diff and the
/// project's existing releases.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReleaseStep {
    Create,
    Resume {
        release_id: String,
        release_number: Option<u64>,
    },
    UpToDate,
}

/// Minimal view of a release list item used for the resume decision.
#[derive(Debug, Clone)]
struct ReleaseSummary {
    id: String,
    release_number: Option<u64>,
    environment: String,
    status: String,
}

fn release_summaries(body: &Value) -> Result<Vec<ReleaseSummary>, DeployError> {
    let items = body.as_array().ok_or_else(|| {
        DeployError::UnexpectedResponse {
            endpoint: "/projects/{project_id}/releases",
            reason: "expected a JSON array".to_string(),
        }
    })?;

    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let required_string = |field| {
                item.get(field)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
                    .ok_or_else(|| {
                        DeployError::UnexpectedResponse {
                            endpoint: "/projects/{project_id}/releases",
                            reason: format!("release at index {index} has no valid {field:?}"),
                        }
                    })
            };
            let release_number = match item.get("releaseNumber") {
                None | Some(Value::Null) => None,
                Some(value) => {
                    Some(value.as_u64().ok_or_else(|| {
                        DeployError::UnexpectedResponse {
                            endpoint: "/projects/{project_id}/releases",
                            reason: format!(
                                "release at index {index} has a non-integer \"releaseNumber\""
                            ),
                        }
                    })?)
                }
            };
            Ok(ReleaseSummary {
                id: required_string("id")?,
                release_number,
                environment: required_string("environment")?,
                status: required_string("status")?,
            })
        })
        .collect()
}

async fn inactive_release_summaries(
    api: &ApiArgs,
    config: &mut CliConfig,
    cli_args: &CliArgs,
    project_ref: &str,
    environment: &str,
) -> Result<Vec<ReleaseSummary>, DeployError> {
    let mut summaries = Vec::new();
    let mut seen_ids = BTreeSet::new();
    let mut offset = 0_usize;
    let limit = RELEASE_PAGE_SIZE.to_string();

    loop {
        let offset_value = offset.to_string();
        let body = api
            .workflow_get_json_with_query(
                config,
                cli_args,
                "get_projects_project_id_releases",
                &[("project_id", project_ref)],
                &[
                    ("environment", environment),
                    ("status", "inactive"),
                    ("limit", limit.as_str()),
                    ("offset", offset_value.as_str()),
                ],
            )
            .await?;
        let page = release_summaries(&body)?;
        let page_len = page.len();
        if page_len > RELEASE_PAGE_SIZE {
            return Err(DeployError::UnexpectedResponse {
                endpoint: "/projects/{project_id}/releases",
                reason: format!(
                    "page at offset {offset} returned {page_len} releases, exceeding the requested limit {RELEASE_PAGE_SIZE}"
                ),
            });
        }
        for release in page {
            if release.environment != environment || release.status != "inactive" {
                return Err(DeployError::UnexpectedResponse {
                    endpoint: "/projects/{project_id}/releases",
                    reason: format!(
                        "filtered release page returned id {} with environment {:?} and status {:?}",
                        release.id, release.environment, release.status
                    ),
                });
            }
            if !seen_ids.insert(release.id.clone()) {
                return Err(DeployError::UnexpectedResponse {
                    endpoint: "/projects/{project_id}/releases",
                    reason: format!(
                        "release id {} appeared more than once while paginating",
                        release.id
                    ),
                });
            }
            summaries.push(release);
        }
        if page_len < RELEASE_PAGE_SIZE {
            return Ok(summaries);
        }
        offset = offset.checked_add(page_len).ok_or_else(|| {
            DeployError::UnexpectedResponse {
                endpoint: "/projects/{project_id}/releases",
                reason: "release pagination offset overflowed".to_string(),
            }
        })?;
    }
}

/// Outcome of scanning *every* inactive release for the environment against
/// the current preview snapshot. The preview diffs against the *active*
/// release only, so an inactive release left by an interrupted earlier run is
/// invisible to it; and the newest inactive release is not necessarily the one
/// that matches (a later attempt can leave a different leftover). So each
/// inactive release's stored snapshot is compared against the payload
/// individually ([`snapshot_matches_preview`]), and the results are classified
/// here.
enum ResumeScan {
    /// No inactive release matched — defer to the preview diff.
    None,
    /// Exactly one inactive release matched — resume it.
    One(ReleaseSummary),
    /// More than one matched; pcl refuses to guess which interrupted run is
    /// ours. Carries the ambiguous release ids for the error.
    Ambiguous(Vec<String>),
}

/// Classifies the inactive releases whose snapshot matched the preview.
fn scan_inactive_matches(mut matched: Vec<ReleaseSummary>) -> ResumeScan {
    match matched.len() {
        0 => ResumeScan::None,
        1 => ResumeScan::One(matched.remove(0)),
        _ => ResumeScan::Ambiguous(matched.iter().map(|r| r.id.clone()).collect()),
    }
}

/// Resume decision. An inactive release is resumed only when exactly one of
/// them snapshot-matched the payload (resolved upstream by
/// [`scan_inactive_matches`]); otherwise the preview diff decides between
/// creating a fresh release and doing nothing.
fn release_step(has_changes: bool, resume: Option<&ReleaseSummary>) -> ReleaseStep {
    match resume {
        Some(candidate) => {
            ReleaseStep::Resume {
                release_id: candidate.id.clone(),
                release_number: candidate.release_number,
            }
        }
        None if has_changes => ReleaseStep::Create,
        // No matching leftover and no diff: the active release already matches
        // the config.
        None => ReleaseStep::UpToDate,
    }
}

/// One assertion's canonical identity within a release:
/// (file, constructor args, assertion id).
type AssertionKey = (String, Vec<String>, String);

/// One contract's canonical identity within a release:
/// (label, address, display name, assertions).
type ContractKey = (String, String, String, Vec<AssertionKey>);

fn strict_string_array(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(ToString::to_string))
        .collect()
}

/// The [`ContractKey`] set describing an inactive release's canonical
/// contents. Release snapshots contain the assertion ids assigned after the
/// platform has compiled and stored the assertions in DA.
fn snapshot_contract_set(contracts: &Value) -> Option<Vec<ContractKey>> {
    let mut set = Vec::new();
    for (label, contract) in contracts.as_object()? {
        let address = contract.get("address")?.as_str()?.to_ascii_lowercase();
        let name = contract.get("name")?.as_str()?.to_string();
        let mut assertions = Vec::new();
        for assertion in contract.get("assertions")?.as_array()? {
            assertions.push((
                assertion.get("file")?.as_str()?.to_string(),
                strict_string_array(assertion.get("args")?)?,
                assertion.get("assertionId")?.as_str()?.to_ascii_lowercase(),
            ));
        }
        assertions.sort();
        set.push((label.clone(), address, name, assertions));
    }
    set.sort();
    Some(set)
}

/// The desired canonical contract set exposed by release preview. Removed
/// contracts and assertions are excluded; added, modified, and unchanged
/// entries together describe the release that the current payload would
/// create. Assertion ids come from the platform's canonical DA compilation,
/// so they remain comparable to a stored snapshot even when the local compiler
/// bytecode is normalized by the platform during release creation.
fn preview_contract_set(preview: &Value) -> Option<Vec<ContractKey>> {
    let mut set = Vec::new();
    for (label, contract) in preview.get("diff")?.get("contracts")?.as_object()? {
        match contract.get("changeType")?.as_str()? {
            "removed" => continue,
            "added" | "modified" | "unchanged" => {}
            _ => return None,
        }
        let address = contract.get("address")?.as_str()?.to_ascii_lowercase();
        let name = contract.get("name")?.as_str()?.to_string();
        let mut assertions = Vec::new();
        for assertion in contract.get("assertions")?.as_array()? {
            match assertion.get("changeType")?.as_str()? {
                "removed" => continue,
                "added" | "modified" | "unchanged" => {}
                _ => return None,
            }
            assertions.push((
                assertion.get("file")?.as_str()?.to_string(),
                strict_string_array(assertion.get("args")?)?,
                assertion.get("assertionId")?.as_str()?.to_ascii_lowercase(),
            ));
        }
        assertions.sort();
        set.push((label.clone(), address, name, assertions));
    }
    set.sort();
    Some(set)
}

/// Whether an existing inactive release contains exactly the canonical
/// contract state produced by the current preview. The preview endpoint diffs
/// against the *active* release only, so without this check a rerun before
/// activation would create a duplicate release instead of resuming.
fn snapshot_matches_preview(release_detail: &Value, preview: &Value) -> bool {
    let snapshot = release_detail
        .get("configSnapshot")
        .and_then(|snapshot| snapshot.get("contracts"))
        .and_then(snapshot_contract_set);
    let wanted = preview_contract_set(preview);
    match (snapshot, wanted) {
        (Some(snapshot), Some(wanted)) => snapshot == wanted,
        _ => false,
    }
}

/// Deploy-gating verdict derived from a release detail response.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CheckVerdict {
    Passed,
    /// The platform explicitly reported that no check rows exist
    /// (e.g. local/dev platforms without a check engine).
    NoChecks,
    Pending(String),
    Failed(String),
}

fn check_verdict(release_detail: &Value) -> CheckVerdict {
    let Some(status) = release_detail
        .get("checkSummary")
        .and_then(|summary| summary.get("deployBlockingStatus"))
        .and_then(Value::as_str)
    else {
        // Right after release creation the summary may not have materialized
        // yet. Fail closed: only an explicit `no_checks` bypasses the deploy
        // gates, an absent/malformed summary keeps polling until it appears
        // (or the check timeout errors out).
        return CheckVerdict::Pending("check_summary_missing".to_string());
    };
    match status {
        "all_passed" => CheckVerdict::Passed,
        "no_checks" => CheckVerdict::NoChecks,
        "has_failed" | "all_cancelled" => CheckVerdict::Failed(status.to_string()),
        other => CheckVerdict::Pending(other.to_string()),
    }
}

/// Inserts or replaces the top-level `project_id` key in a credible.toml
/// document while leaving everything else byte-for-byte intact.
fn upsert_project_id(contents: &str, project_id: Uuid) -> String {
    let line = format!("project_id = \"{project_id}\"");
    let mut replaced = false;
    let mut output: Vec<String> = contents
        .lines()
        .map(|existing| {
            if !replaced && existing.trim_start().starts_with("project_id") {
                replaced = true;
                line.clone()
            } else {
                existing.to_string()
            }
        })
        .collect();
    if !replaced {
        // Top-level keys must precede any [table] section; the file starts
        // with top-level keys (environment, ...), so prepending is safe.
        output.insert(0, line);
    }
    let mut result = output.join("\n");
    if contents.ends_with('\n') || !replaced {
        result.push('\n');
    }
    result
}

/// A create intent persisted *before* the project-create POST.
///
/// If the POST's outcome is ambiguous — response lost, malformed, or a
/// post-commit server error — no project id reaches credible.toml, but the
/// intent survives. The next run then reconciles against the platform (adopt
/// the already-created project) instead of blindly posting a second create:
/// the platform permits duplicate project names, so a blind retry would
/// orphan real projects.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct CreateIntent {
    project_name: String,
    chain_id: u64,
    platform_url: String,
}

/// The intent lives next to the credible.toml whose `project_id` it guards
/// (e.g. `.credible.toml.create-intent.json`), so it shares the writability
/// preflight and travels with the project checkout.
fn create_intent_path(config_path: &Path) -> PathBuf {
    let file_name = config_path.file_name().map_or_else(
        || "credible.toml".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    config_path.with_file_name(format!(".{file_name}.create-intent.json"))
}

/// Loads a surviving create intent. A corrupt intent is an error, not a
/// silent create: it still signals that an earlier create's outcome was
/// never recorded.
fn load_create_intent(path: &Path) -> Result<Option<CreateIntent>, DeployError> {
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DeployError::CreateIntentUnreadable {
                path: path.display().to_string(),
                reason: error.to_string(),
            });
        }
    };
    serde_json::from_str(&contents).map(Some).map_err(|error| {
        DeployError::CreateIntentUnreadable {
            path: path.display().to_string(),
            reason: error.to_string(),
        }
    })
}

fn write_create_intent(path: &Path, intent: &CreateIntent) -> Result<(), DeployError> {
    let contents = serde_json::to_string_pretty(intent).map_err(|error| {
        DeployError::CreateIntentWrite {
            path: path.display().to_string(),
            reason: error.to_string(),
        }
    })?;
    // load_create_intent() is called before this write, so an existing path
    // means another process (or a symlink swap) won the race. Never truncate
    // or follow it: the marker protects against duplicate remote creates.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            DeployError::CreateIntentWrite {
                path: path.display().to_string(),
                reason: error.to_string(),
            }
        })?;
    file.write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            DeployError::CreateIntentWrite {
                path: path.display().to_string(),
                reason: error.to_string(),
            }
        })
}

/// Best-effort removal once the create outcome is recorded (or definitely
/// did not happen). A leftover intent only costs one reconcile pass on the
/// next run, so removal failures are not fatal.
fn clear_create_intent(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// IDs of the user's projects matching a create intent, by exact name and
/// chain. An item without readable chain info still matches on name alone —
/// counting it as a candidate errs toward "do not create a duplicate", and a
/// wrongly adopted project is still caught by [`resolve_chain_id`] before
/// anything is mutated.
fn matching_project_ids(projects: &Value, name: &str, chain_id: u64) -> Vec<String> {
    // `/views/projects/home` nests the caller's own projects at
    // `data.member_projects`; only those count as evidence that an earlier
    // create landed (`saved_projects` are other people's projects). Simpler
    // endpoints return a bare array or wrap it in projects/data/items.
    let items = projects
        .as_array()
        .or_else(|| {
            projects
                .get("data")
                .unwrap_or(projects)
                .get("member_projects")
                .and_then(Value::as_array)
        })
        .or_else(|| {
            ["projects", "data", "items"]
                .iter()
                .find_map(|key| projects.get(key).and_then(Value::as_array))
        })
        .map(Vec::as_slice)
        .unwrap_or_default();
    items
        .iter()
        .filter(|item| {
            ["project_name", "projectName", "name"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str))
                .is_some_and(|candidate| candidate == name)
        })
        .filter(|item| project_chain_id(item).is_none_or(|candidate| candidate == chain_id))
        .filter_map(|item| {
            ["id", "project_id", "projectId"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str))
                .map(ToString::to_string)
        })
        .collect()
}

/// Decides whether a persisted create intent can be safely adopted from the
/// project index. An empty index is *not* evidence that the earlier POST did
/// not land: the response may have been lost and the index may still be
/// delayed. Keep the intent fail-closed until the project can be adopted or
/// the user explicitly resolves it.
fn resolve_create_intent_match(
    ids: &[String],
    intent: &CreateIntent,
    intent_path: &Path,
) -> Result<Uuid, DeployError> {
    match ids {
        [] => {
            Err(DeployError::PendingProjectCreate {
                name: intent.project_name.clone(),
                chain_id: intent.chain_id,
                path: intent_path.display().to_string(),
            })
        }
        [id] => {
            Uuid::parse_str(id).map_err(|_| {
                DeployError::UnexpectedResponse {
                    endpoint: "/views/projects/home",
                    reason: format!("project id {id} is not a UUID"),
                }
            })
        }
        _ => {
            Err(DeployError::AmbiguousProjectCreate {
                name: intent.project_name.clone(),
                chain_id: intent.chain_id,
                count: ids.len(),
                path: intent_path.display().to_string(),
            })
        }
    }
}

/// Confirms an adopted project actually belongs to the pending create before
/// any local recovery state is written. [`matching_project_ids`] can only
/// match on name when the project index reports chain *names* rather than ids
/// (as `/views/projects/home` does), so a same-name project on a different
/// chain can slip through. Comparing the authoritative project record's chain
/// against the intent closes that gap: a mismatch is rejected while the intent
/// is still in place, instead of recording the wrong project id and clearing
/// the intent (which a later run would then treat as the configured project
/// and deploy to).
fn validate_adopted_project_chain(
    project: &Value,
    project_id: Uuid,
    intent: &CreateIntent,
    intent_path: &Path,
) -> Result<(), DeployError> {
    let found = project_chain_id(project).ok_or(DeployError::UnexpectedResponse {
        endpoint: "/projects/{project_id}",
        reason: "adopted project has no chain_id/project_networks; cannot confirm it matches the pending create"
            .to_string(),
    })?;
    if found != intent.chain_id {
        return Err(DeployError::AdoptedProjectChainMismatch {
            name: intent.project_name.clone(),
            project_id,
            intent_chain_id: intent.chain_id,
            found_chain_id: found,
            path: intent_path.display().to_string(),
        });
    }
    Ok(())
}

/// Verifies credible.toml can be read and written *before* the project is
/// created remotely, so a read-only file or missing path fails while
/// cancellation is still side-effect free.
fn preflight_project_id_write(config_path: &Path) -> Result<(), DeployError> {
    let toml_write_back = |reason: String| {
        DeployError::TomlWriteBack {
            path: config_path.display().to_string(),
            reason,
        }
    };
    std::fs::read_to_string(config_path)
        .map_err(|e| toml_write_back(format!("not readable: {e}")))?;
    // Opening for append verifies writability without touching the contents.
    std::fs::OpenOptions::new()
        .append(true)
        .open(config_path)
        .map(drop)
        .map_err(|e| toml_write_back(format!("not writable: {e}")))
}

/// Writes the updated credible.toml atomically (temp file + rename in the
/// same directory) so an interrupted write can never truncate the config.
fn write_project_id(config_path: &Path, project_id: Uuid) -> Result<(), DeployError> {
    let toml_write_back = |reason: String| {
        DeployError::TomlWriteBack {
            path: config_path.display().to_string(),
            reason,
        }
    };
    let contents =
        std::fs::read_to_string(config_path).map_err(|e| toml_write_back(e.to_string()))?;
    let updated = upsert_project_id(&contents, project_id);
    let parent = config_path.parent().unwrap_or(Path::new("."));
    // Create the temp file with a random name via O_EXCL (`tempfile` does
    // both): a predictable `.tmp` path lets a hostile project directory
    // pre-plant a symlink there, so a plain `fs::write` would follow it and
    // clobber another user-writable file before the rename. Building it in the
    // config's own directory keeps the rename atomic.
    let mut temp = tempfile::Builder::new()
        .prefix(".credible.toml.")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| toml_write_back(e.to_string()))?;
    temp.write_all(updated.as_bytes())
        .map_err(|e| toml_write_back(e.to_string()))?;
    temp.persist(config_path)
        .map(drop)
        .map_err(|e| toml_write_back(e.error.to_string()))
}

/// Chain id the deploy binds to (manager challenge, RPC selection).
/// `--chain-id` is authoritative only for a project created this run, where
/// it was the create input; an existing project's chain comes from the
/// platform record, and a contradicting flag is an error rather than an
/// override.
fn resolve_chain_id(
    flag: Option<u64>,
    created_chain_id: Option<u64>,
    project: &Value,
) -> Result<u64, DeployError> {
    if let Some(chain_id) = created_chain_id {
        return Ok(chain_id);
    }
    let recorded = project_chain_id(project).ok_or(DeployError::UnexpectedResponse {
        endpoint: "/projects/{project_id}",
        reason: "missing chain_id/project_networks".to_string(),
    })?;
    if let Some(flag) = flag
        && flag != recorded
    {
        return Err(DeployError::ChainIdMismatch {
            flag,
            project: recorded,
        });
    }
    Ok(recorded)
}

/// Extracts the chain id from a project response. The live API exposes it as
/// `project_networks: ["8453"]` (strings); older/other shapes may use
/// `chain_id`.
pub(crate) fn project_chain_id(project: &Value) -> Option<u64> {
    project
        .get("chain_id")
        .or_else(|| project.get("chainId"))
        .and_then(Value::as_u64)
        .or_else(|| {
            project
                .get("project_networks")
                .and_then(Value::as_array)
                .and_then(|networks| networks.first())
                .and_then(|network| {
                    network
                        .as_str()
                        .and_then(|value| value.parse().ok())
                        .or_else(|| network.as_u64())
                })
        })
}

fn progress(human: bool, message: &str) {
    if human {
        eprintln!("{} {message}", "→".cyan());
    }
}

#[cfg(feature = "credible")]
fn insert_field(data: &mut Value, key: &str, value: Value) {
    if let Some(object) = data.as_object_mut() {
        object.insert(key.to_string(), value);
    }
}

impl DeployArgs {
    #[allow(clippy::too_many_lines)]
    pub async fn run(&self, cli_args: &CliArgs, config: &mut CliConfig) -> Result<(), DeployError> {
        let output_mode = cli_args.output_mode();
        let human = output_mode == OutputMode::Human;
        if !human && !self.yes && !self.dry_run {
            return Err(DeployError::MachineYesRequired);
        }

        let root = canonicalize_root(&self.root)?;
        let config_path = root.join(&self.config);
        let mut credible =
            CredibleToml::from_path(&config_path).map_err(crate::error::ApplyError::from)?;

        // Resolve the wallet up-front so a misconfigured signer fails before
        // any mutation. In --dry-run an unconfigured wallet is tolerated.
        let signer = if self.dry_run && !self.wallet.is_configured() {
            None
        } else {
            Some(self.wallet.signer(human).await?)
        };

        if self.dry_run {
            // Plan only: build and verify locally, touch nothing remote.
            let plan = Self::dry_run_plan(&credible, &root, output_mode, signer.as_ref())?;
            return Self::finish_dry_run(plan, output_mode);
        }
        // Past the dry-run return a signer was always resolved above.
        let signer = signer.ok_or(crate::wallet::WalletError::NoWallet)?;

        let api = ApiArgs::headless(self.api_url.clone());
        ApplyArgs::ensure_fresh_auth(config, cli_args, &self.api_url).await?;

        // ------------------------------------------------------------------
        // Step 1: resolve or create the project
        // ------------------------------------------------------------------
        let (project_id, project, created_chain_id) = if let Some(project_id) = credible.project_id
        {
            progress(human, &format!("Using project {project_id}"));
            let project_id_string = project_id.to_string();
            let project = api
                .workflow_json(
                    config,
                    cli_args,
                    HttpMethod::Get,
                    "get_projects_project_id",
                    &[("project_id", project_id_string.as_str())],
                    None,
                )
                .await?;
            (project_id, project, None)
        } else {
            let name = self
                .project_name
                .clone()
                .or_else(|| credible.project_name.clone())
                .ok_or(DeployError::MissingProjectInfo)?;
            let chain_id = self.chain_id.ok_or(DeployError::MissingProjectInfo)?;
            // The created project's id must be persisted right after the
            // POST; verify that write can succeed while nothing has been
            // mutated yet.
            preflight_project_id_write(&config_path)?;
            let intent_path = create_intent_path(&config_path);
            // A surviving intent means an earlier create POST's outcome was
            // never recorded; reconcile with the platform before creating
            // anything else — never blind-POST a second create.
            let adopted = match load_create_intent(&intent_path)? {
                Some(intent) => {
                    Some(
                        self.reconcile_create_intent(
                            &api,
                            config,
                            cli_args,
                            &intent,
                            &intent_path,
                            human,
                        )
                        .await?,
                    )
                }
                None => None,
            };
            if let Some((project_id, project)) = adopted {
                // reconcile_create_intent already fetched the project record
                // and confirmed its chain matches the intent, so recording the
                // id and clearing the intent here is safe.
                progress(
                    human,
                    &format!("Adopting project {project_id} created by an interrupted earlier run"),
                );
                write_project_id(&config_path, project_id).map_err(|error| {
                    DeployError::TomlWriteBackAfterCreate {
                        path: config_path.display().to_string(),
                        project_id,
                        reason: match error {
                            DeployError::TomlWriteBack { reason, .. } => reason,
                            other => other.to_string(),
                        },
                    }
                })?;
                clear_create_intent(&intent_path);
                credible.project_id = Some(project_id);
                // Adopted, not created: the platform record is authoritative
                // for the chain, exactly like a pre-recorded project_id.
                (project_id, project, None)
            } else {
                self.confirm_step(
                    human,
                    &format!("Create project {name:?} on chain {chain_id} on the platform?"),
                )?;
                // Persist the intent before the POST: from here on, a lost or
                // malformed create response leaves a marker that forces the
                // next run to reconcile instead of creating a duplicate (the
                // platform allows duplicate project names).
                write_create_intent(
                    &intent_path,
                    &CreateIntent {
                        project_name: name.clone(),
                        chain_id,
                        platform_url: self.api_url.as_str().trim_end_matches('/').to_string(),
                    },
                )?;
                progress(
                    human,
                    &format!("Creating project {name:?} on chain {chain_id}"),
                );
                let body = json!({ "project_name": name, "chain_id": chain_id });
                let project = match api
                    .workflow_json(
                        config,
                        cli_args,
                        HttpMethod::Post,
                        "post_projects",
                        &[],
                        Some(&body),
                    )
                    .await
                {
                    Ok(project) => project,
                    Err(error) => {
                        // A definite rejection means nothing was created and
                        // the intent can go; any other failure (network drop,
                        // 5xx, lost response) is ambiguous and the intent
                        // must survive for the next run's reconcile pass.
                        if matches!(
                            &error,
                            crate::api::ApiCommandError::HttpStatus { status, .. }
                                if (400..=499).contains(status)
                        ) {
                            clear_create_intent(&intent_path);
                        }
                        return Err(error.into());
                    }
                };
                let project_id = project
                    .get("id")
                    .or_else(|| project.get("project_id"))
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok())
                    .ok_or(DeployError::UnexpectedResponse {
                        endpoint: "/projects",
                        reason: "missing project id in create response".to_string(),
                    })?;
                // The remote project now exists; a failure here must carry
                // its id and a resume path, and the surviving intent lets the
                // next run adopt it automatically instead of creating a
                // duplicate.
                write_project_id(&config_path, project_id).map_err(|error| {
                    DeployError::TomlWriteBackAfterCreate {
                        path: config_path.display().to_string(),
                        project_id,
                        reason: match error {
                            DeployError::TomlWriteBack { reason, .. } => reason,
                            other => other.to_string(),
                        },
                    }
                })?;
                clear_create_intent(&intent_path);
                credible.project_id = Some(project_id);
                progress(
                    human,
                    &format!("Created project {project_id} and recorded it in credible.toml"),
                );
                (project_id, project, Some(chain_id))
            }
        };
        let project_created = created_chain_id.is_some();

        let project_chain_id = resolve_chain_id(self.chain_id, created_chain_id, &project)?;

        // ------------------------------------------------------------------
        // Step 2: protocol manager
        // ------------------------------------------------------------------
        let current_manager = project_manager_address(&project)?;
        let wallet_address = signer.address();

        let manager_action =
            match manager_step(self.skip_protocol_manager, current_manager, wallet_address) {
                ManagerStep::Skipped => "skipped".to_string(),
                ManagerStep::AlreadySet => {
                    progress(human, "Protocol manager already set to this wallet");
                    "already_set".to_string()
                }
                ManagerStep::Mismatch { current } => {
                    return Err(DeployError::ManagerMismatch {
                        project_id,
                        current: current.to_string(),
                        wallet: wallet_address.to_string(),
                    });
                }
                ManagerStep::NeedsSet => {
                    self.confirm_step(
                        human,
                        &format!(
                            "Set the protocol manager of project {project_id} to {wallet_address}?"
                        ),
                    )?;
                    progress(
                        human,
                        "Setting protocol manager (signed challenge, no transaction)",
                    );
                    api.protocol_manager_set_signed_flow(
                        config,
                        cli_args,
                        &project_id.to_string(),
                        project_chain_id,
                        self.broadcast_args(&signer),
                    )
                    .await?;
                    "set".to_string()
                }
            };

        // ------------------------------------------------------------------
        // Step 3: build + verify + preview + create/resume release
        // ------------------------------------------------------------------
        progress(human, "Building and verifying assertions");
        let (payload, verification_inputs) = ApplyArgs::build_payload(&credible, &root)?;
        #[cfg(feature = "credible")]
        let verification = ApplyArgs::verify_all_assertions(&verification_inputs, output_mode)?;
        #[cfg(not(feature = "credible"))]
        let _ = verification_inputs;

        let client = self.typed_client(config)?;
        progress(human, "Previewing release changes");
        let preview = ApplyArgs::call_preview(&client, &project_id, &payload).await?;

        let project_ref = project_id.to_string();
        let summaries = inactive_release_summaries(
            &api,
            config,
            cli_args,
            project_ref.as_str(),
            &credible.environment,
        )
        .await?;
        // Every inactive release for this environment is a potential resume
        // candidate, not just the newest: an interrupted earlier run can leave
        // a matching inactive release behind a later, non-matching one. The
        // preview cannot vouch for any of them (it diffs against the active
        // release), so fetch each detail and compare its stored snapshot to the
        // payload whether or not the preview reports changes.
        let preview_value = serde_json::to_value(&preview)?;
        let mut matched = Vec::new();
        for candidate in &summaries {
            let detail = api
                .workflow_json(
                    config,
                    cli_args,
                    HttpMethod::Get,
                    "get_projects_project_id_releases_release_id",
                    &[
                        ("project_id", project_ref.as_str()),
                        ("release_id", candidate.id.as_str()),
                    ],
                    None,
                )
                .await?;
            if snapshot_matches_preview(&detail, &preview_value) {
                matched.push(candidate.clone());
            }
        }
        let resume = match scan_inactive_matches(matched) {
            ResumeScan::None => None,
            ResumeScan::One(candidate) => Some(candidate),
            ResumeScan::Ambiguous(release_ids) => {
                return Err(DeployError::AmbiguousInactiveRelease {
                    project_id,
                    environment: credible.environment.clone(),
                    release_ids,
                });
            }
        };
        let step = release_step(preview.has_changes(), resume.as_ref());

        let (release_id, release_number, resumed) = match step {
            ReleaseStep::UpToDate => {
                progress(human, "Everything already released and deployed");
                let data = json!({
                    "outcome": "up_to_date",
                    "project_id": project_id,
                    "project_created": project_created,
                    "protocol_manager": { "action": manager_action, "address": wallet_address },
                    "release": Value::Null,
                    "tx": Value::Null,
                });
                #[cfg(feature = "credible")]
                let data = {
                    let mut data = data;
                    insert_field(&mut data, "verification", json!(verification));
                    data
                };
                return Self::finish(
                    output_mode,
                    data,
                    vec![
                        format!("pcl releases list {project_id}"),
                        format!("pcl projects show {project_id}"),
                    ],
                );
            }
            ReleaseStep::Resume {
                release_id,
                release_number,
            } => {
                progress(
                    human,
                    &format!("Resuming existing inactive release {release_id}"),
                );
                (release_id, release_number, true)
            }
            ReleaseStep::Create => {
                if human {
                    print!("{}", preview.render_plan());
                    if !self.yes && !confirm_apply()? {
                        return Err(DeployError::Cancelled);
                    }
                }
                progress(human, "Creating release");
                let release =
                    ApplyArgs::call_create_release(&client, &project_id, &payload).await?;
                (
                    release.id.to_string(),
                    Some(u64::from(release.release_number)),
                    false,
                )
            }
        };

        // ------------------------------------------------------------------
        // Step 4: wait for deploy-gating checks
        // ------------------------------------------------------------------
        let check_state = self
            .wait_for_checks(&api, config, cli_args, &project_ref, &release_id, human)
            .await?;

        // ------------------------------------------------------------------
        // Step 5+6: on-chain deploy (noop-aware) + platform confirmation
        // ------------------------------------------------------------------
        progress(human, "Deploying release on-chain");
        let deploy_envelope = api
            .release_deploy_broadcast_flow(
                config,
                cli_args,
                &project_ref,
                &release_id,
                self.broadcast_args(&signer),
            )
            .await?;
        let deploy_data = deploy_envelope.get("data").cloned().unwrap_or(Value::Null);

        let data = json!({
            "outcome": if resumed { "resumed_and_deployed" } else { "released_and_deployed" },
            "project_id": project_id,
            "project_created": project_created,
            "protocol_manager": { "action": manager_action, "address": wallet_address },
            "release": {
                "id": release_id,
                "release_number": release_number,
                "resumed": resumed,
            },
            "checks": check_state,
            "deploy": deploy_data,
        });
        #[cfg(feature = "credible")]
        let data = {
            let mut data = data;
            insert_field(&mut data, "verification", json!(verification));
            data
        };
        Self::finish(
            output_mode,
            data,
            vec![
                format!("pcl releases show {project_id} {release_id}"),
                format!("pcl deployments --project {project_id}"),
            ],
        )
    }

    /// Resolves a surviving create intent against the platform: returns the
    /// already-created project's id when exactly one of the user's projects
    /// matches the intent's name and chain. It fails closed when the index is
    /// empty or ambiguous: neither state proves the earlier create did not
    /// land, so a second POST would risk a duplicate project.
    async fn reconcile_create_intent(
        &self,
        api: &ApiArgs,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        intent: &CreateIntent,
        intent_path: &Path,
        human: bool,
    ) -> Result<(Uuid, Value), DeployError> {
        let requested = self.api_url.as_str().trim_end_matches('/').to_string();
        if intent.platform_url != requested {
            // The unresolved create belongs to another platform; matching
            // this platform's projects against it proves nothing.
            return Err(DeployError::CreateIntentPlatformMismatch {
                path: intent_path.display().to_string(),
                intent_platform: intent.platform_url.clone(),
                requested,
            });
        }
        progress(
            human,
            &format!(
                "An earlier create for project {:?} never recorded its outcome; checking the platform before creating",
                intent.project_name
            ),
        );
        let mine = api
            .workflow_json(
                config,
                cli_args,
                HttpMethod::Get,
                "get_views_projects_home",
                &[],
                None,
            )
            .await?;
        let ids = matching_project_ids(&mine, &intent.project_name, intent.chain_id);
        let project_id = resolve_create_intent_match(&ids, intent, intent_path)?;
        // The index only matched on name (the home view lists chain names, not
        // ids), so fetch the authoritative record and confirm its chain before
        // returning. Both this GET and the validation happen before the caller
        // touches any local recovery state, so a mismatch leaves the intent in
        // place instead of adopting a same-name project on the wrong chain.
        let project_id_string = project_id.to_string();
        let project = api
            .workflow_json(
                config,
                cli_args,
                HttpMethod::Get,
                "get_projects_project_id",
                &[("project_id", project_id_string.as_str())],
                None,
            )
            .await?;
        validate_adopted_project_chain(&project, project_id, intent, intent_path)?;
        Ok((project_id, project))
    }

    /// Interactive gate before a mutating step; cancelling is side-effect
    /// free because the prompt always precedes the mutation it guards.
    /// `--yes` skips prompting (machine output already requires `--yes`).
    fn confirm_step(&self, human: bool, prompt: &str) -> Result<(), DeployError> {
        if !human || self.yes {
            return Ok(());
        }
        let confirmed = inquire::Confirm::new(prompt)
            .with_default(false)
            .prompt()
            .map_err(|_| DeployError::Cancelled)?;
        if confirmed {
            Ok(())
        } else {
            Err(DeployError::Cancelled)
        }
    }

    /// Sub-flows receive the already-resolved signer so a keystore is
    /// decrypted (and its password prompted for) exactly once per run.
    fn broadcast_args(&self, signer: &alloy_signer_local::PrivateKeySigner) -> BroadcastArgs {
        BroadcastArgs {
            broadcast: true,
            yes: self.yes,
            wallet: crate::wallet::WalletArgs::from_signer(signer.clone()),
            tx: self.tx.clone(),
        }
    }

    fn typed_client(
        &self,
        config: &CliConfig,
    ) -> Result<dapp_api_client::generated::client::Client, DeployError> {
        crate::client::authenticated_client(config, &self.api_url)
            .map_err(crate::apply::client_error_to_apply)
            .map_err(DeployError::Apply)
    }

    async fn wait_for_checks(
        &self,
        api: &ApiArgs,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        project_ref: &str,
        release_id: &str,
        human: bool,
    ) -> Result<String, DeployError> {
        let deadline = std::time::Instant::now() + Duration::from_secs(self.check_timeout_secs);
        let mut delay = CHECK_POLL_INITIAL;
        loop {
            let detail = api
                .workflow_json(
                    config,
                    cli_args,
                    HttpMethod::Get,
                    "get_projects_project_id_releases_release_id",
                    &[("project_id", project_ref), ("release_id", release_id)],
                    None,
                )
                .await?;
            match check_verdict(&detail) {
                CheckVerdict::Passed => {
                    progress(human, "Deploy-gating checks passed");
                    return Ok("all_passed".to_string());
                }
                CheckVerdict::NoChecks => {
                    progress(
                        human,
                        "Platform reports no deploy-gating checks; continuing (dev/local platform)",
                    );
                    return Ok("no_checks".to_string());
                }
                CheckVerdict::Failed(status) => {
                    return Err(DeployError::ChecksFailed {
                        project_id: Uuid::parse_str(project_ref).unwrap_or(Uuid::nil()),
                        release_id: release_id.to_string(),
                        status,
                    });
                }
                CheckVerdict::Pending(status) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(DeployError::ChecksTimeout {
                            timeout_secs: self.check_timeout_secs,
                            status,
                        });
                    }
                    progress(human, &format!("Waiting for release checks ({status})"));
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(CHECK_POLL_MAX);
                }
            }
        }
    }

    fn dry_run_plan(
        credible: &CredibleToml,
        root: &Path,
        output_mode: OutputMode,
        signer: Option<&alloy_signer_local::PrivateKeySigner>,
    ) -> Result<Value, DeployError> {
        let (payload, verification_inputs) = ApplyArgs::build_payload(credible, root)?;
        #[cfg(feature = "credible")]
        let verification = ApplyArgs::verify_all_assertions(&verification_inputs, output_mode)?;
        #[cfg(not(feature = "credible"))]
        {
            let _ = (verification_inputs, output_mode);
        }
        let data = json!({
            "outcome": "dry_run",
            "project_id": credible.project_id,
            "would_create_project": credible.project_id.is_none(),
            "wallet_address": signer.map(|s| s.address().to_string()),
            "payload": payload,
        });
        #[cfg(feature = "credible")]
        let data = {
            let mut data = data;
            insert_field(&mut data, "verification", json!(verification));
            data
        };
        Ok(data)
    }

    fn finish_dry_run(data: Value, output_mode: OutputMode) -> Result<(), DeployError> {
        if output_mode == OutputMode::Human {
            println!(
                "Dry run complete. Built and verified the release payload; no project or release was created."
            );
            return Ok(());
        }
        let envelope = ok_envelope(data, vec!["pcl deploy --yes".to_string()]);
        print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
        Ok(())
    }

    fn finish(
        output_mode: OutputMode,
        data: Value,
        next_actions: Vec<String>,
    ) -> Result<(), DeployError> {
        if output_mode == OutputMode::Human {
            let outcome = data
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or("done");
            let release = data
                .get("release")
                .and_then(|r| r.get("id"))
                .and_then(Value::as_str);
            let tx_hash = data
                .get("deploy")
                .and_then(|d| d.get("tx"))
                .and_then(|t| t.get("tx_hash"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            println!("{} {}", "Deploy complete:".bold().green(), outcome);
            if let Some(release) = release {
                println!("  release: {release}");
            }
            if let Some(tx_hash) = tx_hash {
                println!("  tx:      {tx_hash}");
            }
            for action in &next_actions {
                println!("  next:    {action}");
            }
            return Ok(());
        }
        let envelope = ok_envelope(data, next_actions);
        print_envelope(&envelope, output_mode, OutputStream::Stdout)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use chrono::{
        TimeZone,
        Utc,
    };
    use mockito::Matcher;
    use std::collections::BTreeMap;

    const WALLET: Address = address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    const OTHER: Address = address!("0101010101010101010101010101010101010101");

    #[test]
    fn manager_step_decision_table() {
        assert_eq!(manager_step(true, None, WALLET), ManagerStep::Skipped);
        assert_eq!(
            manager_step(true, Some(OTHER), WALLET),
            ManagerStep::Skipped
        );
        assert_eq!(manager_step(false, None, WALLET), ManagerStep::NeedsSet);
        assert_eq!(
            manager_step(false, Some(WALLET), WALLET),
            ManagerStep::AlreadySet
        );
        assert_eq!(
            manager_step(false, Some(OTHER), WALLET),
            ManagerStep::Mismatch { current: OTHER }
        );
    }

    #[test]
    fn project_manager_address_fails_closed_on_malformed_values() {
        assert_eq!(project_manager_address(&json!({})).unwrap(), None);
        assert_eq!(
            project_manager_address(&json!({ "protocol_manager_address": null })).unwrap(),
            None
        );
        assert_eq!(
            project_manager_address(&json!({
                "protocol_manager_address": WALLET.to_string()
            }))
            .unwrap(),
            Some(WALLET)
        );
        for malformed in [json!("not-an-address"), json!(123)] {
            assert!(matches!(
                project_manager_address(&json!({ "protocol_manager_address": malformed })),
                Err(DeployError::UnexpectedResponse { .. })
            ));
        }
    }

    fn summaries(items: &[(&str, &str, &str)]) -> Vec<ReleaseSummary> {
        items
            .iter()
            .enumerate()
            .map(|(i, (id, environment, status))| {
                ReleaseSummary {
                    id: (*id).to_string(),
                    release_number: Some(i as u64 + 1),
                    environment: (*environment).to_string(),
                    status: (*status).to_string(),
                }
            })
            .collect()
    }

    #[test]
    fn release_step_decision_table() {
        let candidate = &summaries(&[("new", "staging", "inactive")])[0];

        // No resume candidate: diff → create, no diff → up to date.
        assert_eq!(release_step(true, None), ReleaseStep::Create);
        assert_eq!(release_step(false, None), ReleaseStep::UpToDate);

        // A matching inactive release is resumed regardless of the preview
        // diff: the preview only diffs against the active release, so it cannot
        // see a release created by an interrupted earlier run.
        for has_changes in [true, false] {
            assert_eq!(
                release_step(has_changes, Some(candidate)),
                ReleaseStep::Resume {
                    release_id: "new".to_string(),
                    release_number: Some(1),
                }
            );
        }
    }

    #[test]
    fn scan_inactive_matches_resumes_unique_and_flags_ambiguous() {
        // Nothing matched → defer to the preview diff.
        assert!(matches!(
            scan_inactive_matches(Vec::new()),
            ResumeScan::None
        ));

        // Exactly one match → resume it.
        let one = summaries(&[("r1", "staging", "inactive")]);
        match scan_inactive_matches(one) {
            ResumeScan::One(release) => assert_eq!(release.id, "r1"),
            _ => panic!("expected exactly one match"),
        }

        // More than one match → ambiguous, carrying every candidate id.
        let many = summaries(&[("r1", "staging", "inactive"), ("r2", "staging", "inactive")]);
        match scan_inactive_matches(many) {
            ResumeScan::Ambiguous(ids) => {
                assert_eq!(ids, vec!["r1".to_string(), "r2".to_string()]);
            }
            _ => panic!("expected ambiguous"),
        }
    }

    #[test]
    fn check_verdict_decision_table() {
        let detail = |status: &str| json!({ "checkSummary": { "deployBlockingStatus": status } });
        assert_eq!(check_verdict(&detail("all_passed")), CheckVerdict::Passed);
        assert_eq!(check_verdict(&detail("no_checks")), CheckVerdict::NoChecks);
        assert_eq!(
            check_verdict(&detail("has_failed")),
            CheckVerdict::Failed("has_failed".to_string())
        );
        assert_eq!(
            check_verdict(&detail("all_cancelled")),
            CheckVerdict::Failed("all_cancelled".to_string())
        );
        assert_eq!(
            check_verdict(&detail("in_progress")),
            CheckVerdict::Pending("in_progress".to_string())
        );
        // Missing/malformed checkSummary fails closed: keep polling instead
        // of skipping the deploy gates.
        assert_eq!(
            check_verdict(&json!({ "id": "x" })),
            CheckVerdict::Pending("check_summary_missing".to_string())
        );
        assert_eq!(
            check_verdict(&json!({ "checkSummary": null })),
            CheckVerdict::Pending("check_summary_missing".to_string())
        );
        assert_eq!(
            check_verdict(&json!({ "checkSummary": { "deployBlockingStatus": 3 } })),
            CheckVerdict::Pending("check_summary_missing".to_string())
        );
    }

    #[test]
    fn upsert_project_id_inserts_before_existing_content() {
        let id = Uuid::nil();
        let contents = "environment = \"staging\"\n\n[contracts.Foo]\naddress = \"0x01\"\n";
        let updated = upsert_project_id(contents, id);
        assert!(updated.starts_with(&format!("project_id = \"{id}\"\n")));
        assert!(updated.contains("environment = \"staging\""));
        assert!(updated.contains("[contracts.Foo]"));
        // Valid TOML with the id present.
        let parsed: toml::Value = toml::from_str(&updated).unwrap();
        assert_eq!(
            parsed.get("project_id").and_then(toml::Value::as_str),
            Some(id.to_string().as_str())
        );
    }

    #[test]
    fn upsert_project_id_replaces_existing_key() {
        let id = Uuid::nil();
        let contents = "environment = \"staging\"\nproject_id = \"11111111-1111-1111-1111-111111111111\"\n[contracts.Foo]\naddress = \"0x01\"\n";
        let updated = upsert_project_id(contents, id);
        assert_eq!(updated.matches("project_id").count(), 1);
        assert!(updated.contains(&format!("project_id = \"{id}\"")));
        assert!(!updated.contains("11111111"));
    }

    fn snapshot_contracts(assertion_id: &str, args: &[&str]) -> Value {
        json!({
            "mock": {
                "address": "0x0101010101010101010101010101010101010101",
                "name": "Mock",
                "assertions": [{
                    "file": "assertions/src/A.a.sol:A",
                    "args": args,
                    "bytecode": "0xplatform-normalized-bytecode",
                    "assertionId": assertion_id,
                }],
            }
        })
    }

    fn release_detail(contracts: Value) -> Value {
        let mut detail = json!({ "configSnapshot": { "contracts": null } });
        detail["configSnapshot"]["contracts"] = contracts;
        detail
    }

    fn release_preview(assertion_id: &str, args: &[&str]) -> Value {
        json!({
            "diff": {
                "contracts": {
                    "mock": {
                        "address": "0x0101010101010101010101010101010101010101",
                        "name": "Mock",
                        "changeType": "added",
                        "assertions": [{
                            "file": "assertions/src/A.a.sol:A",
                            "args": args,
                            "changeType": "added",
                            "assertionId": assertion_id,
                        }],
                    }
                }
            }
        })
    }

    #[test]
    fn snapshot_matching_uses_complete_canonical_preview_state() {
        // Same content matches even with different address casing.
        let mut upper = snapshot_contracts("0xassertion", &[]);
        upper["mock"]["address"] = json!(
            "0x0101010101010101010101010101010101010101"
                .to_ascii_uppercase()
                .replace("0X", "0x")
        );
        assert!(snapshot_matches_preview(
            &release_detail(upper),
            &release_preview("0xASSERTION", &[])
        ));

        // The preview uses the same complete desired-state shape for retained
        // contracts and assertions, regardless of their diff classification.
        for change_type in ["added", "modified", "unchanged"] {
            let mut classified = release_preview("0xassertion", &[]);
            classified["diff"]["contracts"]["mock"]["changeType"] = json!(change_type);
            classified["diff"]["contracts"]["mock"]["assertions"][0]["changeType"] =
                json!(change_type);
            assert!(snapshot_matches_preview(
                &release_detail(snapshot_contracts("0xassertion", &[])),
                &classified
            ));
        }

        // Removed entries describe the active release, not the desired
        // snapshot, so they are excluded from the comparison.
        let mut removal_preview = release_preview("0xassertion", &[]);
        removal_preview["diff"]["contracts"]["old"] = json!({
            "address": "0x0202020202020202020202020202020202020202",
            "name": "Old",
            "changeType": "removed",
            "assertions": [],
        });
        assert!(snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &removal_preview
        ));
    }

    #[test]
    fn snapshot_matching_rejects_divergent_canonical_state() {
        // A source/compiler change produces a different canonical assertion
        // id and must not resume the stale release.
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xold", &[])),
            &release_preview("0xnew", &[])
        ));

        // Different constructor args must NOT resume.
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &["1"])),
            &release_preview("0xassertion", &["2"])
        ));

        // Contract metadata is part of the desired snapshot. Resuming across
        // a label or display-name change would persist stale platform state.
        let mut renamed_label = release_preview("0xassertion", &[]);
        let renamed_contract = renamed_label["diff"]["contracts"]
            .as_object_mut()
            .unwrap()
            .remove("mock")
            .unwrap();
        renamed_label["diff"]["contracts"]["renamed"] = renamed_contract;
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &renamed_label
        ));

        let mut renamed_display = release_preview("0xassertion", &[]);
        renamed_display["diff"]["contracts"]["mock"]["name"] = json!("Renamed");
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &renamed_display
        ));
        let mut assertion_removal = release_preview("0xassertion", &[]);
        assertion_removal["diff"]["contracts"]["mock"]["assertions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "file": "assertions/src/Old.a.sol:Old",
                "args": [],
                "changeType": "removed",
                "assertionId": null,
                "previousAssertionId": "0xold",
            }));
        assert!(snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &assertion_removal
        ));
    }

    #[test]
    fn snapshot_matching_fails_closed_on_malformed_data() {
        assert!(!snapshot_matches_preview(
            &json!({ "configSnapshot": null }),
            &release_preview("0xassertion", &[])
        ));
        // Malformed server data fails closed instead of normalizing into a
        // potentially matching but incomplete identity.
        let mut malformed_args = release_preview("0xassertion", &[]);
        malformed_args["diff"]["contracts"]["mock"]["assertions"][0]["args"] = json!([1]);
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &malformed_args
        ));

        let mut malformed_change = release_preview("0xassertion", &[]);
        malformed_change["diff"]["contracts"]["mock"]["changeType"] = json!("unknown");
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &malformed_change
        ));

        let mut missing_id = release_preview("0xassertion", &[]);
        missing_id["diff"]["contracts"]["mock"]["assertions"][0]["assertionId"] = Value::Null;
        assert!(!snapshot_matches_preview(
            &release_detail(snapshot_contracts("0xassertion", &[])),
            &missing_id
        ));
    }

    #[test]
    fn preflight_rejects_missing_or_readonly_credible_toml() {
        let dir = tempfile::tempdir().unwrap();

        // Missing file fails before any remote mutation.
        let missing = dir.path().join("credible.toml");
        assert!(matches!(
            preflight_project_id_write(&missing),
            Err(DeployError::TomlWriteBack { .. })
        ));

        // A read-only file fails the writability probe without being changed.
        std::fs::write(&missing, "environment = \"staging\"\n").unwrap();
        let mut permissions = std::fs::metadata(&missing).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&missing, permissions.clone()).unwrap();
        assert!(matches!(
            preflight_project_id_write(&missing),
            Err(DeployError::TomlWriteBack { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&missing).unwrap(),
            "environment = \"staging\"\n"
        );

        // Restore writability: preflight passes and still changes nothing.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&missing, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        #[cfg(not(unix))]
        {
            permissions.set_readonly(false);
            std::fs::set_permissions(&missing, permissions).unwrap();
        }
        preflight_project_id_write(&missing).unwrap();
        assert_eq!(
            std::fs::read_to_string(&missing).unwrap(),
            "environment = \"staging\"\n"
        );
    }

    #[test]
    fn create_intent_round_trips_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("credible.toml");
        let intent_path = create_intent_path(&config_path);
        assert_eq!(
            intent_path.file_name().unwrap().to_str().unwrap(),
            ".credible.toml.create-intent.json"
        );

        // No intent → no reconcile needed.
        assert!(load_create_intent(&intent_path).unwrap().is_none());

        let intent = CreateIntent {
            project_name: "demo".to_string(),
            chain_id: 84532,
            platform_url: "https://api.example".to_string(),
        };
        write_create_intent(&intent_path, &intent).unwrap();
        let loaded = load_create_intent(&intent_path).unwrap().unwrap();
        assert_eq!(loaded.project_name, "demo");
        assert_eq!(loaded.chain_id, 84532);
        assert_eq!(loaded.platform_url, "https://api.example");

        clear_create_intent(&intent_path);
        assert!(load_create_intent(&intent_path).unwrap().is_none());

        // A corrupt intent is an error, not a silent fresh create: it still
        // marks an unresolved earlier create.
        std::fs::write(&intent_path, "not json").unwrap();
        assert!(matches!(
            load_create_intent(&intent_path),
            Err(DeployError::CreateIntentUnreadable { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn create_intent_refuses_to_follow_an_existing_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let intent_path = dir.path().join(".credible.toml.create-intent.json");
        let target = dir.path().join("unrelated.json");
        std::fs::write(&target, "do not overwrite").unwrap();
        symlink(&target, &intent_path).unwrap();
        let intent = CreateIntent {
            project_name: "demo".to_string(),
            chain_id: 84532,
            platform_url: "https://api.example".to_string(),
        };

        assert!(matches!(
            write_create_intent(&intent_path, &intent),
            Err(DeployError::CreateIntentWrite { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "do not overwrite"
        );
    }

    #[test]
    fn create_intent_empty_index_fails_closed_until_the_project_appears() {
        let dir = tempfile::tempdir().unwrap();
        let intent_path = dir.path().join(".credible.toml.create-intent.json");
        let intent = CreateIntent {
            project_name: "demo".to_string(),
            chain_id: 84532,
            platform_url: "https://api.example".to_string(),
        };

        // The first index read can be stale after a lost create response. It
        // must stop the run before the create POST branch, leaving the intent
        // in place for a later reconciliation rather than risking a duplicate.
        assert!(matches!(
            resolve_create_intent_match(&[], &intent, &intent_path),
            Err(DeployError::PendingProjectCreate {
                name,
                chain_id: 84532,
                ..
            }) if name == "demo"
        ));

        // Once the project appears, the same intent is adopted without a
        // second create request.
        let project_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string();
        assert_eq!(
            resolve_create_intent_match(std::slice::from_ref(&project_id), &intent, &intent_path)
                .unwrap(),
            Uuid::parse_str(&project_id).unwrap()
        );
    }

    #[test]
    fn adopting_validates_the_project_chain_against_the_intent() {
        let dir = tempfile::tempdir().unwrap();
        let intent_path = dir.path().join(".credible.toml.create-intent.json");
        let intent = CreateIntent {
            project_name: "demo".to_string(),
            chain_id: 84532,
            platform_url: "https://api.example".to_string(),
        };
        let project_id = Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap();

        // The home index only matches on name (it lists chain *names*), so a
        // same-name project on another chain reaches this check. It must be
        // rejected before any local recovery state is written — not adopted
        // and then caught by resolve_chain_id after credible.toml is already
        // pointed at the wrong project.
        let other_chain = json!({ "project_networks": ["1"] });
        assert!(matches!(
            validate_adopted_project_chain(&other_chain, project_id, &intent, &intent_path),
            Err(DeployError::AdoptedProjectChainMismatch {
                intent_chain_id: 84532,
                found_chain_id: 1,
                ..
            })
        ));

        // The authoritative record on the intended chain adopts cleanly.
        let same_chain = json!({ "project_networks": ["84532"] });
        assert!(
            validate_adopted_project_chain(&same_chain, project_id, &intent, &intent_path).is_ok()
        );

        // A record with no chain info cannot be confirmed: fail closed rather
        // than adopt on faith.
        assert!(matches!(
            validate_adopted_project_chain(&json!({}), project_id, &intent, &intent_path),
            Err(DeployError::UnexpectedResponse { .. })
        ));
    }

    #[test]
    fn matching_project_ids_filters_by_name_and_chain() {
        let projects = json!([
            { "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "project_name": "demo", "project_networks": ["84532"] },
            { "id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", "project_name": "demo", "project_networks": ["1"] },
            { "id": "cccccccc-cccc-cccc-cccc-cccccccccccc", "project_name": "other", "project_networks": ["84532"] },
        ]);

        // Name + chain must both match.
        assert_eq!(
            matching_project_ids(&projects, "demo", 84532),
            vec!["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string()]
        );
        assert!(matching_project_ids(&projects, "demo", 59141).is_empty());
        assert!(matching_project_ids(&projects, "missing", 84532).is_empty());

        // Duplicate names on the same chain are all reported — the caller
        // must refuse to guess.
        let duplicates = json!({ "projects": [
            { "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "projectName": "demo", "project_networks": ["84532"] },
            { "id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", "name": "demo", "chain_id": 84532 },
        ]});
        assert_eq!(matching_project_ids(&duplicates, "demo", 84532).len(), 2);

        // An item without readable chain info still matches on name: err
        // toward "do not create a duplicate".
        let chainless =
            json!([{ "id": "dddddddd-dddd-dddd-dddd-dddddddddddd", "project_name": "demo" }]);
        assert_eq!(matching_project_ids(&chainless, "demo", 84532).len(), 1);

        // The exact `/views/projects/home` response shape: the caller's own
        // projects live at data.member_projects (with project_id/project_name
        // keys and chain *names*, not ids). Only member_projects count —
        // saved_projects are other people's projects, never proof that our
        // create landed.
        let home = json!({
            "data": {
                "member_projects": [
                    {
                        "project_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                        "project_name": "demo",
                        "slug": "demo",
                        "chain_names": ["Base Sepolia"],
                        "is_private": true
                    }
                ],
                "saved_projects": [
                    { "project_id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", "project_name": "demo" }
                ],
                "no_project_adopters": []
            },
            "_meta": { "sources": ["offchain"], "fetchedAt": "2026-05-10T04:16:00Z" }
        });
        assert_eq!(
            matching_project_ids(&home, "demo", 84532),
            vec!["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string()]
        );
        assert!(matching_project_ids(&home, "missing", 84532).is_empty());
    }

    #[test]
    fn write_project_id_replaces_the_file_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credible.toml");
        std::fs::write(&path, "environment = \"staging\"\n").unwrap();

        let id = Uuid::nil();
        write_project_id(&path, id).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(contents.starts_with(&format!("project_id = \"{id}\"")));
        // No temp file is left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn resolve_chain_id_uses_the_flag_only_for_created_projects() {
        let networks = json!({ "project_networks": ["8453"] });

        // Created this run: the create input is authoritative.
        assert_eq!(resolve_chain_id(Some(1), Some(1), &json!({})).unwrap(), 1);

        // Existing project: the platform record wins; a matching flag is fine.
        assert_eq!(resolve_chain_id(None, None, &networks).unwrap(), 8453);
        assert_eq!(resolve_chain_id(Some(8453), None, &networks).unwrap(), 8453);

        // A contradicting flag is rejected instead of rebinding the deploy.
        assert!(matches!(
            resolve_chain_id(Some(1), None, &networks),
            Err(DeployError::ChainIdMismatch {
                flag: 1,
                project: 8453
            })
        ));

        // An existing project without chain info is an error, not a fallback
        // to the flag.
        assert!(matches!(
            resolve_chain_id(Some(1), None, &json!({})),
            Err(DeployError::UnexpectedResponse { .. })
        ));
    }

    #[test]
    fn project_chain_id_handles_live_and_legacy_shapes() {
        // Live shadow API shape: string array.
        assert_eq!(
            project_chain_id(&json!({ "project_networks": ["8453"] })),
            Some(8453)
        );
        // Numeric array variant.
        assert_eq!(
            project_chain_id(&json!({ "project_networks": [8453] })),
            Some(8453)
        );
        // Direct field wins when present.
        assert_eq!(
            project_chain_id(&json!({ "chain_id": 1, "project_networks": ["8453"] })),
            Some(1)
        );
        assert_eq!(project_chain_id(&json!({})), None);
        assert_eq!(project_chain_id(&json!({ "project_networks": [] })), None);
    }

    #[test]
    fn release_summaries_parses_dapp_list_shape() {
        let body = json!([
            {
                "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "releaseNumber": 3,
                "environment": "staging",
                "status": "inactive",
                "previouslyDeployed": false,
                "createdAt": "2026-06-01T00:00:00+00:00",
                "deployedAt": null,
                "diff": null,
            },
        ]);
        let summaries = release_summaries(&body).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].release_number, Some(3));
        assert_eq!(summaries[0].status, "inactive");
    }

    #[test]
    fn release_summaries_fail_closed_on_malformed_items() {
        for body in [
            json!({ "releases": [] }),
            json!([{ "unexpected": "shape" }]),
            json!([{
                "id": "release-1",
                "releaseNumber": "3",
                "environment": "staging",
                "status": "inactive"
            }]),
        ] {
            assert!(matches!(
                release_summaries(&body),
                Err(DeployError::UnexpectedResponse { .. })
            ));
        }
    }

    #[tokio::test]
    async fn inactive_release_scan_paginates_past_the_first_hundred() {
        let mut server = mockito::Server::new_async().await;
        let project_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let first_page = Value::Array(
            (0..RELEASE_PAGE_SIZE)
                .map(|index| {
                    json!({
                        "id": format!("release-{index}"),
                        "releaseNumber": index,
                        "environment": "staging",
                        "status": "inactive"
                    })
                })
                .collect(),
        );
        let first = server
            .mock(
                "GET",
                format!("/api/v1/projects/{project_id}/releases").as_str(),
            )
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("environment".into(), "staging".into()),
                Matcher::UrlEncoded("status".into(), "inactive".into()),
                Matcher::UrlEncoded("limit".into(), "100".into()),
                Matcher::UrlEncoded("offset".into(), "0".into()),
            ]))
            .match_header("authorization", "Bearer access-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(first_page.to_string())
            .expect(1)
            .create_async()
            .await;
        let second = server
            .mock(
                "GET",
                format!("/api/v1/projects/{project_id}/releases").as_str(),
            )
            .match_query(Matcher::AllOf(vec![
                Matcher::UrlEncoded("environment".into(), "staging".into()),
                Matcher::UrlEncoded("status".into(), "inactive".into()),
                Matcher::UrlEncoded("limit".into(), "100".into()),
                Matcher::UrlEncoded("offset".into(), "100".into()),
            ]))
            .match_header("authorization", "Bearer access-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!([{
                    "id": "release-100",
                    "releaseNumber": 100,
                    "environment": "staging",
                    "status": "inactive"
                }])
                .to_string(),
            )
            .expect(1)
            .create_async()
            .await;
        let api = ApiArgs::headless(server.url().parse().unwrap());
        let mut config = CliConfig {
            auth: Some(crate::config::UserAuth {
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                expires_at: Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: Some("agent@example.com".to_string()),
            }),
            platform_url: Some(server.url()),
            rpc: BTreeMap::new(),
        };
        let config_dir = tempfile::tempdir().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };

        let summaries =
            inactive_release_summaries(&api, &mut config, &cli_args, project_id, "staging")
                .await
                .unwrap();

        assert_eq!(summaries.len(), RELEASE_PAGE_SIZE + 1);
        assert_eq!(summaries.last().unwrap().id, "release-100");
        first.assert_async().await;
        second.assert_async().await;
    }
}
