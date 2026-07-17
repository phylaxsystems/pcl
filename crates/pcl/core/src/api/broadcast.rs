//! Broadcast glue: fetch backend-computed calldata, sign and submit it
//! on-chain, then confirm the mutation with the API.
//!
//! Mirrors the dapp's browser-wallet flows (see
//! `packages/hooks/src/hooks/api/use-release-deploy.ts` and friends in the
//! credible-layer-dapp repo). The backend remains the only calldata encoder;
//! pcl only wraps operation calldata in `StateOracle.batch(bytes[])` or sends
//! raw `{to, data}` transactions.

use super::{
    ApiArgs,
    ApiCommandError,
    HttpMethod,
    ProtocolManagerArgs,
    ReleasesArgs,
    WorkflowOperation,
    runtime_types::WorkflowRequest,
};
use crate::{
    config::CliConfig,
    onchain::{
        TxArgs,
        TxOutcome,
        TxSender,
        encode_batch,
    },
    wallet::{
        WalletArgs,
        sign_personal,
    },
};
use alloy_primitives::{
    Address,
    Bytes,
};
use alloy_signer_local::PrivateKeySigner;
use colored::Colorize;
use pcl_common::args::CliArgs;
use serde::Deserialize;
use serde_json::{
    Value,
    json,
};
use std::path::Path;

/// Shared flags for commands that can sign and submit transactions.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct BroadcastArgs {
    /// Sign and submit the transaction on-chain, then confirm it with the API
    #[arg(long)]
    pub broadcast: bool,

    /// Skip the interactive broadcast confirmation prompt
    #[arg(long)]
    pub yes: bool,

    #[command(flatten)]
    pub wallet: WalletArgs,

    #[command(flatten)]
    pub tx: TxArgs,
}

// ---------------------------------------------------------------------------
// Response shapes (mirroring the dapp zod schemas; tolerate extra fields)
// ---------------------------------------------------------------------------

/// `GET /projects/{id}/releases/{rid}/deploy-calldata`
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReleaseDeployCalldataResponse {
    chain_id: u64,
    state_oracle_address: Address,
    #[serde(default)]
    calldata: Vec<Bytes>,
    is_noop: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    operations: Vec<Value>,
}

/// `GET /projects/{id}/releases/{rid}/remove-calldata`
#[derive(Debug, Deserialize)]
struct ReleaseRemoveCalldataResponse {
    to: Address,
    data: Bytes,
    #[serde(default)]
    assertions: Vec<Value>,
}

/// `GET /projects/{id}/protocol-manager/nonce`
#[derive(Debug, Deserialize)]
struct ProtocolManagerNonceResponse {
    nonce: String,
    message: String,
    #[serde(default)]
    expires_at: Option<String>,
}

/// `GET /projects/{id}/protocol-manager/transfer-calldata` (mode union)
#[derive(Debug, Deserialize)]
struct TransferCalldataResponse {
    mode: String,
    #[serde(default)]
    chain_id: Option<u64>,
    #[serde(default)]
    to: Option<Address>,
    #[serde(default)]
    calldata: Option<Bytes>,
    #[serde(default)]
    total_contracts: Option<u64>,
}

/// `GET /projects/{id}/protocol-manager/accept-calldata`
#[derive(Debug, Deserialize)]
struct AcceptCalldataResponse {
    chain_id: u64,
    to: Address,
    calldata: Bytes,
}

fn parse_response<T: serde::de::DeserializeOwned>(
    endpoint: &'static str,
    body: &Value,
) -> Result<T, ApiCommandError> {
    serde_json::from_value(body.clone()).map_err(|e| {
        ApiCommandError::UnexpectedResponse {
            endpoint,
            reason: e.to_string(),
        }
    })
}

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

async fn resolve_signer(
    wallet: &WalletArgs,
    cli_args: &CliArgs,
) -> Result<PrivateKeySigner, ApiCommandError> {
    Ok(wallet.signer(cli_args.human_output()).await?)
}

