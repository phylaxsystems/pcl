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
    function transferManager(address contractAddress, address newManager);
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

/// Checks a manager batch: `transferManager(contract, new_manager)` per
/// contract when `new_manager` is set (initiation), otherwise
/// `acceptManagerTransfer(contract)` (acceptance).
pub(crate) fn validate_manager_batch(
    platform: &Url,
    chain_id: u64,
    to: Address,
    data: &Bytes,
    contracts: &[(Address, Bytes)],
    new_manager: Option<Address>,
) -> Result<(), String> {
    // Manager responses name no environment, and the dapp builds them for
    // whichever single environment holds the project's active contracts, so
    // either of the chain's active oracles may be the target.
    check_target(platform, chain_id, None, to)?;

    let mut expected = Vec::with_capacity(contracts.len());
    for (index, (contract, inner)) in contracts.iter().enumerate() {
        let (call, function): (Bytes, _) = match new_manager {
            Some(new_manager) => {
                (
                    transferManagerCall {
                        contractAddress: *contract,
                        newManager: new_manager,
                    }
                    .abi_encode()
                    .into(),
                    "transferManager",
                )
            }
            None => {
                (
                    acceptManagerTransferCall {
                        contractAddress: *contract,
                    }
                    .abi_encode()
                    .into(),
                    "acceptManagerTransfer",
                )
            }
        };
        if *inner != call {
            return Err(format!(
                "contract {index} calldata is not the expected {function} call for {contract}"
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
    use alloy_primitives::b256;
    use serde_json::json;

    sol! {
        function transfer(address to, uint256 amount);
    }

    const ETH_URL: &str = "https://ethereum.phylax.systems";
    const ETH_PRODUCTION: Address = address!("8681e3fF0526C785FE82db9153E6E1F54551ebdF");
    const ETH_STAGING: Address = address!("B5E1b58638E05a69c853dEf16837D651B7EEeAA2");
    const ETH_RETIRED: Address = address!("B4951F9eA03C6bc7eE441E208C4AC996B2f6e369");
    const LINEA_PRODUCTION: Address = address!("984Ed2269Cdcc0e105675b4a16120Bc0531Bb218");
    const ADOPTER: Address = address!("0101010101010101010101010101010101010101");
    const TOKEN: Address = address!("1111111111111111111111111111111111111111");
    const ATTACKER: Address = address!("2222222222222222222222222222222222222222");
    const OTHER_PROJECT: Address = address!("0404040404040404040404040404040404040404");
    const ASSERTION: B256 =
        b256!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    fn remove_entry(contract: Address) -> Bytes {
        removeAssertionCall {
            contractAddress: contract,
            assertionId: ASSERTION,
        }
        .abi_encode()
        .into()
    }

    fn accept_entry() -> Bytes {
        acceptManagerTransferCall {
            contractAddress: ADOPTER,
        }
        .abi_encode()
        .into()
    }

    fn listed(contract: Address) -> Vec<Value> {
        vec![json!({"contractAddress": contract.to_string(), "assertionId": ASSERTION.to_string()})]
    }

    fn token_transfer() -> Bytes {
        transferCall {
            to: ATTACKER,
            amount: alloy_primitives::U256::from(10).pow(alloy_primitives::U256::from(24)),
        }
        .abi_encode()
        .into()
    }

    fn remove_on(
        url: &str,
        environment: OracleEnvironment,
        to: Address,
        data: &Bytes,
        listed: &[Value],
    ) -> Result<(), String> {
        validate_removal_batch(
            &url.parse().unwrap(),
            1,
            environment,
            to,
            data,
            listed,
            &[(ADOPTER, ASSERTION)],
        )
    }

    /// PHYL1-208: a compromised API points release removal at another
    /// contract, another `StateOracle`, or another project's assertions.
    #[test]
    fn phyl1_208_compromised_removal_responses_are_refused() {
        let good = encode_batch(vec![remove_entry(ADOPTER)]);
        let other = encode_batch(vec![remove_entry(OTHER_PROJECT)]);
        let mut trailing = good.to_vec();
        trailing.push(0);
        let mut dirty = remove_entry(ADOPTER).to_vec();
        dirty[4] = 0xff; // the address word's zero padding
        let cases = [
            (
                TOKEN,
                token_transfer(),
                listed(ADOPTER),
                "is not the active StateOracle",
            ),
            (
                ETH_RETIRED,
                good.clone(),
                listed(ADOPTER),
                "is not the active StateOracle",
            ),
            (
                LINEA_PRODUCTION,
                good.clone(),
                listed(ADOPTER),
                "is not the active StateOracle",
            ),
            (
                ETH_STAGING,
                good.clone(),
                listed(ADOPTER),
                "is not the active StateOracle",
            ),
            (
                ETH_PRODUCTION,
                other.clone(),
                listed(OTHER_PROJECT),
                "release record",
            ),
            (
                ETH_PRODUCTION,
                other,
                listed(ADOPTER),
                "is not the StateOracle.batch",
            ),
            (
                ETH_PRODUCTION,
                trailing.into(),
                listed(ADOPTER),
                "is not the StateOracle.batch",
            ),
            (
                ETH_PRODUCTION,
                encode_batch(vec![dirty.into()]),
                listed(ADOPTER),
                "is not the StateOracle.batch",
            ),
        ];
        for (to, data, listed, reason) in cases {
            let err =
                remove_on(ETH_URL, OracleEnvironment::Production, to, &data, &listed).unwrap_err();
            assert!(err.contains(reason), "{to}: {err}");
        }
        remove_on(
            ETH_URL,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &good,
            &listed(ADOPTER),
        )
        .unwrap();
        remove_on(
            ETH_URL,
            OracleEnvironment::Staging,
            ETH_STAGING,
            &good,
            &listed(ADOPTER),
        )
        .unwrap();
    }

    /// PHYL1-96: a compromised API points manager acceptance at another
    /// contract, another chain's `StateOracle`, or a call other than
    /// `acceptManagerTransfer`. The project chain check is in `broadcast.rs`.
    #[test]
    fn phyl1_96_compromised_accept_responses_are_refused() {
        let accept = |chain_id: u64, to: Address, data: Bytes, entry: Bytes| {
            validate_manager_batch(
                &ETH_URL.parse().unwrap(),
                chain_id,
                to,
                &data,
                &[(ADOPTER, entry)],
                None,
            )
        };
        let batch = encode_batch(vec![accept_entry()]);
        let hijack: Bytes = transferManagerCall {
            contractAddress: ADOPTER,
            newManager: ATTACKER,
        }
        .abi_encode()
        .into();
        let cases = [
            (
                1,
                TOKEN,
                token_transfer(),
                accept_entry(),
                "is not the active StateOracle",
            ),
            (
                1,
                TOKEN,
                batch.clone(),
                accept_entry(),
                "is not the active StateOracle",
            ),
            (
                1,
                LINEA_PRODUCTION,
                batch.clone(),
                accept_entry(),
                "is not the active StateOracle",
            ),
            (
                59_144,
                LINEA_PRODUCTION,
                batch.clone(),
                accept_entry(),
                "no active StateOracle",
            ),
            (
                1,
                ETH_PRODUCTION,
                encode_batch(vec![hijack.clone()]),
                hijack,
                "expected acceptManagerTransfer",
            ),
            (
                1,
                ETH_PRODUCTION,
                accept_entry(),
                accept_entry(),
                "is not the StateOracle.batch",
            ),
            (
                1,
                ETH_PRODUCTION,
                encode_batch(Vec::new()),
                accept_entry(),
                "is not the StateOracle.batch",
            ),
        ];
        for (chain_id, to, data, entry, reason) in cases {
            let err = accept(chain_id, to, data, entry).unwrap_err();
            assert!(err.contains(reason), "{to}: {err}");
        }
        for oracle in [ETH_PRODUCTION, ETH_STAGING] {
            accept(1, oracle, batch.clone(), accept_entry()).unwrap();
        }
    }

    #[test]
    fn transfer_entries_must_name_the_requested_new_manager() {
        let requested = address!("0303030303030303030303030303030303030303");
        let entry = |new_manager| -> Bytes {
            transferManagerCall {
                contractAddress: ADOPTER,
                newManager: new_manager,
            }
            .abi_encode()
            .into()
        };
        let check = |inner: Bytes| {
            validate_manager_batch(
                &ETH_URL.parse().unwrap(),
                1,
                ETH_PRODUCTION,
                &encode_batch(vec![inner.clone()]),
                &[(ADOPTER, inner)],
                Some(requested),
            )
        };
        check(entry(requested)).unwrap();
        assert!(check(entry(ATTACKER)).is_err());
        assert!(check(accept_entry()).is_err());
    }

    #[test]
    fn only_the_canonical_platform_origin_is_trusted() {
        let data = encode_batch(vec![remove_entry(ADOPTER)]);
        for url in [
            "http://ethereum.phylax.systems",
            "https://ethereum.phylax.systems:8443",
            "https://u:p@ethereum.phylax.systems",
            "https://ETHEREUM.phylax.systems.",
            "https://ethereum.phylax.systems/api/v1",
            "https://ethereum.phylax.systems.evil.example",
        ] {
            let reason = remove_on(
                url,
                OracleEnvironment::Production,
                ETH_PRODUCTION,
                &data,
                &listed(ADOPTER),
            )
            .unwrap_err();
            assert!(reason.contains(ETH_URL), "{url}: {reason}");
        }
    }
}
