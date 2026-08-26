//! Transaction broadcasting for on-chain Credible Layer operations.
//!
//! The backend computes all calldata (see the `*-calldata` API endpoints);
//! this module only signs and submits it — either as `StateOracle.batch(bytes[])`
//! or as a raw `{to, data}` transaction — and waits for confirmations.
//!
//! Submission signs once and resubmits those exact bytes when an endpoint says
//! it is unavailable, so a node deduplicates the retry rather than accepting a
//! second transaction.

use crate::config::CliConfig;
use alloy_network::{
    Ethereum,
    EthereumWallet,
    eip2718::Encodable2718,
};
use alloy_primitives::{
    B256,
    Bytes,
};
use alloy_provider::{
    PendingTransactionBuilder,
    PendingTransactionError,
    Provider,
    ProviderBuilder,
    RootProvider,
    SendableTx,
    fillers::{
        FillProvider,
        JoinFill,
        WalletFiller,
    },
    transport::{
        RpcError,
        TransportError,
    },
    utils::JoinedRecommendedFillers,
};
use alloy_rpc_types_eth::{
    TransactionReceipt,
    TransactionRequest,
};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{
    SolCall,
    sol,
};
use serde::Serialize;
use std::time::Duration;
use thiserror::Error;
use tokio::time::Instant;
use url::Url;

// Attempts a credible-layer-gated step gets before it is treated as terminal.
// The alignment window is 0.2-1.2s in practice; the backoff below spans ~6s.
const ALIGNMENT_ATTEMPTS: u32 = 6;
const ALIGNMENT_FIRST_DELAY: Duration = Duration::from_millis(250);
const ALIGNMENT_MAX_DELAY: Duration = Duration::from_secs(2);

sol! {
    /// `StateOracle`'s batch entrypoint; the only function pcl encodes locally.
    function batch(bytes[] calldata data) external;
}

/// Fallback confirmation floor when the platform does not state its own
/// requirement: 1 for local devnets, 3 for known testnets, and 12 for
/// mainnets/unknown chains. The real policy lives in the platform's chain
/// profile (`getRequiredConfirmations`) and varies per deployment, so a
/// platform-provided value always overrides this table; when only the
/// fallback is available and it undershoots, the confirmation POST is
/// retried while confirmations accrue instead of failing the landed tx.
pub fn fallback_confirmations(chain_id: u64) -> u64 {
    match chain_id {
        // Local devnets (anvil/hardhat).
        1337 | 31337 => 1,
        // Sepolia, Base Sepolia, Linea Sepolia, Arbitrum Sepolia, OP Sepolia.
        11_155_111 | 84532 | 59141 | 421_614 | 11_155_420 => 3,
        _ => 12,
    }
}

/// Errors that can occur while broadcasting transactions.
///
/// RPC endpoints in these messages are redacted to their scheme/host/port
/// origin: stored provider URLs commonly embed API keys, and error envelopes
/// end up in logs and transcripts.
#[derive(Error, Debug)]
pub enum OnchainError {
    #[error(
        "No RPC endpoint for chain {chain_id}. Pass --rpc-url (or set PCL_RPC_URL), or store one with `pcl config set-rpc {chain_id} <url>`."
    )]
    RpcUrlMissing { chain_id: u64 },

    #[error("Invalid RPC URL ({redacted_url}) configured for chain {chain_id}: {reason}")]
    InvalidRpcUrl {
        chain_id: u64,
        redacted_url: String,
        reason: String,
    },

    #[error(
        "{requested} confirmation(s) is below the {required} required on chain {chain_id}; the platform would reject the confirmation with INSUFFICIENT_CONFIRMATIONS. Raise --confirmations (or the stored per-chain value) to at least {required}."
    )]
    InsufficientConfirmations {
        chain_id: u64,
        requested: u64,
        required: u64,
    },

    #[error(
        "RPC endpoint {redacted_url} serves chain {actual}, but this transaction targets chain {expected}. Refusing to broadcast."
    )]
    ChainIdMismatch {
        redacted_url: String,
        expected: u64,
        actual: u64,
    },

    #[error("RPC transport error communicating with {redacted_url}: {message}")]
    Transport {
        redacted_url: String,
        message: String,
    },

    #[error("Failed to send transaction via {redacted_url}: {message}")]
    Send {
        redacted_url: String,
        message: String,
    },

    #[error(
        "Transaction {tx_hash} was submitted to chain {chain_id} but its confirmation state is unknown ({message}). Do not re-broadcast: the transaction may still confirm. Check it by hash first, and re-run the command only once its status is known."
    )]
    ConfirmationUnknown {
        tx_hash: B256,
        chain_id: u64,
        redacted_url: String,
        message: String,
    },

    #[error(
        "{redacted_url} could not judge the transaction after {attempts} attempt(s) over {waited_ms}ms ({message}). Nothing was signed or submitted and the nonce is unchanged: re-run the command."
    )]
    AssertionsUnavailable {
        chain_id: u64,
        redacted_url: String,
        attempts: u32,
        waited_ms: u128,
        message: String,
    },

    #[error(
        "Transaction {tx_hash} was submitted to chain {chain_id} {attempts} time(s) without confirmed acceptance ({message}). It may be in flight: check the hash before re-running, which would reuse the same nonce."
    )]
    SubmissionUnconfirmed {
        tx_hash: B256,
        chain_id: u64,
        redacted_url: String,
        attempts: u32,
        message: String,
    },

    #[error("Transaction {tx_hash} reverted on-chain (block {block})")]
    Reverted { tx_hash: B256, block: u64 },
}