/// Interactive guard before spending gas. Machine mode treats `--broadcast`
/// itself as consent; human mode prompts unless `--yes`.
fn confirm_send(
    cli_args: &CliArgs,
    yes: bool,
    signer: Address,
    chain_id: u64,
    to: Address,
    summary: &str,
) -> Result<(), ApiCommandError> {
    if !cli_args.human_output() || yes {
        return Ok(());
    }
    eprintln!("{}", "About to broadcast a transaction".bold());
    eprintln!("  signer:   {signer}");
    eprintln!("  chain id: {chain_id}");
    eprintln!("  to:       {to}");
    eprintln!("  action:   {summary}");
    let confirmed = inquire::Confirm::new("Send this transaction?")
        .with_default(false)
        .prompt()
        .map_err(|_| ApiCommandError::BroadcastCancelled)?;
    if !confirmed {
        return Err(ApiCommandError::BroadcastCancelled);
    }
    Ok(())
}

/// Interactive guard before a mutating platform call that involves no
/// transaction (noop deploy confirmation, direct manager transfer). Machine
/// mode treats `--broadcast` itself as consent; human mode prompts unless
/// `--yes`.
fn confirm_mutation(cli_args: &CliArgs, yes: bool, summary: &str) -> Result<(), ApiCommandError> {
    if !cli_args.human_output() || yes {
        return Ok(());
    }
    eprintln!("{}", "About to confirm a platform mutation".bold());
    eprintln!("  action: {summary}");
    let confirmed = inquire::Confirm::new("Proceed?")
        .with_default(false)
        .prompt()
        .map_err(|_| ApiCommandError::BroadcastCancelled)?;
    if !confirmed {
        return Err(ApiCommandError::BroadcastCancelled);
    }
    Ok(())
}

fn progress(cli_args: &CliArgs, message: &str) {
    if cli_args.human_output() {
        eprintln!("{} {message}", "→".cyan());
    }
}

async fn send_tx(
    config: &CliConfig,
    tx_args: &TxArgs,
    signer: PrivateKeySigner,
    chain_id: u64,
    to: Address,
    data: Bytes,
) -> Result<TxOutcome, ApiCommandError> {
    let rpc = tx_args.resolve_rpc(config, chain_id)?;
    let confirmations = tx_args.resolve_confirmations(config, chain_id);
    let sender = TxSender::connect(rpc, signer, chain_id).await?;
    Ok(sender
        .send_and_confirm(to, data, confirmations, tx_args.timeout())
        .await?)
}

fn tx_value(outcome: &TxOutcome) -> Value {
    json!({
        "tx_hash": outcome.tx_hash,
        "chain_id": outcome.chain_id,
        "block_number": outcome.block_number,
        "gas_used": outcome.gas_used,
        "effective_gas_price": outcome.effective_gas_price.to_string(),
        "confirmations_waited": outcome.confirmations_waited,
    })
}

fn broadcast_envelope(action: &str, data: Value, next_actions: Vec<String>) -> Value {
    crate::output::with_envelope_metadata(json!({
        "status": "ok",
        "action": action,
        "data": data,
        "next_actions": next_actions,
    }))
}

fn get_operation(
    operation_id: &'static str,
    project: &str,
    release_id: Option<&str>,
    query: Vec<(String, String)>,
) -> Result<WorkflowRequest, ApiCommandError> {
    let mut operation =
        WorkflowOperation::new(HttpMethod::Get, operation_id).path_param("project_id", project);
    if let Some(release_id) = release_id {
        operation = operation.path_param("release_id", release_id);
    }
    WorkflowRequest::from_operation(operation, query, None, true, Vec::<String>::new())
}

fn post_operation(
    operation_id: &'static str,
    project: &str,
    release_id: Option<&str>,
    body: &Value,
) -> Result<WorkflowRequest, ApiCommandError> {
    let mut operation =
        WorkflowOperation::new(HttpMethod::Post, operation_id).path_param("project_id", project);
    if let Some(release_id) = release_id {
        operation = operation.path_param("release_id", release_id);
    }
    WorkflowRequest::from_operation(
        operation,
        Vec::new(),
        Some(body.to_string()),
        true,
        Vec::<String>::new(),
    )
}

