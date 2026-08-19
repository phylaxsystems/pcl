//! Which assertion spec a target platform runs, and whether a project's
//! assertions are written against the V2 spec.
//!
//! The Linea platform (`linea.phylax.systems`) runs the V1 assertion spec;
//! Ethereum (`ethereum.phylax.systems`) runs V2. Assertions written against the
//! V2 triggers and precompiles
//! (`registerTxEndTrigger`, `ph.staticcallAt`, the fork-id reads, the
//! protection-suite precompiles, the cumulative-flow circuit breakers) do not
//! run there, so `pcl deploy` and `pcl auth login` warn instead of letting the
//! mismatch surface as a failed release or an assertion that never triggers.
//!
//! Detection is a source-level heuristic over the assertion files listed in
//! `credible.toml`: V2-only identifiers are matched in the project's own
//! sources with comments stripped. Vendored `credible-std` code is never
//! scanned, because a V2-capable library declares those identifiers whether or
//! not an assertion uses them. The result only ever produces a warning, never
//! a hard failure, so a miss in either direction is recoverable.

use crate::credible_config::CredibleToml;
use serde_json::{
    Value,
    json,
};
use std::{
    collections::HashSet,
    fmt::Write as _,
    path::Path,
};
use url::Url;

/// Linea Mainnet.
pub const LINEA_MAINNET_CHAIN_ID: u64 = 59144;
/// Linea Sepolia.
pub const LINEA_SEPOLIA_CHAIN_ID: u64 = 59141;

/// Chains served by a platform that runs the V1 assertion spec.
const V1_ONLY_CHAIN_IDS: [u64; 2] = [LINEA_MAINNET_CHAIN_ID, LINEA_SEPOLIA_CHAIN_ID];

/// Platform hosts that run the V1 assertion spec.
///
/// `linea.phylax.systems` serves the Linea platform, which runs V1;
/// `ethereum.phylax.systems` runs V2 and is deliberately absent.
/// `app.phylax.systems` is kept because it is now only a router page and no
/// longer serves the API — an explicit `-u https://app.phylax.systems` from an
/// older habit or script should still get the notice rather than silently
/// nothing.
///
/// This host list is a proxy, and a poor one: the spec a deploy actually needs is
/// a property of the executor behind a platform and the chain it serves, not of a
/// hostname, and it has to be maintained by hand every time a platform moves.
/// `V1_ONLY_CHAIN_IDS` is the more reliable signal and the only one available
/// once the chain is known — but `pcl auth login` has no project or chain in
/// scope, so the host is all it has. Replacing both with a capability the
/// platform reports is tracked as follow-up work; until then a deploy to a
/// network that cannot run the assertions is warned about, not blocked.
const V1_ONLY_PLATFORM_HOSTS: [&str; 2] = ["linea.phylax.systems", "app.phylax.systems"];

/// Warning code used in machine output.
pub const V2_SPEC_UNSUPPORTED_CODE: &str = "assertion_spec.v2_unsupported";

/// V2-only identifiers called as functions: triggers, precompiles, and the
/// fork helpers on `Assertion`. This is the whole surface `credible-std`
/// declares under its `V2` section headers, plus the experimental flow-rate
/// reads that only exist alongside it; `tests::v2_surface_is_covered` fails
/// when the two drift apart.
///
/// A needle containing `.` (`ph.context`) is matched with the prefix, because
/// the bare identifier is too common to be evidence of anything. `ph.load` is
/// deliberately absent: the V1 `load(address, bytes32)` and the V2
/// `load(bytes32)` share a name, and telling them apart needs argument
/// parsing this heuristic does not do.
const V2_CALL_MARKERS: [&str; 47] = [
    // Triggers
    "registerTxEndTrigger",
    "registerFnCallTrigger",
    "registerErc20ChangeTrigger",
    "watchCumulativeInflow",
    "watchCumulativeOutflow",
    "watchAnomaly",
    // Fork helpers
    "_preTx",
    "_postTx",
    "_preCall",
    "_postCall",
    // Fork-aware reads and call inspection
    "staticcallAt",
    "loadStateAt",
    "callinputAt",
    "callOutputAt",
    "_matchingCalls",
    "matchingCalls",
    "_successOnlyFilter",
    "getLogsForCall",
    "getLogsQuery",
    "getErc20TransfersForTokens",
    "getErc20Transfers",
    "changedErc20BalanceDeltas",
    "reduceErc20BalanceDeltas",
    "getTxObject",
    "forbidChangeForSlot",
    "forbidChangeForSlots",
    // Trigger context
    "ph.context",
    "inflowContext",
    "outflowContext",
    "inflowRate",
    "outflowRate",
    // Experimental proof verification
    "verifyGnarkPlonkProof",
    // Persistent assertion storage
    "ph.store",
    "ph.exists",
    "ph.values_left",
    // Mapping tracing
    "changedMappingKeys",
    "mappingValueDiff",
    // Anomaly detection
    "anomalyContext",
    // Protection-suite precompiles
    "assetsMatchSharePriceAt",
    "assetsMatchSharePrice",
    "conserveBalance",
    "oracleSanityAt",
    "oracleSanity",
    "normalizeDecimals",
    "ratioGe",
    // Math precompiles
    "ph.mulDivDown",
    "ph.mulDivUp",
];