/// Transaction arguments shared by every command that broadcasts.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct TxArgs {
    /// HTTP(S) RPC URL used to broadcast transactions
    #[arg(long, env = "PCL_RPC_URL")]
    pub rpc_url: Option<Url>,

    /// Confirmations to wait for after broadcasting (default: the platform-stated requirement when the calldata carries one, else a per-chain fallback; values below the requirement are rejected)
    #[arg(long)]
    pub confirmations: Option<u64>,

    /// Seconds to wait for the transaction to confirm before giving up
    #[arg(long, default_value_t = 300)]
    pub tx_timeout_secs: u64,
}

impl TxArgs {
    /// Resolves the RPC URL for a chain: flag/env first, then the per-chain
    /// config map.
    pub fn resolve_rpc(&self, config: &CliConfig, chain_id: u64) -> Result<Url, OnchainError> {
        if let Some(url) = &self.rpc_url {
            return Ok(url.clone());
        }
        let endpoint = config
            .rpc_endpoint(chain_id)
            .ok_or(OnchainError::RpcUrlMissing { chain_id })?;
        endpoint.url.parse().map_err(|e: url::ParseError| {
            OnchainError::InvalidRpcUrl {
                chain_id,
                redacted_url: crate::config::redacted_rpc_host(&endpoint.url),
                reason: e.to_string(),
            }
        })
    }

    /// Resolves how many confirmations to wait for: flag first, then the
    /// per-chain config, then the requirement itself. The requirement is the
    /// platform-stated value when the calldata response carries one
    /// (authoritative — the platform's chain profiles differ per deployment),
    /// falling back to [`fallback_confirmations`] otherwise. A flag or stored
    /// value below the requirement is rejected up front — the platform
    /// verifier would refuse the receipt after gas was spent.
    pub fn resolve_confirmations(
        &self,
        config: &CliConfig,
        chain_id: u64,
        platform_required: Option<u64>,
    ) -> Result<u64, OnchainError> {
        let required = platform_required.unwrap_or_else(|| fallback_confirmations(chain_id));
        let requested = self
            .confirmations
            .or_else(|| config.rpc_endpoint(chain_id).and_then(|e| e.confirmations));
        match requested {
            Some(requested) if requested < required => {
                Err(OnchainError::InsufficientConfirmations {
                    chain_id,
                    requested,
                    required,
                })
            }
            Some(requested) => Ok(requested),
            None => Ok(required),
        }
    }

    /// Transaction confirmation timeout.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.tx_timeout_secs)
    }
}

/// ABI-encodes a `StateOracle.batch(bytes[])` call around backend-provided
/// operation calldata.
pub fn encode_batch(calldata: Vec<Bytes>) -> Bytes {
    batchCall { data: calldata }.abi_encode().into()
}

/// Outcome of a confirmed transaction, surfaced in command envelopes.
#[derive(Debug, Clone, Serialize)]
pub struct TxOutcome {
    pub tx_hash: B256,
    pub chain_id: u64,
    pub block_number: Option<u64>,
    pub gas_used: u64,
    pub effective_gas_price: u128,
    pub confirmations_waited: u64,
}

/// Scrubs a provider/transport error display before it reaches an error
/// envelope: any occurrence of the RPC URL (which may embed API keys in
/// userinfo, path, or query) is replaced with its redacted origin.
fn scrub_rpc_error(text: &str, rpc: &Url) -> String {
    let redacted = crate::config::redacted_rpc_host(rpc.as_str());
    text.replace(rpc.as_str().trim_end_matches('/'), &redacted)
        .replace(rpc.as_str(), &redacted)
}

