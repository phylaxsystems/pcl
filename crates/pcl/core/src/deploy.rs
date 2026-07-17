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
    created_at: String,
}

fn release_summaries(body: &Value) -> Vec<ReleaseSummary> {
    body.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(ReleaseSummary {
                        id: item.get("id")?.as_str()?.to_string(),
                        release_number: item.get("releaseNumber").and_then(Value::as_u64),
                        environment: item
                            .get("environment")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        status: item
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        created_at: item
                            .get("createdAt")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Newest release for the environment, if any.
fn newest_release<'a>(
    releases: &'a [ReleaseSummary],
    environment: &str,
) -> Option<&'a ReleaseSummary> {
    releases
        .iter()
        .filter(|release| release.environment == environment)
        .max_by(|a, b| a.created_at.cmp(&b.created_at))
}

/// Resume decision: when the preview reports no diff, the config already
/// matches the newest release for this environment — resume it if it was
/// never activated, otherwise everything is already deployed.
fn release_step(has_changes: bool, releases: &[ReleaseSummary], environment: &str) -> ReleaseStep {
    if has_changes {
        return ReleaseStep::Create;
    }
    match newest_release(releases, environment) {
        Some(release) if release.status == "inactive" => {
            ReleaseStep::Resume {
                release_id: release.id.clone(),
                release_number: release.release_number,
            }
        }
        _ => ReleaseStep::UpToDate,
    }
}

/// One assertion's identity within a release: (address, file, args, bytecode).
type AssertionKey = (String, String, Vec<String>, String);

/// The [`AssertionKey`] set describing a release's contents, extracted from
/// either a release payload or a release detail's `configSnapshot` — the two
/// share the `contracts` record shape.
fn assertion_set(contracts: &Value) -> Option<Vec<AssertionKey>> {
    let mut set = Vec::new();
    for contract in contracts.as_object()?.values() {
        let address = contract.get("address")?.as_str()?.to_ascii_lowercase();
        for assertion in contract.get("assertions")?.as_array()? {
            set.push((
                address.clone(),
                assertion.get("file")?.as_str()?.to_string(),
                assertion
                    .get("args")?
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect(),
                // Bytecode participates so a recompile (new compiler settings,
                // edited source) never resumes a stale release.
                assertion.get("bytecode")?.as_str()?.to_ascii_lowercase(),
            ));
        }
    }
    set.sort();
    Some(set)
}

/// Whether an existing (inactive) release contains exactly the assertions the
/// current payload would create. The preview endpoint diffs against the
/// *active* release only, so without this check a rerun before activation
/// would create a duplicate release instead of resuming.
fn snapshot_matches_payload(release_detail: &Value, payload: &Value) -> bool {
    let snapshot = release_detail
        .get("configSnapshot")
        .and_then(|snapshot| snapshot.get("contracts"))
        .and_then(assertion_set);
    let wanted = payload.get("contracts").and_then(assertion_set);
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
    let temp = parent.join(format!(".credible.toml.{}.tmp", std::process::id()));
    std::fs::write(&temp, &updated).map_err(|e| toml_write_back(e.to_string()))?;
    std::fs::rename(&temp, config_path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        toml_write_back(e.to_string())
    })
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
            self.confirm_step(
                human,
                &format!("Create project {name:?} on chain {chain_id} on the platform?"),
            )?;
            progress(
                human,
                &format!("Creating project {name:?} on chain {chain_id}"),
            );
            let body = json!({ "project_name": name, "chain_id": chain_id });
            let project = api
                .workflow_json(
                    config,
                    cli_args,
                    HttpMethod::Post,
                    "post_projects",
                    &[],
                    Some(&body),
                )
                .await?;
            let project_id = project
                .get("id")
                .or_else(|| project.get("project_id"))
                .and_then(Value::as_str)
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or(DeployError::UnexpectedResponse {
                    endpoint: "/projects",
                    reason: "missing project id in create response".to_string(),
                })?;
            // The remote project now exists; a failure here must carry its id
            // and a resume path so the next run adopts it instead of creating
            // a duplicate.
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
            credible.project_id = Some(project_id);
            progress(
                human,
                &format!("Created project {project_id} and recorded it in credible.toml"),
            );
            (project_id, project, Some(chain_id))
        };
        let project_created = created_chain_id.is_some();

        let project_chain_id = resolve_chain_id(self.chain_id, created_chain_id, &project)?;

        // ------------------------------------------------------------------
        // Step 2: protocol manager
        // ------------------------------------------------------------------
        let current_manager = project
            .get("protocol_manager_address")
            .and_then(Value::as_str)
            .and_then(|address| address.parse::<Address>().ok());
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
        let releases = api
            .workflow_json(
                config,
                cli_args,
                HttpMethod::Get,
                "get_projects_project_id_releases",
                &[("project_id", project_ref.as_str())],
                None,
            )
            .await?;
        let summaries = release_summaries(&releases);
        let step = if preview.has_changes() {
            // The preview diffs against the *active* release, so a release
            // created by an interrupted earlier run still shows as "changes".
            // Resume the newest inactive release when its snapshot matches
            // what we would create, instead of stacking duplicates.
            match newest_release(&summaries, &credible.environment) {
                Some(candidate) if candidate.status == "inactive" => {
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
                    if snapshot_matches_payload(&detail, &serde_json::to_value(&payload)?) {
                        ReleaseStep::Resume {
                            release_id: candidate.id.clone(),
                            release_number: candidate.release_number,
                        }
                    } else {
                        ReleaseStep::Create
                    }
                }
                _ => ReleaseStep::Create,
            }
        } else {
            release_step(false, &summaries, &credible.environment)
        };

        let (release_id, release_number, resumed) = match step {
            ReleaseStep::UpToDate => {
                progress(human, "Everything already released and deployed");
                let mut data = json!({
                    "outcome": "up_to_date",
                    "project_id": project_id,
                    "project_created": project_created,
                    "protocol_manager": { "action": manager_action, "address": wallet_address },
                    "release": Value::Null,
                    "tx": Value::Null,
                });
                #[cfg(feature = "credible")]
                insert_field(&mut data, "verification", json!(verification));
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

        let mut data = json!({
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
        insert_field(&mut data, "verification", json!(verification));
        Self::finish(
            output_mode,
            data,
            vec![
                format!("pcl releases show {project_id} {release_id}"),
                format!("pcl deployments --project {project_id}"),
            ],
        )
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
        let mut data = json!({
            "outcome": "dry_run",
            "project_id": credible.project_id,
            "would_create_project": credible.project_id.is_none(),
            "wallet_address": signer.map(|s| s.address().to_string()),
            "payload": payload,
        });
        #[cfg(feature = "credible")]
        insert_field(&mut data, "verification", json!(verification));
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

    fn summaries(items: &[(&str, &str, &str, &str)]) -> Vec<ReleaseSummary> {
        items
            .iter()
            .enumerate()
            .map(|(i, (id, environment, status, created_at))| {
                ReleaseSummary {
                    id: (*id).to_string(),
                    release_number: Some(i as u64 + 1),
                    environment: (*environment).to_string(),
                    status: (*status).to_string(),
                    created_at: (*created_at).to_string(),
                }
            })
            .collect()
    }

    #[test]
    fn release_step_decision_table() {
        // Diff present → always create.
        assert_eq!(release_step(true, &[], "staging"), ReleaseStep::Create);

        // No diff, no releases → up to date (nothing to deploy).
        assert_eq!(release_step(false, &[], "staging"), ReleaseStep::UpToDate);

        // No diff, newest matching-env release inactive → resume it
        // (created earlier, tx never landed).
        let releases = summaries(&[
            ("old", "staging", "active", "2026-01-01T00:00:00Z"),
            ("new", "staging", "inactive", "2026-02-01T00:00:00Z"),
        ]);
        assert_eq!(
            release_step(false, &releases, "staging"),
            ReleaseStep::Resume {
                release_id: "new".to_string(),
                release_number: Some(2),
            }
        );

        // No diff, newest matching-env release active → fully deployed.
        let releases = summaries(&[
            ("old", "staging", "inactive", "2026-01-01T00:00:00Z"),
            ("new", "staging", "active", "2026-02-01T00:00:00Z"),
        ]);
        assert_eq!(
            release_step(false, &releases, "staging"),
            ReleaseStep::UpToDate
        );

        // Other environments don't influence the decision.
        let releases = summaries(&[("prod", "production", "inactive", "2026-03-01T00:00:00Z")]);
        assert_eq!(
            release_step(false, &releases, "staging"),
            ReleaseStep::UpToDate
        );
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

    #[test]
    fn snapshot_matching_detects_identical_and_divergent_releases() {
        let contracts = |bytecode: &str, args: Vec<&str>| {
            json!({
                "mock": {
                    "address": "0x0101010101010101010101010101010101010101",
                    "name": "Mock",
                    "assertions": [{
                        "file": "assertions/src/A.a.sol:A",
                        "args": args,
                        "bytecode": bytecode,
                    }],
                }
            })
        };
        let detail = |contracts: Value| json!({ "configSnapshot": { "contracts": contracts } });
        let payload = |contracts: Value| json!({ "contracts": contracts });

        // Same content matches even with different address casing.
        let mut upper = contracts("0x6080", vec![]);
        upper["mock"]["address"] = json!(
            "0x0101010101010101010101010101010101010101"
                .to_ascii_uppercase()
                .replace("0X", "0x")
        );
        assert!(snapshot_matches_payload(
            &detail(upper),
            &payload(contracts("0x6080", vec![]))
        ));

        // Different bytecode (recompile) must NOT resume.
        assert!(!snapshot_matches_payload(
            &detail(contracts("0x6080", vec![])),
            &payload(contracts("0xdead", vec![]))
        ));

        // Different constructor args must NOT resume.
        assert!(!snapshot_matches_payload(
            &detail(contracts("0x6080", vec!["1"])),
            &payload(contracts("0x6080", vec!["2"]))
        ));

        // Missing snapshot (e.g. null bytecode) must NOT resume.
        assert!(!snapshot_matches_payload(
            &json!({ "configSnapshot": null }),
            &payload(contracts("0x6080", vec![]))
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
            { "unexpected": "shape" },
        ]);
        let summaries = release_summaries(&body);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].release_number, Some(3));
        assert_eq!(summaries[0].status, "inactive");
    }
}
