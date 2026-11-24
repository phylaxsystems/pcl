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
use std::collections::BTreeSet;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HandoverBinding {
    pub environment: OracleEnvironment,
    pub contracts: BTreeSet<Address>,
}

/// Active `StateOracle` addresses compiled from the ethereum and linea dapp chain
/// profiles. Only entries whose profile status is `active` are listed.
/// A platform that is not here has no oracle pcl is willing to sign against.
const ACTIVE_ORACLES: &[(Network, u64, OracleEnvironment, Address)] = &[
    (
        Network::EthereumMainnet,
        1,
        OracleEnvironment::Production,
        address!("14386aF70E91dFE12aC6243D4B5e087870d7f14D"),
    ),
    (
        Network::EthereumMainnet,
        1,
        OracleEnvironment::Staging,
        address!("DE55F19430b3Ec5Fa7e5978B49E4467e74C4175b"),
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

/// Requires `to` to be the active `StateOracle` for the platform, chain and
/// environment. The platform must be a network's canonical
/// origin (`Network::from_url`), so another scheme, port, userinfo or a
/// trailing-dot host is refused.
fn check_target(
    platform: &Url,
    chain_id: u64,
    environment: OracleEnvironment,
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
        "{} chain {chain_id} {}",
        network.host(),
        environment.as_str()
    );
    let active = ACTIVE_ORACLES
        .iter()
        .filter(|(oracle_network, oracle_chain, oracle_env, _)| {
            *oracle_network == network && *oracle_chain == chain_id && *oracle_env == environment
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

/// The detail pcl fetched must be the release the confirmation will name,
/// still active, and owned by the project the operator asked to remove.
pub(crate) fn require_requested_active_release(
    release: &Value,
    project_id: &str,
    release_id: &str,
) -> Result<(), String> {
    require_release_identity(release, project_id, release_id)?;
    let status = release_string(release, &["status"])?;
    if status != "active" {
        return Err(format!(
            "release record status is {status}; only an active release can be removed"
        ));
    }
    Ok(())
}

/// The detail must be the release that was requested and belong to the
/// project whose handover or removal is being signed.
fn require_release_identity(
    release: &Value,
    project_id: &str,
    release_id: &str,
) -> Result<(), String> {
    let id = release_string(release, &["id"])?;
    if !id.eq_ignore_ascii_case(release_id) {
        return Err(format!(
            "release record id {id} is not the requested release {release_id}"
        ));
    }
    let owner = release_string(release, &["projectId", "project_id"])?;
    if !owner.eq_ignore_ascii_case(project_id) {
        return Err(format!(
            "release record projectId {owner} is not the requested project {project_id}"
        ));
    }
    Ok(())
}

fn release_string(release: &Value, keys: &[&str]) -> Result<String, String> {
    keys.iter()
        .find_map(|key| release.get(*key).and_then(Value::as_str))
        .map(str::to_owned)
        .ok_or_else(|| format!("the release record is missing {}", keys[0]))
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
    // `buildRemoveCalldata` emits one call per distinct pair and skips a pair
    // the snapshot stored twice. The calldata must still contain that one call.
    if listed.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("the listed assertions repeat a (contract, assertionId) pair".to_string());
    }
    let mut held = release.to_vec();
    held.sort_unstable();
    held.dedup();
    if listed != held {
        return Err(
            "the listed assertions do not match the release record's config snapshot".to_string(),
        );
    }
    check_target(platform, chain_id, environment, to)?;
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

/// Page size for the active release list. The dapp keeps at most one active
/// release per environment, so a full page is unexpected and refused as
/// possibly truncated rather than trusted.
pub(crate) const ACTIVE_RELEASE_PAGE: usize = 100;

pub(crate) fn active_release_ids(body: &Value) -> Result<Vec<String>, String> {
    let items = body
        .as_array()
        .ok_or("active release list is not a JSON array")?;
    if items.len() >= ACTIVE_RELEASE_PAGE {
        return Err(
            "active release page is full, so the contract set may be truncated".to_string(),
        );
    }
    let mut ids = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        require_active(item, index)?;
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("release {index} has no id"))?;
        ids.push(id.to_string());
    }
    Ok(ids)
}

/// Environment and contract set of a fetched release detail. The detail must
/// be the listed release, owned by this project, and still report `active`.
/// Identity is checked before any address is read, so a detail for another
/// release or project cannot supply the handover set.
pub(crate) fn active_snapshot(
    id: &str,
    project_id: &str,
    release: &Value,
) -> Result<(OracleEnvironment, BTreeSet<Address>), String> {
    require_release_identity(release, project_id, id)?;
    require_active(release, id)?;
    Ok((snapshot_environment(release)?, snapshot_addresses(release)?))
}

fn require_active(release: &Value, label: impl std::fmt::Display) -> Result<(), String> {
    match release.get("status").and_then(Value::as_str) {
        Some("active") => Ok(()),
        Some(status) => Err(format!("release {label} has status {status}, not active")),
        None => Err(format!("release {label} has no status")),
    }
}

pub(crate) fn snapshot_environment(release: &Value) -> Result<OracleEnvironment, String> {
    match release.get("environment").and_then(Value::as_str) {
        Some("production") => Ok(OracleEnvironment::Production),
        Some("staging") => Ok(OracleEnvironment::Staging),
        Some(other) => Err(format!("unknown release environment {other:?}")),
        None => Err("missing environment".to_string()),
    }
}

fn snapshot_addresses(release: &Value) -> Result<BTreeSet<Address>, String> {
    snapshot_contracts(release)?
        .map(|contract| contract.map(|(address, _)| address))
        .collect()
}

/// Each `configSnapshot.contracts` entry of a release record with its parsed
/// address, failing on the first entry without a valid one.
fn snapshot_contracts(
    release: &Value,
) -> Result<impl Iterator<Item = Result<(Address, &Value), String>>, String> {
    let contracts = release
        .get("configSnapshot")
        .or_else(|| release.get("config_snapshot"))
        .and_then(|snapshot| snapshot.get("contracts"))
        .and_then(Value::as_object)
        .ok_or("the release record has no config snapshot contracts")?;
    Ok(contracts.values().map(|contract| {
        let address = contract
            .get("address")
            .and_then(Value::as_str)
            .and_then(|address| address.parse().ok())
            .ok_or("a release record contract has no valid address")?;
        Ok((address, contract))
    }))
}

pub(crate) fn bind_handover(
    releases: &[(OracleEnvironment, BTreeSet<Address>)],
) -> Result<HandoverBinding, String> {
    // Like the dapp, a release with no contracts holds nothing to hand over
    // and does not count toward the single environment.
    let held = releases
        .iter()
        .filter(|(_, contracts)| !contracts.is_empty());
    let Some((environment, _)) = held.clone().next() else {
        return Err(
            "the project has no active release contracts to bind the manager batch to".to_string(),
        );
    };
    // There are two environments, so a second one present means both are.
    if held.clone().any(|(other, _)| other != environment) {
        return Err(
            "active releases span production and staging; pcl signs a manager handover for one environment's StateOracle"
                .to_string(),
        );
    }
    Ok(HandoverBinding {
        environment: *environment,
        contracts: held
            .flat_map(|(_, contracts)| contracts.iter().copied())
            .collect(),
    })
}

fn require_exact_contracts(
    held: &BTreeSet<Address>,
    listed: &[(Address, Bytes)],
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    for (address, _) in listed {
        if !seen.insert(*address) {
            repeated.insert(*address);
        }
    }
    if repeated.is_empty() && &seen == held {
        return Ok(());
    }
    let missing = held
        .difference(&seen)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut extra: Vec<String> = seen.difference(held).map(ToString::to_string).collect();
    extra.extend(repeated.iter().map(ToString::to_string));
    extra.sort();
    extra.dedup();
    Err(format!(
        "manager batch contracts are not the active release set (missing: {missing}; extra: {})",
        extra.join(", ")
    ))
}

pub(crate) fn validate_manager_batch(
    platform: &Url,
    chain_id: u64,
    to: Address,
    data: &Bytes,
    contracts: &[(Address, Bytes)],
    new_manager: Option<Address>,
    handover: &HandoverBinding,
) -> Result<(), String> {
    require_exact_contracts(&handover.contracts, contracts)?;
    check_target(platform, chain_id, handover.environment, to)?;

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
    let mut targets = Vec::new();
    for contract in snapshot_contracts(release)? {
        let (address, contract) = contract?;
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
    const ETH_PRODUCTION: Address = address!("14386aF70E91dFE12aC6243D4B5e087870d7f14D");
    const ETH_STAGING: Address = address!("DE55F19430b3Ec5Fa7e5978B49E4467e74C4175b");
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

    fn handover(
        environment: OracleEnvironment,
        contracts: impl IntoIterator<Item = Address>,
    ) -> HandoverBinding {
        HandoverBinding {
            environment,
            contracts: contracts.into_iter().collect(),
        }
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
                &handover(OracleEnvironment::Production, [ADOPTER]),
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
        accept(1, ETH_PRODUCTION, batch.clone(), accept_entry()).unwrap();
        validate_manager_batch(
            &ETH_URL.parse().unwrap(),
            1,
            ETH_STAGING,
            &batch,
            &[(ADOPTER, accept_entry())],
            None,
            &handover(OracleEnvironment::Staging, [ADOPTER]),
        )
        .unwrap();
    }

    #[test]
    fn manager_batch_refuses_the_other_environments_oracle() {
        let batch = encode_batch(vec![accept_entry()]);
        let err = validate_manager_batch(
            &ETH_URL.parse().unwrap(),
            1,
            ETH_STAGING,
            &batch,
            &[(ADOPTER, accept_entry())],
            None,
            &handover(OracleEnvironment::Production, [ADOPTER]),
        )
        .unwrap_err();
        assert!(err.contains(&ETH_STAGING.to_string()), "{err}");
        assert!(err.contains("production"), "{err}");
    }

    #[test]
    fn handover_with_staging_contracts_only_targets_the_staging_oracle() {
        let handover = bind_handover(&[
            (OracleEnvironment::Production, BTreeSet::new()),
            (OracleEnvironment::Staging, BTreeSet::from([ADOPTER])),
        ])
        .unwrap();
        let batch = encode_batch(vec![accept_entry()]);
        let validate = |to| {
            validate_manager_batch(
                &ETH_URL.parse().unwrap(),
                1,
                to,
                &batch,
                &[(ADOPTER, accept_entry())],
                None,
                &handover,
            )
        };
        validate(ETH_STAGING).unwrap();
        let err = validate(ETH_PRODUCTION).unwrap_err();
        assert!(err.contains("staging"), "{err}");
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
                &handover(OracleEnvironment::Production, [ADOPTER]),
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

    #[test]
    fn a_repeated_snapshot_pair_still_matches_one_removal() {
        let data = encode_batch(vec![remove_entry(ADOPTER)]);
        validate_removal_batch(
            &ETH_URL.parse().unwrap(),
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &data,
            &listed(ADOPTER),
            &[(ADOPTER, ASSERTION), (ADOPTER, ASSERTION)],
        )
        .unwrap();
    }

    #[test]
    fn a_repeated_listed_pair_is_refused() {
        let mut assertions = listed(ADOPTER);
        assertions.extend(listed(ADOPTER));
        let err = validate_removal_batch(
            &ETH_URL.parse().unwrap(),
            1,
            OracleEnvironment::Production,
            ETH_PRODUCTION,
            &encode_batch(vec![remove_entry(ADOPTER), remove_entry(ADOPTER)]),
            &assertions,
            &[(ADOPTER, ASSERTION)],
        )
        .unwrap_err();
        assert!(err.contains("repeat"), "{err}");
    }

    #[test]
    fn handover_snapshot_rejects_a_different_release_or_project_before_its_contracts() {
        let project = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let body = json!({
            "id": id.to_ascii_uppercase(),
            "project_id": project.to_ascii_uppercase(),
            "status": "active",
            "environment": "staging",
            "configSnapshot": {"contracts": {"a": {"address": ADOPTER.to_string()}}},
        });
        let (environment, contracts) = active_snapshot(id, project, &body).unwrap();
        assert_eq!(environment, OracleEnvironment::Staging);
        assert_eq!(contracts, BTreeSet::from([ADOPTER]));

        let other_release = json!({
            "id": "cccccccc-cccc-cccc-cccc-cccccccccccc",
            "projectId": project,
            "status": "active",
            "configSnapshot": {"contracts": {"a": {"address": ADOPTER.to_string()}}},
        });
        let err = active_snapshot(id, project, &other_release).unwrap_err();
        assert!(err.contains("is not the requested release"), "{err}");
        assert!(!err.contains("address"), "{err}");

        let other_project = json!({
            "id": id,
            "projectId": "dddddddd-dddd-dddd-dddd-dddddddddddd",
            "status": "active",
            "configSnapshot": {"contracts": {"a": {"address": "not-read"}}},
        });
        let err = active_snapshot(id, project, &other_project).unwrap_err();
        assert!(err.contains("is not the requested project"), "{err}");
        assert!(!err.contains("not-read"), "{err}");

        let missing_project = json!({
            "id": id,
            "status": "active",
            "environment": "production",
        });
        let err = active_snapshot(id, project, &missing_project).unwrap_err();
        assert!(err.contains("missing projectId"), "{err}");
    }

    #[test]
    fn removal_uses_only_the_requested_active_release() {
        let body = json!({
            "id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            "projectId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "status": "active",
        });
        require_requested_active_release(
            &body,
            "AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA",
            "BBBBBBBB-BBBB-BBBB-BBBB-BBBBBBBBBBBB",
        )
        .unwrap();

        let wrong_id = json!({
            "id": "cccccccc-cccc-cccc-cccc-cccccccccccc",
            "projectId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "status": "active",
        });
        let err = require_requested_active_release(
            &wrong_id,
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        )
        .unwrap_err();
        assert!(err.contains("is not the requested release"), "{err}");

        let wrong_project = json!({
            "id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            "project_id": "dddddddd-dddd-dddd-dddd-dddddddddddd",
            "status": "active",
        });
        let err = require_requested_active_release(
            &wrong_project,
            "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        )
        .unwrap_err();
        assert!(err.contains("is not the requested project"), "{err}");

        for status in ["inactive", "removed"] {
            let stale = json!({
                "id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                "projectId": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "status": status,
            });
            let err = require_requested_active_release(
                &stale,
                "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            )
            .unwrap_err();
            assert!(err.contains(status), "{err}");
        }
    }

    #[test]
    fn ethereum_oracles_from_before_the_cutover_are_not_active() {
        let data = encode_batch(vec![remove_entry(ADOPTER)]);
        let previous = [
            address!("8681e3fF0526C785FE82db9153E6E1F54551ebdF"),
            address!("B5E1b58638E05a69c853dEf16837D651B7EEeAA2"),
        ];
        for retired in previous {
            for environment in [OracleEnvironment::Production, OracleEnvironment::Staging] {
                let err =
                    remove_on(ETH_URL, environment, retired, &data, &listed(ADOPTER)).unwrap_err();
                assert!(err.contains("is not the active StateOracle"), "{err}");
            }
        }
    }
}