/// Wraps a confirmation-POST failure that happens *after* the transaction
/// landed so the caller always learns the tx hash.
fn confirm_after_tx(outcome: &TxOutcome, source: ApiCommandError) -> ApiCommandError {
    ApiCommandError::ConfirmAfterTx {
        tx_hash: outcome.tx_hash,
        source: Box::new(source),
    }
}

impl ApiArgs {
    /// Fetches the chain id recorded on a project (used when a calldata
    /// response does not carry one).
    async fn project_chain_id(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        project: &str,
        request_log_path: &Path,
    ) -> Result<u64, ApiCommandError> {
        let request = get_operation("get_projects_project_id", project, None, Vec::new())?;
        let result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        crate::deploy::project_chain_id(&result.body).ok_or(ApiCommandError::UnexpectedResponse {
            endpoint: "/projects/{project_id}",
            reason: "missing chain_id/project_networks".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Releases: deploy / remove
// ---------------------------------------------------------------------------

impl ApiArgs {
    pub(in crate::api) async fn run_releases_broadcast(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ReleasesArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let project = args
            .project
            .clone()
            .ok_or(ApiCommandError::InvalidWorkflow {
                message: "--broadcast requires a project reference".to_string(),
            })?;
        let release_id = args
            .release_id
            .clone()
            .ok_or(ApiCommandError::InvalidWorkflow {
                message: "--broadcast requires a release id".to_string(),
            })?;
        if args.deploy_calldata {
            self.broadcast_release_deploy(
                config,
                cli_args,
                args,
                &project,
                &release_id,
                request_log_path,
            )
            .await
        } else if args.remove_calldata {
            self.broadcast_release_remove(
                config,
                cli_args,
                args,
                &project,
                &release_id,
                request_log_path,
            )
            .await
        } else {
            Err(ApiCommandError::InvalidWorkflow {
                message: "--broadcast applies to `pcl releases calldata deploy|remove`".to_string(),
            })
        }
    }

    async fn broadcast_release_deploy(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ReleasesArgs,
        project: &str,
        release_id: &str,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let signer = resolve_signer(&args.broadcast.wallet, cli_args).await?;
        let signer_address = match args.signer_address.as_deref() {
            None => signer.address(),
            Some(explicit) => {
                let explicit: Address = explicit.parse().map_err(|_| {
                    ApiCommandError::InvalidWorkflow {
                        message: format!("Invalid --signer-address {explicit:?}"),
                    }
                })?;
                if explicit != signer.address() {
                    return Err(ApiCommandError::InvalidWorkflow {
                        message: format!(
                            "--signer-address {explicit} does not match the wallet address {}; the deploy calldata is computed for the broadcasting wallet",
                            signer.address()
                        ),
                    });
                }
                explicit
            }
        };

        progress(cli_args, "Fetching deploy calldata");
        let request = get_operation(
            "get_projects_project_id_releases_release_id_deploy_calldata",
            project,
            Some(release_id),
            vec![("signerAddress".to_string(), signer_address.to_string())],
        )?;
        let calldata_result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let calldata: ReleaseDeployCalldataResponse =
            parse_response("deploy-calldata", &calldata_result.body)?;

        let next_actions = vec![
            format!("pcl releases show {project} {release_id}"),
            format!("pcl deployments --project {project}"),
        ];

        if calldata.is_noop {
            confirm_mutation(
                cli_args,
                args.broadcast.yes,
                &format!(
                    "confirm noop deploy of release {release_id} (no on-chain changes, marks the release deployed)"
                ),
            )?;
            progress(
                cli_args,
                "No on-chain changes needed; confirming noop deploy",
            );
            let confirm_request = post_operation(
                "post_projects_project_id_releases_release_id_deploy",
                project,
                Some(release_id),
                &json!({ "mode": "noop" }),
            )?;
            let confirm = self
                .call_workflow_result(config, cli_args, &confirm_request, request_log_path)
                .await?;
            return Ok(broadcast_envelope(
                "release_deploy_broadcast",
                json!({
                    "noop": true,
                    "message": calldata.message,
                    "calldata": calldata_result.body,
                    "tx": Value::Null,
                    "confirm": confirm.body,
                }),
                next_actions,
            ));
        }

        confirm_send(
            cli_args,
            args.broadcast.yes,
            signer.address(),
            calldata.chain_id,
            calldata.state_oracle_address,
            &format!(
                "StateOracle.batch with {} operation(s) for release {release_id}",
                calldata.operations.len().max(calldata.calldata.len())
            ),
        )?;

        progress(cli_args, "Broadcasting StateOracle.batch transaction");
        let outcome = send_tx(
            config,
            &args.broadcast.tx,
            signer,
            calldata.chain_id,
            calldata.state_oracle_address,
            encode_batch(calldata.calldata.clone()),
        )
        .await?;

        progress(cli_args, "Confirming deployment with the platform");
        let confirm_request = post_operation(
            "post_projects_project_id_releases_release_id_deploy",
            project,
            Some(release_id),
            &json!({ "mode": "transaction", "txHash": outcome.tx_hash }),
        )?;
        let confirm = self
            .call_workflow_result(config, cli_args, &confirm_request, request_log_path)
            .await
            .map_err(|e| confirm_after_tx(&outcome, e))?;

        Ok(broadcast_envelope(
            "release_deploy_broadcast",
            json!({
                "noop": false,
                "calldata": calldata_result.body,
                "tx": tx_value(&outcome),
                "confirm": confirm.body,
            }),
            next_actions,
        ))
    }

    async fn broadcast_release_remove(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ReleasesArgs,
        project: &str,
        release_id: &str,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let signer = resolve_signer(&args.broadcast.wallet, cli_args).await?;

        progress(cli_args, "Fetching remove calldata");
        let request = get_operation(
            "get_projects_project_id_releases_release_id_remove_calldata",
            project,
            Some(release_id),
            Vec::new(),
        )?;
        let calldata_result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let calldata: ReleaseRemoveCalldataResponse =
            parse_response("remove-calldata", &calldata_result.body)?;

        // The remove-calldata response carries no chain id and its `{to,
        // data}` would look valid on any network, so the project record is
        // authoritative. An explicit --chain-id may only confirm it, never
        // rebind the broadcast to another chain.
        let chain_id = self
            .project_chain_id(config, cli_args, project, request_log_path)
            .await?;
        if let Some(explicit) = args.chain_id
            && explicit != chain_id
        {
            return Err(ApiCommandError::InvalidWorkflow {
                message: format!(
                    "--chain-id {explicit} does not match the project's chain {chain_id}; omit --chain-id to broadcast the removal"
                ),
            });
        }

        confirm_send(
            cli_args,
            args.broadcast.yes,
            signer.address(),
            chain_id,
            calldata.to,
            &format!(
                "remove release {release_id} ({} assertion(s))",
                calldata.assertions.len()
            ),
        )?;

        progress(cli_args, "Broadcasting removal transaction");
        let outcome = send_tx(
            config,
            &args.broadcast.tx,
            signer,
            chain_id,
            calldata.to,
            calldata.data.clone(),
        )
        .await?;

        progress(cli_args, "Confirming removal with the platform");
        let confirm_request = post_operation(
            "post_projects_project_id_releases_release_id_remove",
            project,
            Some(release_id),
            &json!({ "txHash": outcome.tx_hash }),
        )?;
        let confirm = self
            .call_workflow_result(config, cli_args, &confirm_request, request_log_path)
            .await
            .map_err(|e| confirm_after_tx(&outcome, e))?;

        Ok(broadcast_envelope(
            "release_remove_broadcast",
            json!({
                "calldata": calldata_result.body,
                "tx": tx_value(&outcome),
                "confirm": confirm.body,
            }),
            vec![format!("pcl releases list {project}")],
        ))
    }
}

// ---------------------------------------------------------------------------
// Protocol manager: set (off-chain signature), transfer, accept
// ---------------------------------------------------------------------------

impl ApiArgs {
    pub(in crate::api) async fn run_protocol_manager_signed(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProtocolManagerArgs,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        validate_manager_consent(args)?;
        let project = args
            .project
            .clone()
            .ok_or(ApiCommandError::InvalidWorkflow {
                message: "protocol-manager signing requires --project".to_string(),
            })?;
        if args.set {
            self.protocol_manager_set_signed(config, cli_args, args, &project, request_log_path)
                .await
        } else if args.transfer_calldata {
            self.protocol_manager_transfer_broadcast(
                config,
                cli_args,
                args,
                &project,
                request_log_path,
            )
            .await
        } else if args.accept_calldata {
            self.protocol_manager_accept_broadcast(
                config,
                cli_args,
                args,
                &project,
                request_log_path,
            )
            .await
        } else {
            Err(ApiCommandError::InvalidWorkflow {
                message: "--sign applies to --set; --broadcast applies to --transfer-calldata or --accept-calldata"
                    .to_string(),
            })
        }
    }

    /// `--set --sign`: fetch the challenge nonce, sign it (EIP-191), submit.
    /// Retries once when the challenge expired between fetch and submit.
    async fn protocol_manager_set_signed(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProtocolManagerArgs,
        project: &str,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let signer = resolve_signer(&args.broadcast.wallet, cli_args).await?;
        let chain_id = args.chain_id.ok_or(ApiCommandError::InvalidWorkflow {
            message: "--set --sign requires --chain-id (the challenge is chain-bound)".to_string(),
        })?;

        let mut attempts = 0;
        loop {
            attempts += 1;
            progress(cli_args, "Fetching protocol-manager challenge nonce");
            let nonce_request = get_operation(
                "get_projects_project_id_protocol_manager_nonce",
                project,
                None,
                vec![
                    ("address".to_string(), signer.address().to_string()),
                    ("chain_id".to_string(), chain_id.to_string()),
                ],
            )?;
            let nonce_result = self
                .call_workflow_result(config, cli_args, &nonce_request, request_log_path)
                .await?;
            let nonce: ProtocolManagerNonceResponse =
                parse_response("protocol-manager/nonce", &nonce_result.body)?;

            progress(cli_args, "Signing challenge message (EIP-191)");
            let signature = sign_personal(&signer, &nonce.message).await?;

            progress(cli_args, "Submitting signed challenge");
            let set_request = post_operation(
                "post_projects_project_id_protocol_manager",
                project,
                None,
                &json!({
                    "address": signer.address(),
                    "signature": signature,
                    "nonce": nonce.nonce,
                }),
            )?;
            match self
                .call_workflow_result(config, cli_args, &set_request, request_log_path)
                .await
            {
                Ok(result) => {
                    return Ok(broadcast_envelope(
                        "protocol_manager_set_signed",
                        json!({
                            "manager_address": signer.address(),
                            "chain_id": chain_id,
                            "nonce_expires_at": nonce.expires_at,
                            "result": result.body,
                        }),
                        vec![format!("pcl protocol-manager --project {project}")],
                    ));
                }
                Err(error) if attempts == 1 && challenge_expired(&error) => {
                    progress(cli_args, "Challenge expired; retrying with a fresh nonce");
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn protocol_manager_transfer_broadcast(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProtocolManagerArgs,
        project: &str,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let new_manager = args
            .new_manager
            .clone()
            .ok_or(ApiCommandError::InvalidWorkflow {
                message: "--transfer-calldata --broadcast requires --new-manager".to_string(),
            })?;
        let signer = resolve_signer(&args.broadcast.wallet, cli_args).await?;

        progress(cli_args, "Fetching manager transfer calldata");
        let request = get_operation(
            "get_projects_project_id_protocol_manager_transfer_calldata",
            project,
            None,
            vec![("new_manager".to_string(), new_manager.clone())],
        )?;
        let calldata_result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let calldata: TransferCalldataResponse =
            parse_response("protocol-manager/transfer-calldata", &calldata_result.body)?;

        let next_actions = vec![format!(
            "pcl protocol-manager --project {project} --pending-transfer"
        )];

        if calldata.mode == "direct" {
            // Nothing on-chain to move; confirm directly.
            confirm_mutation(
                cli_args,
                args.broadcast.yes,
                &format!("transfer the protocol manager to {new_manager} (no transaction needed)"),
            )?;
            progress(cli_args, "No on-chain transfer needed; confirming directly");
            let confirm_request = post_operation(
                "post_projects_project_id_protocol_manager_confirm_transfer",
                project,
                None,
                &json!({ "mode": "direct", "new_manager_address": new_manager }),
            )?;
            let confirm = self
                .call_workflow_result(config, cli_args, &confirm_request, request_log_path)
                .await?;
            return Ok(broadcast_envelope(
                "protocol_manager_transfer_broadcast",
                json!({
                    "mode": "direct",
                    "calldata": calldata_result.body,
                    "tx": Value::Null,
                    "confirm": confirm.body,
                }),
                next_actions,
            ));
        }

        let (Some(chain_id), Some(to), Some(data)) =
            (calldata.chain_id, calldata.to, calldata.calldata.clone())
        else {
            return Err(ApiCommandError::UnexpectedResponse {
                endpoint: "protocol-manager/transfer-calldata",
                reason: "onchain mode without chain_id/to/calldata".to_string(),
            });
        };

        confirm_send(
            cli_args,
            args.broadcast.yes,
            signer.address(),
            chain_id,
            to,
            &format!(
                "transfer protocol manager to {new_manager} ({} contract(s))",
                calldata.total_contracts.unwrap_or_default()
            ),
        )?;

        progress(cli_args, "Broadcasting manager transfer transaction");
        let outcome = send_tx(config, &args.broadcast.tx, signer, chain_id, to, data).await?;

        // The on-chain transfer is a two-transaction flow: this initiation tx
        // only marks the transfer pending. The `ManagerTransferred` logs the
        // platform verifier requires are emitted by the new manager's
        // acceptance tx, so the confirm-transfer POST happens on the
        // acceptance path, not here.
        progress(
            cli_args,
            "Transfer initiated; the new manager must accept it to complete the handover",
        );

        Ok(broadcast_envelope(
            "protocol_manager_transfer_broadcast",
            json!({
                "mode": "onchain",
                "calldata": calldata_result.body,
                "tx": tx_value(&outcome),
                "pending_acceptance": true,
            }),
            vec![
                format!("pcl protocol-manager --project {project} --pending-transfer"),
                format!(
                    "pcl protocol-manager --project {project} --accept-calldata --broadcast (with the new manager's wallet)"
                ),
            ],
        ))
    }

    async fn protocol_manager_accept_broadcast(
        &self,
        config: &mut CliConfig,
        cli_args: &CliArgs,
        args: &ProtocolManagerArgs,
        project: &str,
        request_log_path: &Path,
    ) -> Result<Value, ApiCommandError> {
        let signer = resolve_signer(&args.broadcast.wallet, cli_args).await?;

        progress(cli_args, "Fetching manager acceptance calldata");
        let request = get_operation(
            "get_projects_project_id_protocol_manager_accept_calldata",
            project,
            None,
            Vec::new(),
        )?;
        let calldata_result = self
            .call_workflow_result(config, cli_args, &request, request_log_path)
            .await?;
        let calldata: AcceptCalldataResponse =
            parse_response("protocol-manager/accept-calldata", &calldata_result.body)?;

        confirm_send(
            cli_args,
            args.broadcast.yes,
            signer.address(),
            calldata.chain_id,
            calldata.to,
            "accept pending protocol manager transfer",
        )?;

        progress(cli_args, "Broadcasting manager acceptance transaction");
        let signer_address = signer.address();
        let outcome = send_tx(
            config,
            &args.broadcast.tx,
            signer,
            calldata.chain_id,
            calldata.to,
            calldata.calldata.clone(),
        )
        .await?;

        // The acceptance tx emits the `ManagerTransferred` logs the platform
        // verifier requires, so this — not the initiation tx — is the receipt
        // to confirm with, and the accepting signer is the new manager.
        progress(cli_args, "Confirming transfer with the platform");
        let confirm_request = post_operation(
            "post_projects_project_id_protocol_manager_confirm_transfer",
            project,
            None,
            &json!({
                "mode": "onchain",
                "tx_hash": outcome.tx_hash,
                "chain_id": calldata.chain_id,
                "new_manager_address": signer_address,
            }),
        )?;
        let confirm = self
            .call_workflow_result(config, cli_args, &confirm_request, request_log_path)
            .await
            .map_err(|e| confirm_after_tx(&outcome, e))?;

        Ok(broadcast_envelope(
            "protocol_manager_accept_broadcast",
            json!({
                "calldata": calldata_result.body,
                "tx": tx_value(&outcome),
                "confirm": confirm.body,
            }),
            vec![format!("pcl protocol-manager --project {project}")],
        ))
    }
}

/// Each mutating protocol-manager action must carry its own consent flag:
/// `--sign` consents to the off-chain signed challenge (`--set`), and
/// `--broadcast` consents to an on-chain transaction (`--transfer-calldata` /
/// `--accept-calldata`). Checked before any wallet resolution or mutation so
/// one flag never authorizes the other kind of action.
fn validate_manager_consent(args: &ProtocolManagerArgs) -> Result<(), ApiCommandError> {
    if args.set && !args.sign {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--set requires --sign to submit the signed challenge".to_string(),
        });
    }
    if (args.transfer_calldata || args.accept_calldata) && !args.broadcast.broadcast {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--transfer-calldata and --accept-calldata require --broadcast to submit the transaction"
                .to_string(),
        });
    }
    Ok(())
}

/// Whether an API error indicates the signed challenge expired and a fresh
/// nonce should be fetched.
fn challenge_expired(error: &ApiCommandError) -> bool {
    let ApiCommandError::HttpStatus { body, .. } = error else {
        return false;
    };
    body.get("code")
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .is_some_and(|code| matches!(code, "nonce_expired" | "signature_expired"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn deploy_calldata_response_parses_dapp_shape() {
        let body = json!({
            "chainId": 84532,
            "stateOracleAddress": "0xed9A39bf60d325796De60D67b3BD00ce6c4B5007",
            "operations": [
                { "type": "addAssertion", "adopterAddress": "0x0101010101010101010101010101010101010101", "assertionId": "0xabcd" }
            ],
            "calldata": ["0x1234", "0x56"],
            "isNoop": false,
            "message": null,
            "someFutureField": true,
        });
        let parsed: ReleaseDeployCalldataResponse =
            parse_response("deploy-calldata", &body).unwrap();
        assert_eq!(parsed.chain_id, 84532);
        assert_eq!(
            parsed.state_oracle_address,
            address!("ed9A39bf60d325796De60D67b3BD00ce6c4B5007")
        );
        assert_eq!(parsed.calldata.len(), 2);
        assert!(!parsed.is_noop);
        assert_eq!(parsed.operations.len(), 1);
        assert!(parsed.message.is_none());
    }

    #[test]
    fn deploy_calldata_noop_parses_without_calldata() {
        let body = json!({
            "chainId": 1,
            "stateOracleAddress": "0x0101010101010101010101010101010101010101",
            "operations": [],
            "calldata": [],
            "isNoop": true,
            "message": "Everything already deployed",
        });
        let parsed: ReleaseDeployCalldataResponse =
            parse_response("deploy-calldata", &body).unwrap();
        assert!(parsed.is_noop);
        assert_eq!(
            parsed.message.as_deref(),
            Some("Everything already deployed")
        );
    }

    #[test]
    fn remove_calldata_response_parses_dapp_shape() {
        let body = json!({
            "to": "0xed9A39bf60d325796De60D67b3BD00ce6c4B5007",
            "data": "0x1e897afb",
            "assertions": [
                { "contractAddress": "0x0101010101010101010101010101010101010101", "assertionId": "1", "file": null }
            ],
        });
        let parsed: ReleaseRemoveCalldataResponse =
            parse_response("remove-calldata", &body).unwrap();
        assert_eq!(parsed.assertions.len(), 1);
        assert_eq!(parsed.data.as_ref(), [0x1e, 0x89, 0x7a, 0xfb]);
    }

    #[test]
    fn transfer_calldata_parses_both_union_modes() {
        let onchain = json!({
            "mode": "onchain",
            "chain_id": 84532,
            "to": "0x0101010101010101010101010101010101010101",
            "calldata": "0xdeadbeef",
            "contracts": [{ "address": "0x0101010101010101010101010101010101010101", "calldata": "0x01" }],
            "total_contracts": 1,
        });
        let parsed: TransferCalldataResponse =
            parse_response("transfer-calldata", &onchain).unwrap();
        assert_eq!(parsed.mode, "onchain");
        assert_eq!(parsed.chain_id, Some(84532));
        assert!(parsed.to.is_some());

        let direct = json!({
            "mode": "direct",
            "to": null,
            "calldata": null,
            "contracts": [],
            "total_contracts": 0,
        });
        let parsed: TransferCalldataResponse =
            parse_response("transfer-calldata", &direct).unwrap();
        assert_eq!(parsed.mode, "direct");
        assert!(parsed.to.is_none());
        assert!(parsed.calldata.is_none());
    }

    #[test]
    fn accept_calldata_parses_snake_case_shape() {
        let body = json!({
            "chain_id": 84532,
            "to": "0x0101010101010101010101010101010101010101",
            "calldata": "0x0102",
            "contracts": [],
            "total_contracts": 0,
        });
        let parsed: AcceptCalldataResponse = parse_response("accept-calldata", &body).unwrap();
        assert_eq!(parsed.chain_id, 84532);
    }

    #[test]
    fn nonce_response_parses() {
        let body = json!({
            "nonce": "abc123",
            "message": "Sign this message to prove control",
            "expires_at": "2026-07-03T00:00:00Z",
        });
        let parsed: ProtocolManagerNonceResponse =
            parse_response("protocol-manager/nonce", &body).unwrap();
        assert_eq!(parsed.nonce, "abc123");
        assert!(parsed.message.starts_with("Sign"));
    }

    #[test]
    fn malformed_response_yields_unexpected_response_error() {
        let err =
            parse_response::<ReleaseDeployCalldataResponse>("deploy-calldata", &json!({"nope": 1}))
                .unwrap_err();
        assert!(matches!(
            err,
            ApiCommandError::UnexpectedResponse {
                endpoint: "deploy-calldata",
                ..
            }
        ));
    }

    #[test]
    fn manager_consent_requires_the_exact_flag_pair() {
        let args = |set: bool, transfer: bool, accept: bool, sign: bool, broadcast: bool| {
            ProtocolManagerArgs {
                set,
                transfer_calldata: transfer,
                accept_calldata: accept,
                sign,
                broadcast: BroadcastArgs {
                    broadcast,
                    ..BroadcastArgs::default()
                },
                ..ProtocolManagerArgs::default()
            }
        };

        // Exact pairs pass.
        assert!(validate_manager_consent(&args(true, false, false, true, false)).is_ok());
        assert!(validate_manager_consent(&args(false, true, false, false, true)).is_ok());
        assert!(validate_manager_consent(&args(false, false, true, false, true)).is_ok());
        // The deploy flow passes set+sign with broadcast consent also on.
        assert!(validate_manager_consent(&args(true, false, false, true, true)).is_ok());

        // --set --broadcast must not perform the signed set without --sign.
        assert!(validate_manager_consent(&args(true, false, false, false, true)).is_err());
        // --transfer-calldata/--accept-calldata --sign must not broadcast
        // without --broadcast.
        assert!(validate_manager_consent(&args(false, true, false, true, false)).is_err());
        assert!(validate_manager_consent(&args(false, false, true, true, false)).is_err());
    }

    #[test]
    fn challenge_expired_detects_expiry_codes() {
        let expired = ApiCommandError::HttpStatus {
            method: "POST",
            path: "/x".to_string(),
            status: 400,
            request_id: None,
            body: Box::new(json!({ "code": "nonce_expired" })),
        };
        assert!(challenge_expired(&expired));

        let other = ApiCommandError::HttpStatus {
            method: "POST",
            path: "/x".to_string(),
            status: 400,
            request_id: None,
            body: Box::new(json!({ "code": "signature_mismatch" })),
        };
        assert!(!challenge_expired(&other));
        assert!(!challenge_expired(&ApiCommandError::BroadcastCancelled));
    }
}
