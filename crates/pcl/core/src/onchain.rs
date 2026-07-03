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
    /// StateOracle's batch entrypoint; the only function pcl encodes locally.
    function batch(bytes[] calldata data) external;
}

/// Default confirmations to wait for when neither the flag nor the per-chain
/// config specifies one. The dapp uses 1 for local chains, 3 for testnets and
/// 12 for mainnets; pcl stays conservative-but-fast and lets users raise it.
const DEFAULT_CONFIRMATIONS: u64 = 1;

/// Errors that can occur while broadcasting transactions.
#[derive(Error, Debug)]
pub enum OnchainError {
    #[error(
        "No RPC endpoint for chain {chain_id}. Pass --rpc-url (or set PCL_RPC_URL), or store one with `pcl config set-rpc {chain_id} <url>`."
    )]
    RpcUrlMissing { chain_id: u64 },

    #[error("Invalid RPC URL {url:?} configured for chain {chain_id}: {reason}")]
    InvalidRpcUrl {
        chain_id: u64,
        url: String,
        reason: String,
    },

    #[error(
        "RPC endpoint {url} serves chain {actual}, but this transaction targets chain {expected}. Refusing to broadcast."
    )]
    ChainIdMismatch {
        url: Url,
        expected: u64,
        actual: u64,
    },

    #[error("RPC transport error: {0}")]
    Transport(#[source] alloy_provider::transport::TransportError),

    #[error("Failed to send transaction: {0}")]
    Send(
        #[source]
        alloy_provider::transport::RpcError<alloy_provider::transport::TransportErrorKind>,
    ),

    #[error("Transaction {tx_hash} was not confirmed: {source}")]
    Confirmation {
        tx_hash: B256,
        #[source]
        source: PendingTransactionError,
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

    /// Confirmations to wait for after broadcasting (default: per-chain config, else 1)
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
                url: endpoint.url.clone(),
                reason: e.to_string(),
            }
        })
    }

    /// Resolves how many confirmations to wait for: flag first, then the
    /// per-chain config, then [`DEFAULT_CONFIRMATIONS`].
    pub fn resolve_confirmations(&self, config: &CliConfig, chain_id: u64) -> u64 {
        self.confirmations
            .or_else(|| config.rpc_endpoint(chain_id).and_then(|e| e.confirmations))
            .unwrap_or(DEFAULT_CONFIRMATIONS)
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

/// A connected, chain-checked transaction sender.
pub struct TxSender {
    provider: DynProvider,
    chain_id: u64,
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
        let actual = provider
            .get_chain_id()
            .await
            .map_err(OnchainError::Transport)?;
        if actual != expected_chain_id {
            return Err(OnchainError::ChainIdMismatch {
                url: rpc,
                expected: expected_chain_id,
                actual,
            });
        }
        Ok(Self {
            provider: provider.erased(),
            chain_id: expected_chain_id,
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
            .map_err(OnchainError::Send)?;
        let tx_hash = *pending.tx_hash();

        let receipt = pending
            .with_required_confirmations(confirmations)
            .with_timeout(Some(timeout))
            .get_receipt()
            .await
            .map_err(|source| OnchainError::Confirmation { tx_hash, source })?;

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
    fn resolve_rpc_reports_invalid_stored_url() {
        let config = config_with_rpc(1, "not a url", None);
        let err = TxArgs::default().resolve_rpc(&config, 1).unwrap_err();
        assert!(matches!(err, OnchainError::InvalidRpcUrl { .. }));
    }

    #[test]
    fn resolve_confirmations_precedence() {
        let config = config_with_rpc(1, "https://x.example", Some(5));

        // flag wins
        let flag = TxArgs {
            confirmations: Some(9),
            ..Default::default()
        };
        assert_eq!(flag.resolve_confirmations(&config, 1), 9);

        // then config
        assert_eq!(TxArgs::default().resolve_confirmations(&config, 1), 5);

        // then default
        assert_eq!(
            TxArgs::default().resolve_confirmations(&CliConfig::default(), 1),
            DEFAULT_CONFIRMATIONS
        );

        // config entry without confirmations also falls back to default
        let bare = config_with_rpc(2, "https://y.example", None);
        assert_eq!(TxArgs::default().resolve_confirmations(&bare, 2), 1);
    }
}
