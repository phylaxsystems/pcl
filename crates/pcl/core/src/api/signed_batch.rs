//! Binds removal and manager-accept transactions to the active `StateOracle`
//! before pcl signs them.
//!
//! The dapp signs the API's `{to, data}` bytes directly
//! (`sendTransaction({to, data})`). pcl rebuilds the `StateOracle.batch` it
//! expects (`removeAssertion` or `acceptManagerTransfer` entries) from the
//! listed calls and signs only when the API's bytes are identical. The
//! addresses below are the `status: "active"` entries from the chain profiles
//! compiled into the ethereum (`eth-mainnet`) and linea (`linea-mainnet`)
//! dapp deployments.

use super::ApiCommandError;
use crate::platform::{
    Network,
    SELECTABLE_NETWORKS,
    Url,
    redact_platform_url,
};
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
/// A platform that is not here has no oracle pcl is willing to sign against.
const ACTIVE_ORACLES: &[(Network, u64, OracleEnvironment, Address)] = &[
    (
        Network::EthereumMainnet,
        1,
        OracleEnvironment::Production,
        address!("8681e3fF0526C785FE82db9153E6E1F54551ebdF"),
    ),
    (
        Network::EthereumMainnet,
        1,
        OracleEnvironment::Staging,
        address!("B5E1b58638E05a69c853dEf16837D651B7EEeAA2"),
    ),
    (
        Network::LineaMainnet,
        59_144,
        OracleEnvironment::Production,
        address!("984Ed2269Cdcc0e105675b4a16120Bc0531Bb218"),
    ),
    (
        Network::LineaMainnet,
        59_144,
        OracleEnvironment::Staging,
        address!("c4AB38f642b1ABC38e40f30C60B49663D6c426a3"),
    ),
];

/// Requires `to` to be an active `StateOracle` for the platform, chain and,
/// when known, environment. The platform must be a network's canonical
/// origin (`Network::from_url`), so another scheme, port, userinfo or a
/// trailing-dot host is refused.
fn check_target(
    platform: &Url,
    chain_id: u64,
    environment: Option<OracleEnvironment>,
    to: Address,
) -> Result<(), String> {
    let Some(network) = Network::from_url(platform) else {
        let supported = SELECTABLE_NETWORKS
            .map(|network| format!("https://{}", network.host()))
            .join(", ");
        return Err(format!(
            "{} is not a platform with a compiled StateOracle; pcl signs this transaction only for {supported}",
            redact_platform_url(platform)
        ));
    };
    let scope = format!(
        "{} chain {chain_id}{}",
        network.host(),
        environment.map_or(String::new(), |environment| {
            format!(" {}", environment.as_str())
        })
    );
    let active = ACTIVE_ORACLES
        .iter()
        .filter(|(oracle_network, oracle_chain, oracle_env, _)| {
            *oracle_network == network
                && *oracle_chain == chain_id
                && environment.is_none_or(|environment| *oracle_env == environment)
        })
        .map(|(_, _, _, oracle)| *oracle)
        .collect::<Vec<_>>();
    if active.is_empty() {
        return Err(format!("no active StateOracle for {scope}"));
    }
    if !active.contains(&to) {
        let active = active.iter().map(ToString::to_string).collect::<Vec<_>>();
        return Err(format!(
            "transaction target {to} is not the active StateOracle ({}) for {scope}",
            active.join(", ")
        ));
    }
    Ok(())
}

pub(crate) fn validate_removal_batch(
    platform: &Url,
    chain_id: u64,
    environment: OracleEnvironment,
    to: Address,
    data: &Bytes,
    assertions: &[Value],
    release: &[(Address, B256)],
) -> Result<(), String> {
    let targets = removal_targets(assertions)?;
    let mut listed = targets.clone();
    listed.sort_unstable();
    let mut held = release.to_vec();
    held.sort_unstable();
    if listed != held {
        return Err(
            "the listed assertions do not match the release record's config snapshot".to_string(),
        );
    }
    check_target(platform, chain_id, Some(environment), to)?;
    let expected = targets
        .into_iter()
        .map(|(contract, assertion_id)| {
            removeAssertionCall {
                contractAddress: contract,
                assertionId: assertion_id,
            }
            .abi_encode()
            .into()
        })
        .collect();
    check_batch(data, expected)
}