/// V2-only types, used in declarations rather than calls.
const V2_TYPE_MARKERS: [&str; 14] = [
    // Explicit non-Legacy spec registration
    "AssertionSpec.Reshiram",
    "AssertionSpec.Experimental",
    "PhEvm.ForkId",
    "PhEvm.TriggerContext",
    "PhEvm.TriggerCall",
    "PhEvm.CallFilter",
    "PhEvm.AnomalyContext",
    "PhEvm.InflowContext",
    "PhEvm.OutflowContext",
    "PhEvm.FlowRateContext",
    "PhEvm.Erc20TransferData",
    "PhEvm.StaticCallResult",
    "PhEvm.LogQuery",
    "PhEvm.TxObject",
];

/// One assertion source that uses the V2 spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2SpecFinding {
    /// Path as written in `credible.toml`, relative to the project root.
    pub file: String,
    /// V2-only identifiers found in that file, in canonical order.
    pub markers: Vec<String>,
}

/// Whether `platform_url` runs a platform that supports the V2 spec.
pub fn platform_supports_v2(platform_url: &Url) -> bool {
    platform_url.host_str().is_none_or(|host| {
        let host = host.trim_start_matches("www.").to_ascii_lowercase();
        !V1_ONLY_PLATFORM_HOSTS.contains(&host.as_str())
    })
}

/// Whether `chain_id` is served by a platform that supports the V2 spec.
pub fn chain_supports_v2(chain_id: u64) -> bool {
    !V1_ONLY_CHAIN_IDS.contains(&chain_id)
}

/// Human name for the chains that only run V1, for warning text.
fn chain_label(chain_id: u64) -> String {
    match chain_id {
        LINEA_MAINNET_CHAIN_ID => format!("Linea Mainnet, chain {chain_id}"),
        LINEA_SEPOLIA_CHAIN_ID => format!("Linea Sepolia, chain {chain_id}"),
        other => format!("chain {other}"),
    }
}

/// V2-only identifiers used by `source`, in the order they are declared in the
/// marker lists.
pub fn detect_v2_markers(source: &str) -> Vec<String> {
    let code = strip_comments_and_literals(source);
    let mut found: Vec<String> = Vec::new();

    for marker in V2_CALL_MARKERS {
        if contains_call(&code, marker) {
            found.push(marker.to_string());
        }
    }
    for marker in V2_TYPE_MARKERS {
        if contains_identifier(&code, marker) {
            found.push(marker.to_string());
        }
    }

    found
}

/// Scans the assertion sources declared in `credible.toml` for V2 spec usage.
/// Unreadable files are skipped: a warning check must never be the reason a
/// deploy fails, and the build step reports missing sources with a real error.
pub fn scan_assertion_sources(root: &Path, credible: &CredibleToml) -> Vec<V2SpecFinding> {
    let mut findings: Vec<V2SpecFinding> = Vec::new();

    for relative in unique_assertion_paths(credible) {
        let Ok(source) = std::fs::read_to_string(root.join(relative)) else {
            continue;
        };
        let markers = detect_v2_markers(&source);
        if !markers.is_empty() {
            findings.push(V2SpecFinding {
                file: relative.to_string(),
                markers,
            });
        }
    }

    findings
}

/// Distinct assertion source paths declared in `credible.toml`, in declaration
/// order. A `file` entry may carry a `path:Contract` suffix, and one source is
/// commonly referenced by several contracts; deduplicating before reading keeps
/// deploy startup scaling with unique files rather than with references.
fn unique_assertion_paths(credible: &CredibleToml) -> Vec<&str> {
    let mut seen: HashSet<&str> = HashSet::new();
    credible
        .contracts
        .values()
        .flat_map(|contract| &contract.assertions)
        .map(|assertion| {
            assertion
                .file
                .split_once(':')
                .map_or(assertion.file.as_str(), |(path, _)| path)
        })
        .filter(|relative| seen.insert(relative))
        .collect()
}

