//! Transaction broadcasting for on-chain Credible Layer operations.
//!
//! The backend computes all calldata (see the `*-calldata` API endpoints);
//! this module only signs and submits it — either as `StateOracle.batch(bytes[])`
//! or as a raw `{to, data}` transaction — and waits for confirmations.

use crate::config::CliConfig;
use alloy_network::EthereumWallet;
use alloy_primitives::{
    Address,
    B256,
    Bytes,
};
use alloy_provider::{
    DynProvider,
    PendingTransactionError,
    Provider,
    ProviderBuilder,
};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{
    SolCall,
    sol,
};
use serde::Serialize;
use std::time::Duration;
use thiserror::Error;
use url::Url;

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

/// A connected, chain-checked transaction sender.
pub struct TxSender {
    provider: DynProvider,
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
            provider: provider.erased(),
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
        to: Address,
        data: Bytes,
        confirmations: u64,
        timeout: Duration,
    ) -> Result<TxOutcome, OnchainError> {
        let request = TransactionRequest::default().to(to).input(data.into());

        let pending = self
            .provider
            .send_transaction(request)
            .await
            .map_err(|error| {
                OnchainError::Send {
                    redacted_url: crate::config::redacted_rpc_host(self.rpc.as_str()),
                    message: scrub_rpc_error(&error.to_string(), &self.rpc),
                }
            })?;
        let tx_hash = *pending.tx_hash();

        // A failure while waiting for the receipt does NOT mean the
        // transaction failed — it was already accepted by the RPC. Surface
        // that ambiguity explicitly (submitted, confirmation unknown) and
        // scrub the provider error, which can echo the credential-bearing
        // RPC URL.
        let receipt = pending
            .with_required_confirmations(confirmations)
            .with_timeout(Some(timeout))
            .get_receipt()
            .await
            .map_err(|source: PendingTransactionError| {
                OnchainError::ConfirmationUnknown {
                    tx_hash,
                    chain_id: self.chain_id,
                    redacted_url: crate::config::redacted_rpc_host(self.rpc.as_str()),
                    message: scrub_rpc_error(&source.to_string(), &self.rpc),
                }
            })?;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RpcEndpoint;
    use alloy_primitives::hex;

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
}