pub(crate) fn validate_accept_batch(
    platform: &Url,
    chain_id: u64,
    to: Address,
    data: &Bytes,
    contracts: &[(Address, Bytes)],
) -> Result<(), String> {
    // The accept response names no environment, and the dapp builds it for
    // whichever single environment holds the project's active contracts, so
    // either of the chain's active oracles may be the target.
    check_target(platform, chain_id, None, to)?;

    let mut expected = Vec::with_capacity(contracts.len());
    for (index, (contract, inner)) in contracts.iter().enumerate() {
        let call: Bytes = acceptManagerTransferCall {
            contractAddress: *contract,
        }
        .abi_encode()
        .into();
        if *inner != call {
            return Err(format!(
                "contract {index} calldata is not acceptManagerTransfer({contract})"
            ));
        }
        expected.push(call);
    }
    check_batch(data, expected)
}

pub(crate) fn removal_targets(assertions: &[Value]) -> Result<Vec<(Address, B256)>, String> {
    let mut targets = Vec::with_capacity(assertions.len());
    for (index, assertion) in assertions.iter().enumerate() {
        let Some(object) = assertion.as_object() else {
            return Err(format!("assertion {index} is not an object"));
        };
        let contract = object
            .get("contractAddress")
            .or_else(|| object.get("contract_address"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("assertion {index} is missing contractAddress"))?;
        let contract: Address = contract
            .parse()
            .map_err(|_| format!("assertion {index} has an invalid contract address"))?;
        let assertion_id = object
            .get("assertionId")
            .or_else(|| object.get("assertion_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("assertion {index} is missing assertionId"))?;
        let assertion_id: B256 = assertion_id
            .parse()
            .map_err(|_| format!("assertion {index} has an invalid assertionId"))?;
        targets.push((contract, assertion_id));
    }
    Ok(targets)
}

/// Every `(contract, assertionId)` in the release record's config snapshot:
/// the set the API's `buildRemoveCalldata` removes, read from `getRelease`.
pub(crate) fn release_targets(release: &Value) -> Result<Vec<(Address, B256)>, String> {
    let contracts = release
        .get("configSnapshot")
        .or_else(|| release.get("config_snapshot"))
        .and_then(|snapshot| snapshot.get("contracts"))
        .and_then(Value::as_object)
        .ok_or("the release record has no config snapshot contracts")?;
    let mut targets = Vec::new();
    for contract in contracts.values() {
        let address: Address = contract
            .get("address")
            .and_then(Value::as_str)
            .and_then(|address| address.parse().ok())
            .ok_or("a release record contract has no valid address")?;
        let assertions = contract
            .get("assertions")
            .and_then(Value::as_array)
            .ok_or("a release record contract has no assertions list")?;
        for assertion in assertions {
            let id: B256 = assertion
                .get("assertionId")
                .or_else(|| assertion.get("assertion_id"))
                .and_then(Value::as_str)
                .and_then(|id| id.parse().ok())
                .ok_or("a release record assertion has no valid assertionId")?;
            targets.push((address, id));
        }
    }
    Ok(targets)
}

/// Requires `data` to be exactly the canonical `StateOracle.batch` of
/// `expected`. Building the bytes pcl expects and comparing them rejects dirty
/// padding, trailing bytes and forged offsets or lengths without decoding (or
/// allocating for) anything the API sent.
fn check_batch(data: &Bytes, expected: Vec<Bytes>) -> Result<(), String> {
    if expected.is_empty() {
        return Err("the batch has no calls to sign".to_string());
    }
    let count = expected.len();
    if *data != crate::onchain::encode_batch(expected) {
        return Err(format!(
            "calldata is not the StateOracle.batch of the {count} expected call(s)"
        ));
    }
    Ok(())
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
    use std::time::{
        Duration,
        Instant,
    };

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
            &eth(),
            1,
            OracleEnvironment::Production,
            address!("2222222222222222222222222222222222222222"),
            &removal_data(),
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        let reason = err;
        assert!(
            reason.contains("8681e3fF0526C785FE82db9153E6E1F54551ebdF"),
            "{reason}"
        );
    }

    #[test]
    fn removal_accepts_a_batch_of_remove_assertion_calls() {
        validate_removal_batch(
            &eth(),
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &removal_data(),
            &assertions(),
            &[(ADOPTER, ASSERTION)],
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
            &eth(),
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &data,
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn removal_refuses_an_unknown_platform() {
        let err = validate_removal_batch(
            &"http://127.0.0.1".parse().unwrap(),
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &removal_data(),
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        let reason = err;
        assert!(reason.contains("127.0.0.1"), "{reason}");
    }

    #[test]
    fn accept_refuses_anything_other_than_accept_manager_transfer() {
        let linea: Url = LINEA_URL.parse().unwrap();
        let oracle = address!("984Ed2269Cdcc0e105675b4a16120Bc0531Bb218");
        let inner: Bytes = acceptManagerTransferCall {
            contractAddress: ADOPTER,
        }
        .abi_encode()
        .into();
        validate_accept_batch(
            &linea,
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
            &linea,
            59144,
            oracle,
            &encode_batch(vec![wrong.clone()]),
            &[(ADOPTER, wrong)],
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn retired_ethereum_oracle_is_not_active() {
        let retired = address!("B4951F9eA03C6bc7eE441E208C4AC996B2f6e369");
        let err = validate_removal_batch(
            &eth(),
            1,
            OracleEnvironment::Production,
            retired,
            &removal_data(),
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        assert!(!err.is_empty());
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
            &eth(),
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &naked,
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn removal_uses_the_staging_oracle_only_for_a_staging_release() {
        let staging = address!("B5E1b58638E05a69c853dEf16837D651B7EEeAA2");
        validate_removal_batch(
            &eth(),
            1,
            OracleEnvironment::Staging,
            staging,
            &removal_data(),
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap();

        let err = validate_removal_batch(
            &eth(),
            1,
            OracleEnvironment::Production,
            staging,
            &removal_data(),
            &assertions(),
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn removal_entries_must_match_the_release_snapshot() {
        let data = removal_data();
        let check = |release: &[(Address, B256)]| {
            validate_removal_batch(
                &eth(),
                1,
                OracleEnvironment::Production,
                ETH_PRODUCTION,
                &data,
                &assertions(),
                release,
            )
        };
        check(&[(ADOPTER, ASSERTION)]).unwrap();
        let other = B256::repeat_byte(0xbb);
        for release in [
            vec![],
            vec![(ADOPTER, other)],
            vec![(ADOPTER, ASSERTION), (ADOPTER, other)],
            vec![(ADOPTER, ASSERTION), (ADOPTER, ASSERTION)],
        ] {
            let reason = check(&release).unwrap_err();
            assert!(reason.contains("release record"), "{release:?}: {reason}");
        }
    }

    #[test]
    fn release_targets_read_every_snapshot_assertion() {
        let snapshot = |key: &str, id_key: &str| {
            json!({ key: {"contracts": {
                "b": {"address": "0x0202020202020202020202020202020202020202", "assertions": [{id_key: format!("{ASSERTION}")}]},
                "a": {"address": format!("{ADOPTER}").to_lowercase(), "assertions": [{id_key: format!("{ASSERTION}")}, {id_key: format!("{ASSERTION}")}]}
            }}})
        };
        for release in [
            snapshot("configSnapshot", "assertionId"),
            snapshot("config_snapshot", "assertion_id"),
        ] {
            let mut targets = release_targets(&release).unwrap();
            targets.sort_unstable();
            assert_eq!(
                targets,
                vec![
                    (ADOPTER, ASSERTION),
                    (ADOPTER, ASSERTION),
                    (
                        address!("0202020202020202020202020202020202020202"),
                        ASSERTION
                    ),
                ]
            );
        }
        for broken in [
            json!({}),
            json!({"configSnapshot": {"contracts": []}}),
            json!({"configSnapshot": {"contracts": {"a": {"address": "0x01", "assertions": []}}}}),
            json!({"configSnapshot": {"contracts": {"a": {"address": format!("{ADOPTER}")}}}}),
            json!({"configSnapshot": {"contracts": {"a": {"address": format!("{ADOPTER}"), "assertions": [{"assertionId": "1"}]}}}}),
        ] {
            assert!(release_targets(&broken).is_err(), "{broken}");
        }
    }

    #[test]
    fn release_environment_reads_the_release_record() {
        assert_eq!(
            release_environment(&json!({"environment": "staging"})).unwrap(),
            OracleEnvironment::Staging
        );
        assert!(release_environment(&json!({})).is_err());
    }

    const ETH_URL: &str = "https://ethereum.phylax.systems";
    const LINEA_URL: &str = "https://linea.phylax.systems";

    fn eth() -> Url {
        ETH_URL.parse().unwrap()
    }
    const LINEA_PRODUCTION: Address = address!("984Ed2269Cdcc0e105675b4a16120Bc0531Bb218");

    fn remove_entry() -> Vec<u8> {
        removeAssertionCall {
            contractAddress: ADOPTER,
            assertionId: ASSERTION,
        }
        .abi_encode()
    }

    fn accept_entry() -> Vec<u8> {
        acceptManagerTransferCall {
            contractAddress: ADOPTER,
        }
        .abi_encode()
    }

    fn removal(url: &str, to: Address, data: &[u8], assertions: &[Value]) -> Result<(), String> {
        validate_removal_batch(
            &url.parse().unwrap(),
            1,
            OracleEnvironment::Production,
            to,
            &Bytes::copy_from_slice(data),
            assertions,
            &removal_targets(assertions).unwrap_or_default(),
        )
    }

    fn accept(
        url: &str,
        to: Address,
        data: &[u8],
        contracts: &[(Address, Vec<u8>)],
    ) -> Result<(), String> {
        let contracts = contracts
            .iter()
            .map(|(address, inner)| (*address, Bytes::copy_from_slice(inner)))
            .collect::<Vec<_>>();
        validate_accept_batch(
            &url.parse().unwrap(),
            59_144,
            to,
            &Bytes::copy_from_slice(data),
            &contracts,
        )
    }

    fn dirty_address(mut entry: Vec<u8>) -> Vec<u8> {
        // First byte of the address word's zero padding.
        entry[4] = 0xff;
        entry
    }

    #[test]
    fn removal_refuses_dirty_address_padding() {
        let entry = dirty_address(remove_entry());
        let data = encode_batch(vec![entry.into()]);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &data, &assertions()).is_err());
    }

    #[test]
    fn accept_refuses_dirty_address_padding() {
        let entry = dirty_address(accept_entry());
        let data = encode_batch(vec![entry.clone().into()]);
        assert!(accept(LINEA_URL, LINEA_PRODUCTION, &data, &[(ADOPTER, entry)]).is_err());
    }

    #[test]
    fn removal_refuses_trailing_bytes() {
        let mut outer = encode_batch(vec![remove_entry().into()]).to_vec();
        outer.extend_from_slice(&[0u8; 32]);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &outer, &assertions()).is_err());
        let mut odd = encode_batch(vec![remove_entry().into()]).to_vec();
        odd.push(0);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &odd, &assertions()).is_err());

        let mut inner = remove_entry();
        inner.extend_from_slice(&[0u8; 32]);
        let data = encode_batch(vec![inner.into()]);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &data, &assertions()).is_err());
    }

    #[test]
    fn accept_refuses_trailing_bytes() {
        let mut inner = accept_entry();
        inner.extend_from_slice(&[0u8; 32]);
        let data = encode_batch(vec![inner.clone().into()]);
        assert!(accept(LINEA_URL, LINEA_PRODUCTION, &data, &[(ADOPTER, inner)]).is_err());
        let mut outer = encode_batch(vec![accept_entry().into()]).to_vec();
        outer.extend_from_slice(&[0u8; 32]);
        assert!(
            accept(
                LINEA_URL,
                LINEA_PRODUCTION,
                &outer,
                &[(ADOPTER, accept_entry())]
            )
            .is_err()
        );
    }

    #[test]
    fn empty_batches_are_refused() {
        let empty = encode_batch(Vec::new());
        assert!(removal(ETH_URL, ETH_PRODUCTION, &empty, &[]).is_err());
        assert!(accept(LINEA_URL, LINEA_PRODUCTION, &empty, &[]).is_err());
    }

    /// `selector | offset | count | count offsets that all point at one
    /// element | element length | element bytes`.
    fn overlapping_offsets(count: usize, element_len: usize) -> Vec<u8> {
        let mut data = crate::onchain::encode_batch(Vec::new()).to_vec();
        data.truncate(4);
        let word = |value: usize| {
            let mut word = [0u8; 32];
            word[24..].copy_from_slice(&(value as u64).to_be_bytes());
            word
        };
        data.extend_from_slice(&word(0x20));
        data.extend_from_slice(&word(count));
        for _ in 0..count {
            data.extend_from_slice(&word(count * 32));
        }
        data.extend_from_slice(&word(element_len));
        data.extend(std::iter::repeat_n(0u8, element_len));
        data
    }

    #[test]
    fn hostile_batch_layouts_are_refused_quickly() {
        let started = Instant::now();
        // 64,868 bytes whose 1000 offsets all point at one 32 KiB element.
        let overlapping = overlapping_offsets(1000, 32_768);
        assert_eq!(overlapping.len(), 64_868);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &overlapping, &assertions()).is_err());
        assert!(
            accept(
                LINEA_URL,
                LINEA_PRODUCTION,
                &overlapping,
                &[(ADOPTER, accept_entry())]
            )
            .is_err()
        );

        // A 64 KiB body that declares 2^38 entries.
        let mut forged = overlapping_offsets(0, 0);
        forged[4 + 32 + 24..4 + 64].copy_from_slice(&(1u64 << 38).to_be_bytes());
        forged.resize(65_536, 0);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &forged, &assertions()).is_err());

        // ~1 MiB that a decoder would expand to 8 GB of copied entries.
        let huge = overlapping_offsets(16_000, 512_000);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &huge, &assertions()).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn host_match_requires_the_canonical_origin() {
        let data = encode_batch(vec![remove_entry().into()]);
        for url in [
            "http://ethereum.phylax.systems",
            "https://ethereum.phylax.systems:8080",
            "https://ethereum.phylax.systems:8443",
            "https://u:p@ethereum.phylax.systems",
            "https://ETHEREUM.phylax.systems.",
            "https://ethereum.phylax.systems/?x=1",
            "https://ethereum.phylax.systems/api/v1",
            "https://ethereum.phylax.systems.evil.example",
        ] {
            let reason = removal(url, ETH_PRODUCTION, &data, &assertions()).unwrap_err();
            assert!(reason.contains(ETH_URL), "{url}: {reason}");
        }
        removal(ETH_URL, ETH_PRODUCTION, &data, &assertions()).unwrap();
        removal(
            "https://ETHEREUM.phylax.systems/",
            ETH_PRODUCTION,
            &data,
            &assertions(),
        )
        .unwrap();
    }

    #[test]
    fn right_function_with_wrong_arguments_is_refused() {
        let other = remove_entry_for(ADOPTER, B256::repeat_byte(0xbb));
        let data = encode_batch(vec![other.into()]);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &data, &assertions()).is_err());

        let elsewhere = address!("0202020202020202020202020202020202020202");
        let inner = acceptManagerTransferCall {
            contractAddress: elsewhere,
        }
        .abi_encode();
        let data = encode_batch(vec![inner.clone().into()]);
        assert!(accept(LINEA_URL, LINEA_PRODUCTION, &data, &[(ADOPTER, inner)]).is_err());
    }

    #[test]
    fn batch_and_list_lengths_must_match() {
        let data = encode_batch(vec![remove_entry().into(), remove_entry().into()]);
        assert!(removal(ETH_URL, ETH_PRODUCTION, &data, &assertions()).is_err());
        let data = encode_batch(vec![accept_entry().into()]);
        assert!(
            accept(
                LINEA_URL,
                LINEA_PRODUCTION,
                &data,
                &[(ADOPTER, accept_entry()), (ADOPTER, accept_entry())]
            )
            .is_err()
        );
    }

    fn remove_entry_for(contract: Address, assertion: B256) -> Vec<u8> {
        removeAssertionCall {
            contractAddress: contract,
            assertionId: assertion,
        }
        .abi_encode()
    }
}