// Concrete rather than erased, so `fill` can build and sign before submitting.
type WalletProvider = FillProvider<
    JoinFill<JoinedRecommendedFillers, WalletFiller<EthereumWallet>>,
    RootProvider<Ethereum>,
>;

// Built once, so the hash is known before the first submission and identical on
// every retry.
struct PreparedTx {
    tx_hash: B256,
    raw: Vec<u8>,
}

/// A connected, chain-checked transaction sender.
pub struct TxSender {
    provider: WalletProvider,
    chain_id: u64,
    rpc: Url,
}

impl TxSender {
    /// Connects to `rpc` with `signer` and verifies the endpoint actually
    /// serves `expected_chain_id` before anything is signed.
    pub async fn connect(
        rpc: Url,
        signer: PrivateKeySigner,
        expected_chain_id: u64,
    ) -> Result<Self, OnchainError> {
        let provider = ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer))
            .connect_http(rpc.clone());
        let actual = provider.get_chain_id().await.map_err(|error| {
            OnchainError::Transport {
                redacted_url: crate::config::redacted_rpc_host(rpc.as_str()),
                message: scrub_rpc_error(&error.to_string(), &rpc),
            }
        })?;
        if actual != expected_chain_id {
            return Err(OnchainError::ChainIdMismatch {
                redacted_url: crate::config::redacted_rpc_host(rpc.as_str()),
                expected: expected_chain_id,
                actual,
            });
        }
        Ok(Self {
            provider,
            chain_id: expected_chain_id,
            rpc,
        })
    }

    /// The chain id this sender was validated against.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// Sends `data` to `to`, waits for `confirmations`, and errors if the
    /// receipt reports a revert. Nonce, gas and fees are filled by the
    /// provider (transactions are strictly sequential in pcl).
    pub async fn send_and_confirm(
        &self,
        request: TransactionRequest,
        confirmations: u64,
        timeout: Duration,
        notify: &dyn Fn(&str),
    ) -> Result<TxOutcome, OnchainError> {
        let prepared = self.prepare(request, notify).await?;
        self.submit(&prepared, notify).await?;
        notify("Transaction submitted; waiting for confirmation");
        let receipt = self
            .await_receipt(prepared.tx_hash, confirmations, timeout)
            .await?;

        if !receipt.status() {
            return Err(OnchainError::Reverted {
                tx_hash: receipt.transaction_hash,
                block: receipt.block_number.unwrap_or_default(),
            });
        }

        Ok(TxOutcome {
            tx_hash: receipt.transaction_hash,
            chain_id: self.chain_id,
            block_number: receipt.block_number,
            gas_used: receipt.gas_used,
            effective_gas_price: receipt.effective_gas_price,
            confirmations_waited: confirmations,
        })
    }

    // `eth_estimateGas` is gated too, so filling hits the same window; retrying
    // costs nothing while nothing is signed.
    async fn prepare(
        &self,
        request: TransactionRequest,
        notify: &dyn Fn(&str),
    ) -> Result<PreparedTx, OnchainError> {
        let started = Instant::now();
        let attempts = ALIGNMENT_ATTEMPTS;
        let mut backoff = Backoff::new();
        let mut message = String::new();
        for attempt in 1..=attempts {
            match self.provider.fill(request.clone()).await {
                Ok(filled) => return self.signed(filled),
                Err(error) if assertions_unavailable(&error) => {
                    message = self.scrub(&error);
                    backoff
                        .wait(
                            attempt,
                            attempts,
                            "Credible layer is realigning while preparing the transaction",
                            notify,
                        )
                        .await;
                }
                Err(error) => {
                    return Err(OnchainError::Send {
                        redacted_url: self.redacted(),
                        message: self.scrub(&error),
                    });
                }
            }
        }
        Err(OnchainError::AssertionsUnavailable {
            chain_id: self.chain_id,
            redacted_url: self.redacted(),
            attempts,
            waited_ms: started.elapsed().as_millis(),
            message,
        })
    }

    fn signed(&self, filled: SendableTx<Ethereum>) -> Result<PreparedTx, OnchainError> {
        let envelope = filled.try_into_envelope().map_err(|unsigned| {
            OnchainError::Send {
                redacted_url: self.redacted(),
                message: format!("wallet did not sign the filled transaction: {unsigned}"),
            }
        })?;
        Ok(PreparedTx {
            tx_hash: *envelope.tx_hash(),
            raw: envelope.encoded_2718(),
        })
    }

    // Every attempt sends the same hash and nonce, so a node already holding the
    // transaction deduplicates the retry instead of accepting a second one.
    async fn submit(
        &self,
        prepared: &PreparedTx,
        notify: &dyn Fn(&str),
    ) -> Result<(), OnchainError> {
        let attempts = ALIGNMENT_ATTEMPTS;
        let mut backoff = Backoff::new();
        let mut message = String::new();
        for attempt in 1..=attempts {
            match self.provider.send_raw_transaction(&prepared.raw).await {
                Ok(_) => return Ok(()),
                // The node already holds these exact bytes, so an earlier
                // attempt reached it even if its answer did not reach us.
                Err(error) if already_submitted(&error) => return Ok(()),
                Err(error) if assertions_unavailable(&error) => {
                    message = self.scrub(&error);
                    backoff
                        .wait(
                            attempt,
                            attempts,
                            "Credible layer is realigning while submitting the transaction",
                            notify,
                        )
                        .await;
                }
                Err(error) => {
                    return Err(OnchainError::Send {
                        redacted_url: self.redacted(),
                        message: self.scrub(&error),
                    });
                }
            }
        }
        Err(OnchainError::SubmissionUnconfirmed {
            tx_hash: prepared.tx_hash,
            chain_id: self.chain_id,
            redacted_url: self.redacted(),
            attempts,
            message,
        })
    }

    // Waits for `tx_hash` to reach `confirmations`.
    async fn await_receipt(
        &self,
        tx_hash: B256,
        confirmations: u64,
        timeout: Duration,
    ) -> Result<TransactionReceipt, OnchainError> {
        PendingTransactionBuilder::new(self.provider.root().clone(), tx_hash)
            .with_required_confirmations(confirmations)
            .with_timeout(Some(timeout))
            .get_receipt()
            .await
            .map_err(|source: PendingTransactionError| {
                OnchainError::ConfirmationUnknown {
                    tx_hash,
                    chain_id: self.chain_id,
                    redacted_url: self.redacted(),
                    message: scrub_rpc_error(&source.to_string(), &self.rpc),
                }
            })
    }

    fn redacted(&self) -> String {
        crate::config::redacted_rpc_host(self.rpc.as_str())
    }

    fn scrub(&self, error: &TransportError) -> String {
        scrub_rpc_error(&error.to_string(), &self.rpc)
    }
}

