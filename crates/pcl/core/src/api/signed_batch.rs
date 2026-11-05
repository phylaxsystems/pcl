//! Binds removal and manager-accept transactions to the active `StateOracle`
//! before pcl signs them.
//!
//! The dapp signs the API's `{to, data}` bytes directly
//! (`sendTransaction({to, data})`). Those bytes are a `StateOracle.batch`
//! whose entries are `removeAssertion` or `acceptManagerTransfer`. The
//! addresses below are the `status: "active"` entries from the chain profiles
//! compiled into the ethereum (`eth-mainnet`) and linea (`linea-mainnet`)
//! dapp deployments.

use super::ApiCommandError;
use alloy_primitives::{
    Address,
    B256,
    Bytes,
    address,
};
use alloy_sol_types::{
    SolCall,
    sol,
};
use serde_json::Value;

sol! {
    function removeAssertion(address contractAddress, bytes32 assertionId);
    function acceptManagerTransfer(address contractAddress);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OracleEnvironment {
    Production,
    Staging,
}

impl OracleEnvironment {
    fn as_str(self) -> &'static str {
        match self {
            Self::Production => "production",
            Self::Staging => "staging",
        }
    }
}

pub(crate) fn release_environment(body: &Value) -> Result<OracleEnvironment, ApiCommandError> {
    match body.get("environment").and_then(Value::as_str) {
        Some("production") => Ok(OracleEnvironment::Production),
        Some("staging") => Ok(OracleEnvironment::Staging),
        Some(other) => {
            Err(ApiCommandError::UnexpectedResponse {
                endpoint: "/projects/{project_id}/releases/{release_id}",
                reason: format!("unknown release environment {other:?}"),
            })
        }
        None => {
            Err(ApiCommandError::UnexpectedResponse {
                endpoint: "/projects/{project_id}/releases/{release_id}",
                reason: "missing environment".to_string(),
            })
        }
    }
}

/// Active `StateOracle` addresses compiled from the ethereum and linea dapp chain
/// profiles. Only entries whose profile status is `active` are listed.
/// A host that is not here has no oracle pcl is willing to sign against.
const ACTIVE_ORACLES: &[(&str, u64, OracleEnvironment, Address)] = &[
    (
        "ethereum.phylax.systems",
        1,
        OracleEnvironment::Production,
        address!("8681e3fF0526C785FE82db9153E6E1F54551ebdF"),
    ),
    (
        "ethereum.phylax.systems",
        1,
        OracleEnvironment::Staging,
        address!("B5E1b58638E05a69c853dEf16837D651B7EEeAA2"),
    ),
    (
        "linea.phylax.systems",
        59_144,
        OracleEnvironment::Production,
        address!("984Ed2269Cdcc0e105675b4a16120Bc0531Bb218"),
    ),
    (
        "linea.phylax.systems",
        59_144,
        OracleEnvironment::Staging,
        address!("c4AB38f642b1ABC38e40f30C60B49663D6c426a3"),
    ),
];

/// Active `StateOracle` for one platform host, chain, and release environment.
/// `None` means pcl has no compiled oracle and must not sign.
pub(crate) fn active_state_oracle(
    platform_host: &str,
    chain_id: u64,
    environment: OracleEnvironment,
) -> Option<Address> {
    ACTIVE_ORACLES
        .iter()
        .find_map(|(host, oracle_chain, oracle_env, oracle)| {
            (host.eq_ignore_ascii_case(platform_host)
                && *oracle_chain == chain_id
                && *oracle_env == environment)
                .then_some(*oracle)
        })
}

fn active_oracles_for_chain(platform_host: &str, chain_id: u64) -> Vec<Address> {
    ACTIVE_ORACLES
        .iter()
        .filter(|(host, oracle_chain, _, _)| {
            host.eq_ignore_ascii_case(platform_host) && *oracle_chain == chain_id
        })
        .map(|(_, _, _, oracle)| *oracle)
        .collect()
}