/// The V2-unsupported warning for a deploy, or `None` when the target platform
/// runs V2 or the project's assertions do not use it.
///
/// `chain_id` is `None` when the target chain is not known yet (a `--dry-run`
/// without `--chain-id`); the platform check alone still decides.
///
/// `platform_url` is `None` on a local `--dry-run` that never chose a platform.
/// The chain check still applies — `--chain-id 59144` is enough to know the
/// assertions will not run — so a plan does not have to pick a network to be
/// warned.
pub fn deploy_warning(
    platform_url: Option<&Url>,
    chain_id: Option<u64>,
    findings: &[V2SpecFinding],
) -> Option<String> {
    if findings.is_empty() {
        return None;
    }
    let platform_only_v1 = platform_url.is_some_and(|url| !platform_supports_v2(url));
    let chain_only_v1 = chain_id.is_some_and(|chain_id| !chain_supports_v2(chain_id));
    if !platform_only_v1 && !chain_only_v1 {
        return None;
    }

    let platform = platform_url.map(crate::platform::redact_platform_url);
    let target = match (
        platform,
        chain_id.filter(|chain_id| !chain_supports_v2(*chain_id)),
    ) {
        (Some(platform), Some(chain_id)) => format!("{platform} ({})", chain_label(chain_id)),
        (Some(platform), None) => platform,
        (None, Some(chain_id)) => format!("The target chain ({})", chain_label(chain_id)),
        // Unreachable: one of the two checks above was true to get here.
        (None, None) => return None,
    };

    let mut message =
        format!("{target} runs the V1 assertion spec, but these assertions use V2:\n");
    for finding in findings {
        let _ = writeln!(
            message,
            "  {} — {}",
            finding.file,
            finding.markers.join(", ")
        );
    }
    message.push_str(
        "V2 triggers and precompiles do not run there: the release can be rejected, or the assertion can deploy and never trigger. Rewrite these assertions against the V1 spec, or deploy to a platform that runs V2.",
    );
    Some(message)
}

/// Machine-output form of a warning message.
///
/// `platform_url` is `None` on a local plan that chose no platform, and is
/// reported as `null` rather than invented.
pub fn warning_json(
    message: &str,
    platform_url: Option<&Url>,
    chain_id: Option<u64>,
    findings: &[V2SpecFinding],
) -> Value {
    json!({
        "code": V2_SPEC_UNSUPPORTED_CODE,
        "message": message,
        "platform_url": platform_url.map(crate::platform::redact_platform_url),
        "chain_id": chain_id,
        "assertion_spec": "v1",
        "files": findings
            .iter()
            .map(|finding| json!({ "file": finding.file, "markers": finding.markers }))
            .collect::<Vec<_>>(),
    })
}

/// The notice shown after logging in to a platform that only runs V1, or
/// `None` for a platform that runs V2. No project is in scope at login time,
/// so this is advice rather than a finding.
pub fn login_notice(platform_url: &Url) -> Option<String> {
    if platform_supports_v2(platform_url) {
        return None;
    }
    Some(format!(
        "{} runs the V1 assertion spec. Do not write assertions against the V2 spec: V2 triggers and precompiles (registerTxEndTrigger, registerFnCallTrigger, ph.staticcallAt, the cumulative-flow circuit breakers) are not supported on this platform.",
        crate::platform::redact_platform_url(platform_url),
    ))
}

/// Machine-output form of [`login_notice`].
pub fn login_warning_json(message: &str, platform_url: &Url) -> Value {
    json!({
        "code": V2_SPEC_UNSUPPORTED_CODE,
        "message": message,
        "platform_url": crate::platform::redact_platform_url(platform_url),
        "assertion_spec": "v1",
    })
}

/// Replaces Solidity comments and string-literal contents with spaces, so a
/// commented-out V2 call is not read as usage and neither is one spelled out
/// inside a string. Byte positions are preserved, but only presence matters.
///
/// String state matters in both directions: a literal holding `/*` would
/// otherwise blank the rest of the file and hide a real V2 call, and a literal
/// holding `registerTxEndTrigger(` would otherwise be reported as usage.
fn strip_comments_and_literals(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' || bytes[index] == b'\'' {
            let quote = bytes[index];
            out.push(char::from(quote));
            index += 1;
            // Blank the contents, keeping the quotes so the surrounding code
            // stays separated. An escape consumes the next byte, so `\"` does
            // not end the literal; an unterminated one runs to end of file,
            // matching what solc would reject anyway.
            while index < bytes.len() && bytes[index] != quote {
                if bytes[index] == b'\\' && index + 1 < bytes.len() {
                    out.push(' ');
                    index += 1;
                }
                let char_len = source[index..].chars().next().map_or(1, char::len_utf8);
                for _ in 0..char_len {
                    out.push(if bytes[index] == b'\n' { '\n' } else { ' ' });
                }
                index += char_len;
            }
            if index < bytes.len() {
                out.push(char::from(quote));
                index += 1;
            }
        } else if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                out.push(' ');
                index += 1;
            }
        } else if bytes[index..].starts_with(b"/*") {
            while index < bytes.len() && !bytes[index..].starts_with(b"*/") {
                out.push(if bytes[index] == b'\n' { '\n' } else { ' ' });
                index += 1;
            }
            // Consume the closing `*/` when present; an unterminated block
            // comment runs to end of file.
            for _ in 0..2.min(bytes.len().saturating_sub(index)) {
                out.push(' ');
                index += 1;
            }
        } else {
            // Multi-byte characters only appear inside comments and string
            // literals in practice; copy them whole to keep the output valid
            // UTF-8.
            let char_len = source[index..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&source[index..index + char_len]);
            index += char_len;
        }
    }
    out
}