/// Doubling delay between attempts at a credible-layer-gated step.
struct Backoff {
    delay: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            delay: ALIGNMENT_FIRST_DELAY,
        }
    }

    /// The final attempt has nothing left to wait for.
    async fn wait(&mut self, attempt: u32, attempts: u32, status: &str, notify: &dyn Fn(&str)) {
        if attempt >= attempts {
            return;
        }
        notify(&format!(
            "{status}; retrying in {}ms ({attempt}/{attempts})",
            self.delay.as_millis()
        ));
        tokio::time::sleep(self.delay).await;
        self.delay = (self.delay * 2).min(ALIGNMENT_MAX_DELAY);
    }
}

/// Whether the credible layer refused to judge the call because it has no
/// assertion state aligned with the block it would judge against — transient by
/// construction.
///
/// Matched on the message, not the code: the refusal reuses the generic
/// internal-error code, which would sweep in unrelated failures. An assertion
/// rejection is a revert (code 3) and is never transient, so it cannot match.
fn assertions_unavailable(error: &TransportError) -> bool {
    const REVERT: i64 = 3;
    let RpcError::ErrorResp(payload) = error else {
        return false;
    };
    let message = payload.message.to_ascii_lowercase();
    payload.code != REVERT && message.contains("credible layer") && message.contains("unavailable")
}