pub(crate) fn validate_removal_batch(
    platform_host: &str,
    chain_id: u64,
    environment: OracleEnvironment,
    to: Address,
    data: &Bytes,
    assertions: &[Value],
) -> Result<(), ApiCommandError> {
    let Some(expected) = active_state_oracle(platform_host, chain_id, environment) else {
        return Err(unexpected(
            "remove-calldata",
            format!(
                "no active StateOracle for {platform_host} chain {chain_id} {}",
                environment.as_str()
            ),
        ));
    };
    if to != expected {
        return Err(unexpected(
            "remove-calldata",
            format!(
                "transaction target {to} is not the active StateOracle {expected} for {platform_host} chain {chain_id} {}",
                environment.as_str()
            ),
        ));
    }

    let targets = removal_targets(assertions)?;
    let entries = decode_entries("remove-calldata", data)?;
    if entries.len() != targets.len() {
        return Err(unexpected(
            "remove-calldata",
            format!(
                "batch has {} calls and the release lists {} assertions",
                entries.len(),
                targets.len()
            ),
        ));
    }
    for (index, (entry, (contract, assertion_id))) in entries.iter().zip(targets).enumerate() {
        let Ok(call) = removeAssertionCall::abi_decode(entry) else {
            return Err(unexpected(
                "remove-calldata",
                format!("batch entry {index} is not removeAssertion"),
            ));
        };
        if call.contractAddress != contract || call.assertionId != assertion_id {
            return Err(unexpected(
                "remove-calldata",
                format!("batch entry {index} does not match the release assertion"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_accept_batch(
    platform_host: &str,
    chain_id: u64,
    to: Address,
    data: &Bytes,
    contracts: &[(Address, Bytes)],
) -> Result<(), ApiCommandError> {
    let expected = active_oracles_for_chain(platform_host, chain_id);
    if expected.is_empty() {
        return Err(unexpected(
            "protocol-manager/accept-calldata",
            format!("no active StateOracle for {platform_host} chain {chain_id}"),
        ));
    }
    if !expected.contains(&to) {
        let listed = expected
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(unexpected(
            "protocol-manager/accept-calldata",
            format!(
                "transaction target {to} is not an active StateOracle ({listed}) for {platform_host} chain {chain_id}"
            ),
        ));
    }

    let entries = decode_entries("protocol-manager/accept-calldata", data)?;
    if entries.len() != contracts.len() {
        return Err(unexpected(
            "protocol-manager/accept-calldata",
            format!(
                "batch has {} calls and the response lists {} contracts",
                entries.len(),
                contracts.len()
            ),
        ));
    }
    for (index, (entry, (contract, inner))) in entries.iter().zip(contracts).enumerate() {
        if entry != inner {
            return Err(unexpected(
                "protocol-manager/accept-calldata",
                format!("batch entry {index} does not match the contract calldata"),
            ));
        }
        let Ok(call) = acceptManagerTransferCall::abi_decode(entry) else {
            return Err(unexpected(
                "protocol-manager/accept-calldata",
                format!("batch entry {index} is not acceptManagerTransfer"),
            ));
        };
        if call.contractAddress != *contract {
            return Err(unexpected(
                "protocol-manager/accept-calldata",
                format!("batch entry {index} does not match the contract address"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn removal_targets(
    assertions: &[Value],
) -> Result<Vec<(Address, B256)>, ApiCommandError> {
    let mut targets = Vec::with_capacity(assertions.len());
    for (index, assertion) in assertions.iter().enumerate() {
        let Some(object) = assertion.as_object() else {
            return Err(unexpected(
                "remove-calldata",
                format!("assertion {index} is not an object"),
            ));
        };
        let contract = object
            .get("contractAddress")
            .or_else(|| object.get("contract_address"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                unexpected(
                    "remove-calldata",
                    format!("assertion {index} is missing contractAddress"),
                )
            })?;
        let contract: Address = contract.parse().map_err(|_| {
            unexpected(
                "remove-calldata",
                format!("assertion {index} has an invalid contract address"),
            )
        })?;
        let assertion_id = object
            .get("assertionId")
            .or_else(|| object.get("assertion_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                unexpected(
                    "remove-calldata",
                    format!("assertion {index} is missing assertionId"),
                )
            })?;
        let assertion_id: B256 = assertion_id.parse().map_err(|_| {
            unexpected(
                "remove-calldata",
                format!("assertion {index} has an invalid assertionId"),
            )
        })?;
        targets.push((contract, assertion_id));
    }
    Ok(targets)
}

fn decode_entries(endpoint: &'static str, data: &Bytes) -> Result<Vec<Bytes>, ApiCommandError> {
    crate::onchain::decode_batch(data).map_err(|reason| unexpected(endpoint, reason))
}

fn unexpected(endpoint: &'static str, reason: String) -> ApiCommandError {
    ApiCommandError::UnexpectedResponse { endpoint, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onchain::encode_batch;
    use alloy_primitives::{
        address,
        b256,
    };
    use serde_json::json;

    const ETH: &str = "ethereum.phylax.systems";
    const ETH_PRODUCTION: Address = address!("8681e3fF0526C785FE82db9153E6E1F54551ebdF");
    const ADOPTER: Address = address!("0101010101010101010101010101010101010101");
    const ASSERTION: B256 =
        b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    fn removal_data() -> Bytes {
        encode_batch(vec![
            removeAssertionCall {
                contractAddress: ADOPTER,
                assertionId: ASSERTION,
            }
            .abi_encode()
            .into(),
        ])
    }

    fn assertions() -> Vec<Value> {
        vec![json!({
            "contractAddress": format!("{ADOPTER}"),
            "assertionId": format!("{ASSERTION}"),
            "file": null,
        })]
    }

    #[test]
    fn removal_refuses_a_target_other_than_the_active_state_oracle() {
        let err = validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Production,
            address!("2222222222222222222222222222222222222222"),
            &removal_data(),
            &assertions(),
        )
        .unwrap_err();
        let ApiCommandError::UnexpectedResponse { reason, .. } = err else {
            panic!("expected unexpected response, got {err:?}");
        };
        assert!(
            reason.contains("8681e3fF0526C785FE82db9153E6E1F54551ebdF"),
            "{reason}"
        );
    }

    #[test]
    fn removal_accepts_a_batch_of_remove_assertion_calls() {
        validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &removal_data(),
            &assertions(),
        )
        .unwrap();
    }

    #[test]
    fn removal_refuses_a_different_function_inside_the_batch() {
        let data = encode_batch(vec![
            acceptManagerTransferCall {
                contractAddress: ADOPTER,
            }
            .abi_encode()
            .into(),
        ]);
        let err = validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &data,
            &assertions(),
        )
        .unwrap_err();
        assert!(matches!(err, ApiCommandError::UnexpectedResponse { .. }));
    }

    #[test]
    fn removal_refuses_an_unknown_platform() {
        let err = validate_removal_batch(
            "127.0.0.1",
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &removal_data(),
            &assertions(),
        )
        .unwrap_err();
        let ApiCommandError::UnexpectedResponse { reason, .. } = err else {
            panic!("expected unexpected response, got {err:?}");
        };
        assert!(reason.contains("127.0.0.1"), "{reason}");
    }

    #[test]
    fn accept_refuses_anything_other_than_accept_manager_transfer() {
        let linea = "linea.phylax.systems";
        let oracle = address!("984Ed2269Cdcc0e105675b4a16120Bc0531Bb218");
        let inner: Bytes = acceptManagerTransferCall {
            contractAddress: ADOPTER,
        }
        .abi_encode()
        .into();
        validate_accept_batch(
            linea,
            59144,
            oracle,
            &encode_batch(vec![inner.clone()]),
            &[(ADOPTER, inner)],
        )
        .unwrap();

        let wrong: Bytes = removeAssertionCall {
            contractAddress: ADOPTER,
            assertionId: ASSERTION,
        }
        .abi_encode()
        .into();
        let err = validate_accept_batch(
            linea,
            59144,
            oracle,
            &encode_batch(vec![wrong.clone()]),
            &[(ADOPTER, wrong)],
        )
        .unwrap_err();
        assert!(matches!(err, ApiCommandError::UnexpectedResponse { .. }));
    }

    #[test]
    fn retired_ethereum_oracle_is_not_active() {
        let retired = address!("B4951F9eA03C6bc7eE441E208C4AC996B2f6e369");
        let err = validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Production,
            retired,
            &removal_data(),
            &assertions(),
        )
        .unwrap_err();
        assert!(matches!(err, ApiCommandError::UnexpectedResponse { .. }));
    }

    #[test]
    fn raw_remove_assertion_without_a_batch_is_refused() {
        let naked: Bytes = removeAssertionCall {
            contractAddress: ADOPTER,
            assertionId: ASSERTION,
        }
        .abi_encode()
        .into();
        let err = validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &naked,
            &assertions(),
        )
        .unwrap_err();
        assert!(matches!(err, ApiCommandError::UnexpectedResponse { .. }));
    }

    #[test]
    fn removal_uses_the_staging_oracle_only_for_a_staging_release() {
        let staging = address!("B5E1b58638E05a69c853dEf16837D651B7EEeAA2");
        validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Staging,
            staging,
            &removal_data(),
            &assertions(),
        )
        .unwrap();

        let err = validate_removal_batch(
            ETH,
            1,
            OracleEnvironment::Production,
            staging,
            &removal_data(),
            &assertions(),
        )
        .unwrap_err();
        assert!(matches!(err, ApiCommandError::UnexpectedResponse { .. }));
    }

    #[test]
    fn release_environment_reads_the_release_record() {
        assert_eq!(
            release_environment(&json!({"environment": "staging"})).unwrap(),
            OracleEnvironment::Staging
        );
        assert!(release_environment(&json!({})).is_err());
    }
}