/// Whether `code` calls `marker`: the name appears with an identifier boundary
/// in front of it and an opening parenthesis after it.
fn contains_call(code: &str, marker: &str) -> bool {
    matches_with_boundary(code, marker, |rest| rest.trim_start().starts_with('('))
}

/// Whether `code` mentions `marker` as an identifier (a type, not a call).
fn contains_identifier(code: &str, marker: &str) -> bool {
    matches_with_boundary(code, marker, |rest| !rest.starts_with(is_identifier_char))
}

fn matches_with_boundary(code: &str, marker: &str, accept_rest: impl Fn(&str) -> bool) -> bool {
    let mut offset = 0;
    while let Some(position) = code[offset..].find(marker) {
        let start = offset + position;
        let end = start + marker.len();
        let preceded_by_identifier = code[..start]
            .chars()
            .next_back()
            .is_some_and(is_identifier_char);
        if !preceded_by_identifier && accept_rest(&code[end..]) {
            return true;
        }
        offset = end;
    }
    false
}

fn is_identifier_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '$'
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn url(value: &str) -> Url {
        value.parse().expect("valid url")
    }

    #[test]
    fn linea_is_v1_only_and_ethereum_runs_v2() {
        // Both networks the selector offers, so neither choice can silently
        // lose its compatibility notice when a platform is renamed again.
        assert!(!platform_supports_v2(&url("https://linea.phylax.systems")));
        assert!(!platform_supports_v2(&url(
            "https://Linea.Phylax.Systems/dashboard"
        )));
        assert!(platform_supports_v2(&url(
            "https://ethereum.phylax.systems"
        )));

        // Kept for anyone still passing the old host explicitly.
        assert!(!platform_supports_v2(&url("https://app.phylax.systems")));
        assert!(!platform_supports_v2(&url(
            "https://APP.Phylax.Systems/dashboard"
        )));

        assert!(!chain_supports_v2(LINEA_MAINNET_CHAIN_ID));
        assert!(!chain_supports_v2(LINEA_SEPOLIA_CHAIN_ID));

        assert!(platform_supports_v2(&url("https://dev.phylax.systems")));
        assert!(platform_supports_v2(&url("http://localhost:3000")));
        assert!(chain_supports_v2(84532));
    }

    /// Every network in the selector has a decided spec capability. A network
    /// added to the picker without deciding this would silently inherit "runs
    /// V2", which is the wrong default: it means no warning at all.
    #[test]
    fn every_selectable_network_has_a_decided_spec_capability() {
        for network in crate::platform::SELECTABLE_NETWORKS {
            let expected_v2 = match network {
                crate::platform::Network::EthereumMainnet => true,
                crate::platform::Network::LineaMainnet => false,
            };
            assert_eq!(
                platform_supports_v2(&network.url()),
                expected_v2,
                "{network} has the wrong assertion-spec capability"
            );
        }
    }

    #[test]
    fn detects_v2_triggers_precompiles_and_types() {
        let source = r"
            contract VaultAssertion is Assertion {
                function triggers() external view override {
                    registerTxEndTrigger(this.assertSolvency.selector);
                    watchCumulativeOutflow(token, 500, 1 hours, this.assertFlow.selector);
                }

                function assertSolvency() external {
                    PhEvm.ForkId memory pre = _preTx();
                    bytes memory data = ph.staticcallAt(vault, payload, 100000, pre);
                    require(ph.assetsMatchSharePriceAt(vault, 10, pre, _postTx()));
                }
            }
        ";

        assert_eq!(
            detect_v2_markers(source),
            vec![
                "registerTxEndTrigger",
                "watchCumulativeOutflow",
                "_preTx",
                "_postTx",
                "staticcallAt",
                "assetsMatchSharePriceAt",
                "PhEvm.ForkId",
            ]
        );
    }

    #[test]
    fn v1_assertions_report_no_markers() {
        let source = r"
            contract OwnableAssertion is Assertion {
                function triggers() external view override {
                    registerCallTrigger(this.assertionOwnershipChange.selector);
                }

                function assertionOwnershipChange() external {
                    ph.forkPreState();
                    address pre = ownable.owner();
                    ph.forkPostState();
                    require(pre == ownable.owner(), 'owner changed');
                }
            }
        ";

        assert!(detect_v2_markers(source).is_empty());
    }

    #[test]
    fn ignores_commented_out_and_look_alike_identifiers() {
        let source = r"
            contract Legacy is Assertion {
                // registerTxEndTrigger(this.check.selector);
                /* ph.staticcallAt(target, data, 100, pre); */
                function triggers() external view override {
                    myRegisterTxEndTrigger(this.check.selector);
                    address context = msg.sender;
                    uint256 conserveBalanceLimit = 1;
                }
            }
        ";

        assert!(
            detect_v2_markers(source).is_empty(),
            "{:?}",
            detect_v2_markers(source)
        );
    }

    #[test]
    fn unterminated_block_comment_is_stripped_to_end_of_file() {
        let source = "contract A { /* registerTxEndTrigger(x);";
        assert!(detect_v2_markers(source).is_empty());
    }

    #[test]
    fn non_ascii_source_does_not_panic_and_still_matches() {
        let source = "contract A { string s = \"↯ vault\"; function t() external { registerTxEndTrigger(this.c.selector); } }";
        assert_eq!(detect_v2_markers(source), vec!["registerTxEndTrigger"]);
    }

    #[test]
    fn a_v2_call_inside_a_string_literal_is_not_usage() {
        let source = r#"contract A { string s = "registerTxEndTrigger(x)"; }"#;
        assert!(
            detect_v2_markers(source).is_empty(),
            "{:?}",
            detect_v2_markers(source)
        );
        let escaped = r#"contract A { string s = "say \" registerTxEndTrigger(x)"; }"#;
        assert!(detect_v2_markers(escaped).is_empty());
        let single = "contract A { string s = 'registerTxEndTrigger(x)'; }";
        assert!(detect_v2_markers(single).is_empty());
    }

    #[test]
    fn a_comment_opener_inside_a_string_literal_does_not_blank_the_file() {
        let source = r#"contract A { string s = "/*"; function t() external { registerTxEndTrigger(this.c.selector); } }"#;
        assert_eq!(detect_v2_markers(source), vec!["registerTxEndTrigger"]);
        let line_comment = r#"contract A { string s = "//"; function t() external { registerTxEndTrigger(this.c.selector); } }"#;
        assert_eq!(
            detect_v2_markers(line_comment),
            vec!["registerTxEndTrigger"]
        );
    }

    #[test]
    fn a_quote_inside_a_comment_does_not_open_a_literal() {
        let source = "contract A { // it's fine\n function t() external { registerTxEndTrigger(this.c.selector); } }";
        assert_eq!(detect_v2_markers(source), vec!["registerTxEndTrigger"]);
    }

    fn credible_with(files: &[&str]) -> CredibleToml {
        let assertions = files.iter().fold(String::new(), |mut acc, file| {
            let _ = writeln!(acc, "[[contracts.mock.assertions]]\nfile = \"{file}\"");
            acc
        });
        toml::from_str(&format!(
            "environment = \"production\"\n\
             [contracts.mock]\n\
             address = \"0x0101010101010101010101010101010101010101\"\n\
             name = \"Mock\"\n\
             {assertions}"
        ))
        .expect("valid credible.toml")
    }

    fn project_with(files: &[(&str, &str)]) -> TempDir {
        let temp = TempDir::new().expect("temp dir");
        for (path, source) in files {
            let full = temp.path().join(path);
            fs::create_dir_all(full.parent().expect("parent")).expect("create dirs");
            fs::write(full, source).expect("write source");
        }
        temp
    }

    /// Every V2 identifier `credible-std` declares, with a representative use.
    /// This is the regression net for the marker lists: adding a V2 API to
    /// `credible-std` means adding it here and to `V2_CALL_MARKERS` /
    /// `V2_TYPE_MARKERS`, and `v2_surface_is_covered` fails when only one of
    /// the two happens.
    const V2_SURFACE: [(&str, &str); 61] = [
        (
            "AssertionSpec.Reshiram",
            "registerAssertionSpec(AssertionSpec.Reshiram);",
        ),
        (
            "AssertionSpec.Experimental",
            "registerAssertionSpec(AssertionSpec.Experimental);",
        ),
        // TriggerRecorder / Assertion — V2 trigger registration
        (
            "registerTxEndTrigger",
            "registerTxEndTrigger(this.c.selector);",
        ),
        (
            "registerFnCallTrigger",
            "registerFnCallTrigger(this.c.selector, ITarget.swap.selector);",
        ),
        (
            "registerErc20ChangeTrigger",
            "registerErc20ChangeTrigger(this.c.selector, token);",
        ),
        (
            "watchCumulativeInflow",
            "watchCumulativeInflow(token, 500, 1 hours, this.c.selector);",
        ),
        (
            "watchCumulativeOutflow",
            "watchCumulativeOutflow(token, 500, 1 hours, this.c.selector);",
        ),
        ("watchAnomaly", "watchAnomaly(target, this.c.selector);"),
        // Assertion — ForkId constructors
        ("_preTx", "_preTx();"),
        ("_postTx", "_postTx();"),
        ("_preCall", "_preCall(callId);"),
        ("_postCall", "_postCall(callId);"),
        // Assertion — V2 call matching helpers
        ("_matchingCalls", "_matchingCalls(target, sel, 10);"),
        ("_successOnlyFilter", "_successOnlyFilter();"),
        // PhEvm — fork-aware reads
        (
            "staticcallAt",
            "ph.staticcallAt(target, data, 100000, fork);",
        ),
        ("loadStateAt", "ph.loadStateAt(target, slot, fork);"),
        ("getLogsQuery", "ph.getLogsQuery(query, fork);"),
        ("getErc20Transfers", "ph.getErc20Transfers(token, fork);"),
        (
            "getErc20TransfersForTokens",
            "ph.getErc20TransfersForTokens(tokens, fork);",
        ),
        (
            "changedErc20BalanceDeltas",
            "ph.changedErc20BalanceDeltas(token, fork);",
        ),
        (
            "reduceErc20BalanceDeltas",
            "ph.reduceErc20BalanceDeltas(token, fork);",
        ),
        ("getTxObject", "ph.getTxObject();"),
        ("forbidChangeForSlot", "ph.forbidChangeForSlot(slot);"),
        ("forbidChangeForSlots", "ph.forbidChangeForSlots(slots);"),
        // PhEvm — call inspection
        ("callinputAt", "ph.callinputAt(callId);"),
        ("callOutputAt", "ph.callOutputAt(callId);"),
        (
            "matchingCalls",
            "ph.matchingCalls(target, sel, filter, 10);",
        ),
        ("getLogsForCall", "ph.getLogsForCall(query, callId);"),
        // PhEvm — trigger and flow context
        ("ph.context", "ph.context();"),
        ("inflowContext", "ph.inflowContext();"),
        ("outflowContext", "ph.outflowContext();"),
        ("inflowRate", "ph.inflowRate();"),
        ("outflowRate", "ph.outflowRate();"),
        (
            "verifyGnarkPlonkProof",
            "ph.verifyGnarkPlonkProof(proof, commitment, verifierKeyId);",
        ),
        // PhEvm — persistent assertion storage
        ("ph.store", "ph.store(key, value);"),
        ("ph.exists", "ph.exists(key);"),
        ("ph.values_left", "ph.values_left();"),
        // PhEvm — mapping tracing
        (
            "changedMappingKeys",
            "ph.changedMappingKeys(target, baseSlot);",
        ),
        (
            "mappingValueDiff",
            "ph.mappingValueDiff(target, baseSlot, key, 0);",
        ),
        // PhEvm — anomaly detection
        ("anomalyContext", "ph.anomalyContext(target);"),
        // PhEvm — protection suite
        (
            "assetsMatchSharePrice",
            "ph.assetsMatchSharePrice(vault, 10);",
        ),
        (
            "assetsMatchSharePriceAt",
            "ph.assetsMatchSharePriceAt(vault, 10, fork0, fork1);",
        ),
        (
            "conserveBalance",
            "ph.conserveBalance(fork0, fork1, token, account);",
        ),
        ("oracleSanity", "ph.oracleSanity(oracle, data, 100);"),
        (
            "oracleSanityAt",
            "ph.oracleSanityAt(oracle, data, 100, fork0, fork1);",
        ),
        ("normalizeDecimals", "ph.normalizeDecimals(amount, 6, 18);"),
        ("ratioGe", "ph.ratioGe(n1, d1, n2, d2, 10);"),
        // PhEvm — math precompiles
        ("ph.mulDivDown", "ph.mulDivDown(x, y, denominator);"),
        ("ph.mulDivUp", "ph.mulDivUp(x, y, denominator);"),
        // PhEvm — V2-only types
        ("PhEvm.ForkId", "PhEvm.ForkId memory fork;"),
        ("PhEvm.TriggerContext", "PhEvm.TriggerContext memory ctx;"),
        ("PhEvm.TriggerCall", "PhEvm.TriggerCall[] memory calls;"),
        ("PhEvm.CallFilter", "PhEvm.CallFilter memory filter;"),
        ("PhEvm.AnomalyContext", "PhEvm.AnomalyContext memory ctx;"),
        ("PhEvm.InflowContext", "PhEvm.InflowContext memory ctx;"),
        ("PhEvm.OutflowContext", "PhEvm.OutflowContext memory ctx;"),
        ("PhEvm.FlowRateContext", "PhEvm.FlowRateContext memory ctx;"),
        (
            "PhEvm.Erc20TransferData",
            "PhEvm.Erc20TransferData[] memory transfers;",
        ),
        (
            "PhEvm.StaticCallResult",
            "PhEvm.StaticCallResult memory result;",
        ),
        ("PhEvm.LogQuery", "PhEvm.LogQuery memory query;"),
        ("PhEvm.TxObject", "PhEvm.TxObject memory txObject;"),
    ];

    /// The marker lists and [`V2_SURFACE`] describe the same set, and each
    /// entry is detected on its own. A V2 API that reaches `credible-std`
    /// without reaching the marker lists is exactly the miss this guards: the
    /// assertion deploys to a V1 platform and never fires, unwarned.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn v2_surface_is_covered() {
        let declared: std::collections::BTreeSet<&str> =
            V2_CALL_MARKERS.into_iter().chain(V2_TYPE_MARKERS).collect();
        let exercised: std::collections::BTreeSet<&str> =
            V2_SURFACE.into_iter().map(|(marker, _)| marker).collect();
        assert_eq!(
            declared,
            exercised,
            "marker lists and V2_SURFACE disagree; missing from V2_SURFACE: {:?}, missing from the marker lists: {:?}",
            declared.difference(&exercised).collect::<Vec<_>>(),
            exercised.difference(&declared).collect::<Vec<_>>(),
        );

        for (marker, usage) in V2_SURFACE {
            let source =
                format!("contract A is Assertion {{ function f() external {{ {usage} }} }}");
            assert_eq!(
                detect_v2_markers(&source),
                vec![marker.to_string()],
                "`{usage}` should be detected as exactly {marker}"
            );
        }

        // The executor dependency is the authority for the spec gate. Walk
        // every selector in its generated PhEvm interface, ask Legacy whether
        // it is allowed, and require this marker mapping to cover the exact
        // rejected set. This fails when the pinned executor adds a non-Legacy
        // precompile without a corresponding source marker.
        #[cfg(feature = "credible")]
        {
            use alloy_sol_types::SolCall;
            use assertion_executor::{
                phevm::sol_abi::PhEvm,
                types::AssertionSpec,
            };

            let covered = [
                ("getTxObject", PhEvm::getTxObjectCall::SELECTOR),
                ("ph.context", PhEvm::contextCall::SELECTOR),
                ("outflowContext", PhEvm::outflowContextCall::SELECTOR),
                ("inflowContext", PhEvm::inflowContextCall::SELECTOR),
                ("anomalyContext", PhEvm::anomalyContextCall::SELECTOR),
                ("callinputAt", PhEvm::callinputAtCall::SELECTOR),
                ("callOutputAt", PhEvm::callOutputAtCall::SELECTOR),
                ("matchingCalls", PhEvm::matchingCallsCall::SELECTOR),
                ("loadStateAt", PhEvm::loadStateAt_0Call::SELECTOR),
                ("loadStateAt", PhEvm::loadStateAt_1Call::SELECTOR),
                ("getLogsQuery", PhEvm::getLogsQueryCall::SELECTOR),
                ("getLogsForCall", PhEvm::getLogsForCallCall::SELECTOR),
                (
                    "forbidChangeForSlot",
                    PhEvm::forbidChangeForSlotCall::SELECTOR,
                ),
                (
                    "forbidChangeForSlots",
                    PhEvm::forbidChangeForSlotsCall::SELECTOR,
                ),
                ("staticcallAt", PhEvm::staticcallAtCall::SELECTOR),
                ("conserveBalance", PhEvm::conserveBalanceCall::SELECTOR),
                ("getErc20Transfers", PhEvm::getErc20TransfersCall::SELECTOR),
                (
                    "getErc20TransfersForTokens",
                    PhEvm::getErc20TransfersForTokensCall::SELECTOR,
                ),
                (
                    "changedErc20BalanceDeltas",
                    PhEvm::changedErc20BalanceDeltasCall::SELECTOR,
                ),
                (
                    "reduceErc20BalanceDeltas",
                    PhEvm::reduceErc20BalanceDeltasCall::SELECTOR,
                ),
                (
                    "changedMappingKeys",
                    PhEvm::changedMappingKeysCall::SELECTOR,
                ),
                ("mappingValueDiff", PhEvm::mappingValueDiffCall::SELECTOR),
                ("ph.mulDivDown", PhEvm::mulDivDownCall::SELECTOR),
                ("ph.mulDivUp", PhEvm::mulDivUpCall::SELECTOR),
                ("normalizeDecimals", PhEvm::normalizeDecimalsCall::SELECTOR),
                ("ratioGe", PhEvm::ratioGeCall::SELECTOR),
                ("oracleSanity", PhEvm::oracleSanityCall::SELECTOR),
                ("oracleSanityAt", PhEvm::oracleSanityAtCall::SELECTOR),
                (
                    "assetsMatchSharePrice",
                    PhEvm::assetsMatchSharePriceCall::SELECTOR,
                ),
                (
                    "assetsMatchSharePriceAt",
                    PhEvm::assetsMatchSharePriceAtCall::SELECTOR,
                ),
                ("outflowRate", PhEvm::outflowRateCall::SELECTOR),
                ("inflowRate", PhEvm::inflowRateCall::SELECTOR),
                (
                    "verifyGnarkPlonkProof",
                    PhEvm::verifyGnarkPlonkProofCall::SELECTOR,
                ),
            ];
            let executor_non_legacy: std::collections::BTreeSet<[u8; 4]> =
                PhEvm::PhEvmCalls::SELECTORS
                    .iter()
                    .copied()
                    .filter(|selector| !AssertionSpec::Legacy.allows_selector(*selector))
                    .collect();
            let covered_selectors: std::collections::BTreeSet<[u8; 4]> =
                covered.iter().map(|(_, selector)| *selector).collect();

            assert_eq!(covered_selectors, executor_non_legacy);
            for (marker, _) in covered {
                assert!(
                    declared.contains(marker),
                    "executor-gated precompile `{marker}` has no source marker"
                );
            }
        }
    }

    #[test]
    fn overlapping_call_names_are_all_reported_when_both_are_called() {
        let markers = detect_v2_markers(
            "ph.assetsMatchSharePrice(vault, 10);
             ph.assetsMatchSharePriceAt(vault, 10, pre, post);
             ph.oracleSanity(oracle, data, 10);
             ph.oracleSanityAt(oracle, data, 10, pre, post);",
        );

        assert!(markers.contains(&"assetsMatchSharePrice".to_string()));
        assert!(markers.contains(&"assetsMatchSharePriceAt".to_string()));
        assert!(markers.contains(&"oracleSanity".to_string()));
        assert!(markers.contains(&"oracleSanityAt".to_string()));
    }

    #[test]
    fn shared_sources_are_read_once_per_unique_path() {
        let credible = credible_with(&[
            "assertions/src/Shared.a.sol:First",
            "assertions/src/Shared.a.sol:Second",
            "assertions/src/Shared.a.sol",
            "assertions/src/Other.a.sol",
        ]);

        assert_eq!(
            unique_assertion_paths(&credible),
            vec!["assertions/src/Shared.a.sol", "assertions/src/Other.a.sol"]
        );
    }

    #[test]
    fn scans_declared_sources_once_and_skips_missing_files() {
        let temp = project_with(&[(
            "assertions/src/V2.a.sol",
            "contract V2 is Assertion { function t() external { registerTxEndTrigger(this.c.selector); } }",
        )]);
        let credible = credible_with(&[
            "assertions/src/V2.a.sol:V2",
            "assertions/src/V2.a.sol",
            "assertions/src/Missing.a.sol",
        ]);

        let findings = scan_assertion_sources(temp.path(), &credible);

        assert_eq!(
            findings,
            vec![V2SpecFinding {
                file: "assertions/src/V2.a.sol".to_string(),
                markers: vec!["registerTxEndTrigger".to_string()],
            }]
        );
    }

    #[test]
    fn deploy_warning_fires_on_v1_platform_and_names_files() {
        let findings = vec![V2SpecFinding {
            file: "assertions/src/V2.a.sol".to_string(),
            markers: vec![
                "registerTxEndTrigger".to_string(),
                "staticcallAt".to_string(),
            ],
        }];

        let warning = deploy_warning(Some(&url("https://app.phylax.systems")), None, &findings)
            .expect("warning on the V1-only platform");
        assert!(warning.contains("V1 assertion spec"), "{warning}");
        assert!(warning.contains("assertions/src/V2.a.sol"), "{warning}");
        assert!(
            warning.contains("registerTxEndTrigger, staticcallAt"),
            "{warning}"
        );

        let with_chain = deploy_warning(
            Some(&url("https://app.phylax.systems")),
            Some(LINEA_MAINNET_CHAIN_ID),
            &findings,
        )
        .expect("warning names the chain");
        assert!(
            with_chain.contains("(Linea Mainnet, chain 59144)"),
            "{with_chain}"
        );
    }

    #[test]
    fn deploy_warning_fires_for_a_linea_chain_on_another_platform() {
        let findings = vec![V2SpecFinding {
            file: "a.sol".to_string(),
            markers: vec!["registerTxEndTrigger".to_string()],
        }];

        assert!(
            deploy_warning(
                Some(&url("https://dev.phylax.systems")),
                Some(LINEA_SEPOLIA_CHAIN_ID),
                &findings
            )
            .is_some()
        );
    }

    #[test]
    fn deploy_warning_stays_silent_without_v2_usage_or_on_a_v2_platform() {
        let findings = vec![V2SpecFinding {
            file: "a.sol".to_string(),
            markers: vec!["registerTxEndTrigger".to_string()],
        }];

        assert!(deploy_warning(Some(&url("https://app.phylax.systems")), None, &[]).is_none());
        assert!(
            deploy_warning(
                Some(&url("https://dev.phylax.systems")),
                Some(84532),
                &findings
            )
            .is_none()
        );
    }

    #[test]
    fn login_notice_only_targets_v1_platforms() {
        let notice = login_notice(&url("https://app.phylax.systems")).expect("notice");
        assert!(notice.contains("V1 assertion spec"), "{notice}");
        assert!(notice.contains("V2"), "{notice}");
        assert!(login_notice(&url("https://dev.phylax.systems")).is_none());
    }

    #[test]
    fn warning_json_carries_the_files_and_target() {
        let findings = vec![V2SpecFinding {
            file: "a.sol".to_string(),
            markers: vec!["staticcallAt".to_string()],
        }];
        let platform = url("https://app.phylax.systems");
        let message = deploy_warning(Some(&platform), Some(LINEA_MAINNET_CHAIN_ID), &findings)
            .expect("warning");

        let value = warning_json(
            &message,
            Some(&platform),
            Some(LINEA_MAINNET_CHAIN_ID),
            &findings,
        );

        assert_eq!(value["code"], V2_SPEC_UNSUPPORTED_CODE);
        assert_eq!(value["chain_id"], 59144);
        assert_eq!(value["files"][0]["file"], "a.sol");
        assert_eq!(value["files"][0]["markers"][0], "staticcallAt");
    }
}