/// Whether the node is telling us it already holds these exact bytes. reth and
/// geth both answer a resubmission this way, which is the success case when a
/// refusal was raised after the transaction had already been forwarded.
fn already_submitted(error: &TransportError) -> bool {
    let RpcError::ErrorResp(payload) = error else {
        return false;
    };
    let message = payload.message.to_ascii_lowercase();
    ["already known", "already imported", "already exists"]
        .iter()
        .any(|known| message.contains(known))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RpcEndpoint;
    use alloy_primitives::{
        address,
        hex,
    };
    use mockito::{
        Matcher,
        Mock,
        ServerGuard,
    };
    use serde_json::{
        Value,
        json,
    };

    fn config_with_rpc(chain_id: u64, url: &str, confirmations: Option<u64>) -> CliConfig {
        let mut config = CliConfig::default();
        config.rpc.insert(
            chain_id.to_string(),
            RpcEndpoint {
                url: url.to_string(),
                confirmations,
            },
        );
        config
    }

    #[test]
    fn encode_batch_produces_expected_selector_and_layout() {
        // Golden vector: cast calldata "batch(bytes[])" "[0x1234,0x56]"
        let encoded = encode_batch(vec![
            Bytes::from(hex!("1234").to_vec()),
            Bytes::from(hex!("56").to_vec()),
        ]);
        let expected = hex!(
            "1e897afb"
            "0000000000000000000000000000000000000000000000000000000000000020"
            "0000000000000000000000000000000000000000000000000000000000000002"
            "0000000000000000000000000000000000000000000000000000000000000040"
            "0000000000000000000000000000000000000000000000000000000000000080"
            "0000000000000000000000000000000000000000000000000000000000000002"
            "1234000000000000000000000000000000000000000000000000000000000000"
            "0000000000000000000000000000000000000000000000000000000000000001"
            "5600000000000000000000000000000000000000000000000000000000000000"
        );
        // Selector sanity: first 4 bytes are keccak("batch(bytes[])")[..4].
        assert_eq!(&encoded[..4], &batchCall::SELECTOR);
        assert_eq!(encoded.as_ref(), expected.as_slice());
    }

    #[test]
    fn encode_batch_empty_array() {
        let encoded = encode_batch(Vec::new());
        assert_eq!(&encoded[..4], &batchCall::SELECTOR);
        // offset + zero length
        assert_eq!(encoded.len(), 4 + 32 + 32);
    }

    #[test]
    fn resolve_rpc_prefers_flag_over_config() {
        let config = config_with_rpc(1, "https://config.example", None);
        let args = TxArgs {
            rpc_url: Some("https://flag.example".parse().unwrap()),
            ..Default::default()
        };
        assert_eq!(
            args.resolve_rpc(&config, 1).unwrap().as_str(),
            "https://flag.example/"
        );
    }

    #[test]
    fn resolve_rpc_falls_back_to_config_map() {
        let config = config_with_rpc(84532, "https://sepolia.base.org", None);
        let args = TxArgs::default();
        assert_eq!(
            args.resolve_rpc(&config, 84532).unwrap().as_str(),
            "https://sepolia.base.org/"
        );
    }

    #[test]
    fn resolve_rpc_errors_when_unconfigured() {
        let err = TxArgs::default()
            .resolve_rpc(&CliConfig::default(), 7)
            .unwrap_err();
        assert!(matches!(err, OnchainError::RpcUrlMissing { chain_id: 7 }));
        assert!(err.to_string().contains("pcl config set-rpc 7"));
    }

    #[test]
    fn resolve_rpc_reports_invalid_stored_url_without_leaking_it() {
        let config = config_with_rpc(1, "not a url with secret_key_123", None);
        let err = TxArgs::default().resolve_rpc(&config, 1).unwrap_err();
        assert!(matches!(err, OnchainError::InvalidRpcUrl { .. }));
        assert!(!err.to_string().contains("secret_key_123"));
    }

    #[test]
    fn resolve_confirmations_precedence_and_floor() {
        // Chain 1 falls back to a 12-confirmation floor.
        let config = config_with_rpc(1, "https://x.example", Some(15));

        // A flag at or above the requirement wins.
        let flag = TxArgs {
            confirmations: Some(20),
            ..Default::default()
        };
        assert_eq!(flag.resolve_confirmations(&config, 1, None).unwrap(), 20);

        // Then the stored per-chain value.
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&config, 1, None)
                .unwrap(),
            15
        );

        // Then the requirement itself.
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 1, None)
                .unwrap(),
            12
        );

        // Values below the requirement are rejected up front, whether they
        // come from the flag or the stored config.
        let low_flag = TxArgs {
            confirmations: Some(1),
            ..Default::default()
        };
        assert!(matches!(
            low_flag.resolve_confirmations(&config, 1, None),
            Err(OnchainError::InsufficientConfirmations {
                chain_id: 1,
                requested: 1,
                required: 12,
            })
        ));
        let low_config = config_with_rpc(1, "https://x.example", Some(3));
        assert!(matches!(
            TxArgs::default().resolve_confirmations(&low_config, 1, None),
            Err(OnchainError::InsufficientConfirmations { .. })
        ));

        // Local devnets and testnets have lower fallback floors.
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 31337, None)
                .unwrap(),
            1
        );
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 84532, None)
                .unwrap(),
            3
        );
        // Unknown chains get the conservative mainnet floor.
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 999_999, None)
                .unwrap(),
            12
        );
    }

    #[test]
    fn resolve_confirmations_platform_requirement_overrides_the_fallback_table() {
        // The platform's chain profile is authoritative in both directions:
        // an internal profile can require 3 on a devnet chain the table maps
        // to 1, and an external one can require 6 where the table says 3.
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 1337, Some(3))
                .unwrap(),
            3
        );
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 84532, Some(6))
                .unwrap(),
            6
        );
        // A platform requirement below the table is honored too — the table
        // is only a guess when the platform says nothing.
        assert_eq!(
            TxArgs::default()
                .resolve_confirmations(&CliConfig::default(), 1, Some(2))
                .unwrap(),
            2
        );
        // Explicit values below the platform requirement are rejected.
        let low_flag = TxArgs {
            confirmations: Some(1),
            ..Default::default()
        };
        assert!(matches!(
            low_flag.resolve_confirmations(&CliConfig::default(), 1337, Some(3)),
            Err(OnchainError::InsufficientConfirmations {
                chain_id: 1337,
                requested: 1,
                required: 3,
            })
        ));
    }

    #[test]
    fn rpc_errors_never_contain_the_stored_url_secrets() {
        let rpc: Url = "https://user:hunter2@rpc.example.com/v2/apikey123?token=sekrit"
            .parse()
            .unwrap();

        let scrubbed = scrub_rpc_error(&format!("connection refused connecting to {rpc}"), &rpc);
        assert!(!scrubbed.contains("hunter2"));
        assert!(!scrubbed.contains("apikey123"));
        assert!(!scrubbed.contains("sekrit"));
        assert!(scrubbed.contains("https://rpc.example.com"));

        let mismatch = OnchainError::ChainIdMismatch {
            redacted_url: crate::config::redacted_rpc_host(rpc.as_str()),
            expected: 1,
            actual: 2,
        };
        let rendered = mismatch.to_string();
        assert!(!rendered.contains("hunter2"));
        assert!(!rendered.contains("apikey123"));
        assert!(rendered.contains("https://rpc.example.com"));
    }

    #[test]
    fn confirmation_unknown_is_scrubbed_and_warns_against_rebroadcast() {
        let rpc: Url = "https://user:hunter2@rpc.example.com/v2/apikey123"
            .parse()
            .unwrap();
        // A receipt-poll transport error typically echoes the full RPC URL.
        let error = OnchainError::ConfirmationUnknown {
            tx_hash: B256::repeat_byte(0xab),
            chain_id: 84532,
            redacted_url: crate::config::redacted_rpc_host(rpc.as_str()),
            message: scrub_rpc_error(&format!("request timeout connecting to {rpc}"), &rpc),
        };
        let rendered = error.to_string();
        assert!(!rendered.contains("hunter2"));
        assert!(!rendered.contains("apikey123"));
        // The transaction was already accepted: the hash must be present and
        // the guidance must be to check it, never to send again.
        assert!(rendered.contains(&B256::repeat_byte(0xab).to_string()));
        assert!(rendered.contains("Do not re-broadcast"));
    }

    #[tokio::test]
    async fn a_receipt_that_never_arrives_stops_at_the_configured_timeout() {
        let mut server = mockito::Server::new_async().await;
        ok(&mut server, "eth_chainId", json!(format!("{CHAIN_ID:#x}"))).await;
        ok(&mut server, "eth_getTransactionReceipt", Value::Null).await;
        ok(&mut server, "eth_blockNumber", json!("0x1")).await;
        let sender = TxSender::connect(server.url().parse().unwrap(), signer(), CHAIN_ID)
            .await
            .unwrap();
        let started = Instant::now();
        let timeout = Duration::from_millis(50);

        let error = sender
            .await_receipt(RECEIPT_HASH, 1, timeout)
            .await
            .unwrap_err();

        assert!(matches!(error, OnchainError::ConfirmationUnknown { .. }));
        assert!(started.elapsed() >= timeout);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    const CHAIN_ID: u64 = 31337;
    /// Verbatim from the Credible RPC gate (`src/simulation/error.rs`).
    const UNAVAILABLE: &str = "credible layer: assertions are unavailable, try again shortly";
    const INTERNAL: i64 = -32603;
    const MINED_BLOCK: u64 = 0x10;
    const RECEIPT_HASH: B256 = B256::repeat_byte(0xcd);

    fn signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap()
    }

    /// Mocks are matched in creation order, and one that has met its `expect`
    /// count is skipped, so two mocks for the same method answer in sequence.
    async fn rpc(server: &mut ServerGuard, method: &str, response: Value) -> Mock {
        server
            .mock("POST", "/")
            .match_body(Matcher::PartialJson(json!({ "method": method })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .create_async()
            .await
    }

    async fn ok(server: &mut ServerGuard, method: &str, result: Value) -> Mock {
        rpc(
            server,
            method,
            json!({ "jsonrpc": "2.0", "id": 1, "result": result }),
        )
        .await
    }

    async fn fails(server: &mut ServerGuard, method: &str, code: i64, message: &str) -> Mock {
        rpc(
            server,
            method,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": { "code": code, "message": message },
            }),
        )
        .await
    }

    /// The ungated reads a fill and a receipt wait perform.
    async fn chain(server: &mut ServerGuard) {
        ok(server, "eth_chainId", json!("0x7a69")).await;
        ok(
            server,
            "eth_blockNumber",
            json!(format!("{MINED_BLOCK:#x}")),
        )
        .await;
        ok(
            server,
            "eth_feeHistory",
            json!({
                "oldestBlock": "0xe",
                "baseFeePerGas": ["0x3b9aca00", "0x3b9aca00", "0x3b9aca00"],
                "gasUsedRatio": [0.5, 0.5],
                "reward": [["0x1"], ["0x1"]],
            }),
        )
        .await;
        ok(server, "eth_getTransactionReceipt", receipt()).await;
    }

    fn receipt() -> Value {
        json!({
            "type": "0x2",
            "status": "0x1",
            "cumulativeGasUsed": "0x5208",
            "logs": [],
            "logsBloom": format!("0x{}", "0".repeat(512)),
            "transactionHash": RECEIPT_HASH,
            "transactionIndex": "0x0",
            "blockHash": B256::repeat_byte(0xbb),
            "blockNumber": format!("{MINED_BLOCK:#x}"),
            "gasUsed": "0x5208",
            "effectiveGasPrice": "0x3b9aca00",
            "from": signer().address(),
            "to": address!("0202020202020202020202020202020202020202"),
            "contractAddress": Value::Null,
        })
    }

    async fn send(server: &ServerGuard) -> Result<TxOutcome, OnchainError> {
        let sender = TxSender::connect(server.url().parse().unwrap(), signer(), CHAIN_ID).await?;
        sender
            .send_and_confirm(
                TransactionRequest::default()
                    .to(address!("0202020202020202020202020202020202020202"))
                    .input(Bytes::from_static(&[0x12, 0x34]).into()),
                1,
                Duration::from_secs(2),
                &|message: &str| {
                    if message == "Transaction submitted; waiting for confirmation" {
                        tokio::time::resume();
                    }
                },
            )
            .await
    }

    #[tokio::test(start_paused = true)]
    async fn a_refused_submission_resends_the_same_signed_transaction() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        // Exactly one fill: a resubmission must reuse the signed envelope, not
        // rebuild it, or it could take a second nonce.
        let nonce = ok(&mut server, "eth_getTransactionCount", json!("0x8"))
            .await
            .expect(1);
        let gas = ok(&mut server, "eth_estimateGas", json!("0x5208"))
            .await
            .expect(1);
        let refused = fails(&mut server, "eth_sendRawTransaction", INTERNAL, UNAVAILABLE)
            .await
            .expect(1);
        let accepted = ok(&mut server, "eth_sendRawTransaction", json!(RECEIPT_HASH))
            .await
            .expect(1);
        let outcome = send(&server).await.unwrap();

        assert_eq!(outcome.tx_hash, RECEIPT_HASH);
        assert_eq!(outcome.block_number, Some(MINED_BLOCK));
        nonce.assert_async().await;
        gas.assert_async().await;
        refused.assert_async().await;
        accepted.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_deduplicated_resend_counts_as_submitted() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        ok(&mut server, "eth_estimateGas", json!("0x5208")).await;
        fails(&mut server, "eth_sendRawTransaction", INTERNAL, UNAVAILABLE)
            .await
            .expect(1);
        ok(&mut server, "eth_getTransactionByHash", Value::Null).await;
        // The resend arrives after the pool took the first copy: same bytes,
        // same hash, so the node deduplicates instead of queueing a second one.
        let dedup = fails(
            &mut server,
            "eth_sendRawTransaction",
            -32000,
            "already known",
        )
        .await
        .expect(1);

        let outcome = send(&server).await.unwrap();

        assert_eq!(outcome.tx_hash, RECEIPT_HASH);
        dedup.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refused_gas_estimation_is_retried_before_anything_is_signed() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        let refused = fails(&mut server, "eth_estimateGas", INTERNAL, UNAVAILABLE)
            .await
            .expect(1);
        let estimated = ok(&mut server, "eth_estimateGas", json!("0x5208"))
            .await
            .expect(1);
        let submitted = ok(&mut server, "eth_sendRawTransaction", json!(RECEIPT_HASH))
            .await
            .expect(1);

        assert_eq!(send(&server).await.unwrap().tx_hash, RECEIPT_HASH);
        refused.assert_async().await;
        estimated.assert_async().await;
        submitted.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_lasting_refusal_before_signing_reports_that_nothing_was_submitted() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        fails(&mut server, "eth_estimateGas", INTERNAL, UNAVAILABLE).await;
        let never = ok(&mut server, "eth_sendRawTransaction", json!(RECEIPT_HASH))
            .await
            .expect(0);

        let error = send(&server).await.unwrap_err();

        assert!(matches!(
            error,
            OnchainError::AssertionsUnavailable {
                chain_id: CHAIN_ID,
                attempts: ALIGNMENT_ATTEMPTS,
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .contains("Nothing was signed or submitted")
        );
        never.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_lasting_refusal_after_signing_reports_the_signed_hash() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        ok(&mut server, "eth_estimateGas", json!("0x5208")).await;
        fails(&mut server, "eth_sendRawTransaction", INTERNAL, UNAVAILABLE).await;
        ok(&mut server, "eth_getTransactionByHash", Value::Null).await;

        let error = send(&server).await.unwrap_err();

        // The hash is what makes this recoverable: the transaction may be
        // upstream, so the operator needs it before deciding anything.
        let OnchainError::SubmissionUnconfirmed { tx_hash, .. } = &error else {
            panic!("expected an unconfirmed submission, got {error:?}");
        };
        assert!(error.to_string().contains(&tx_hash.to_string()));
        assert!(error.to_string().contains("may be in flight"));
    }

    #[tokio::test(start_paused = true)]
    async fn an_assertion_rejection_is_never_retried() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        ok(&mut server, "eth_estimateGas", json!("0x5208")).await;
        // A rejection is a verdict about the transaction, not about the node:
        // it names the credible layer but resending can only fail again.
        let rejected = fails(
            &mut server,
            "eth_sendRawTransaction",
            3,
            "execution reverted: credible layer: transaction rejected by an assertion",
        )
        .await
        .expect(1);

        let error = send(&server).await.unwrap_err();

        assert!(matches!(error, OnchainError::Send { .. }));
        rejected.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_ordinary_rejection_is_not_retried() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        ok(&mut server, "eth_estimateGas", json!("0x5208")).await;
        let rejected = fails(
            &mut server,
            "eth_sendRawTransaction",
            -32000,
            "insufficient funds for gas * price + value",
        )
        .await
        .expect(1);
        // No hash probe either: nothing was signed into flight.
        let probe = ok(&mut server, "eth_getTransactionByHash", Value::Null)
            .await
            .expect(0);

        assert!(matches!(
            send(&server).await.unwrap_err(),
            OnchainError::Send { .. }
        ));
        rejected.assert_async().await;
        probe.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_stale_nonce_on_the_first_attempt_is_not_retried() {
        let mut server = mockito::Server::new_async().await;
        chain(&mut server).await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        ok(&mut server, "eth_estimateGas", json!("0x5208")).await;
        // Another transaction took the nonce before this one was submitted, so
        // this envelope can never land; only a resubmission could have spent
        // its own nonce.
        let rejected = fails(
            &mut server,
            "eth_sendRawTransaction",
            -32000,
            "nonce too low",
        )
        .await
        .expect(1);

        assert!(matches!(
            send(&server).await.unwrap_err(),
            OnchainError::Send { .. }
        ));
        rejected.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_reverted_transaction_still_reports_its_receipt() {
        let mut server = mockito::Server::new_async().await;
        ok(&mut server, "eth_chainId", json!("0x7a69")).await;
        ok(
            &mut server,
            "eth_blockNumber",
            json!(format!("{MINED_BLOCK:#x}")),
        )
        .await;
        ok(
            &mut server,
            "eth_feeHistory",
            json!({
                "oldestBlock": "0xe",
                "baseFeePerGas": ["0x3b9aca00", "0x3b9aca00", "0x3b9aca00"],
                "gasUsedRatio": [0.5, 0.5],
                "reward": [["0x1"], ["0x1"]],
            }),
        )
        .await;
        ok(&mut server, "eth_getTransactionCount", json!("0x8")).await;
        ok(&mut server, "eth_estimateGas", json!("0x5208")).await;
        ok(&mut server, "eth_sendRawTransaction", json!(RECEIPT_HASH)).await;
        let mut reverted = receipt();
        reverted["status"] = json!("0x0");
        ok(&mut server, "eth_getTransactionReceipt", reverted).await;

        assert!(matches!(
            send(&server).await.unwrap_err(),
            OnchainError::Reverted {
                tx_hash: RECEIPT_HASH,
                block: MINED_BLOCK,
            }
        ));
    }
}
