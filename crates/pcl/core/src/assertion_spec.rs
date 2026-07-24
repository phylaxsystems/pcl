//! Which assertion spec a target platform runs, and whether a project's
//! assertions are written against the V2 spec.
//!
//! The production platform (`app.phylax.systems`, Linea) runs the V1 assertion
//! spec. Assertions written against the V2 triggers and precompiles
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
const V1_ONLY_PLATFORM_HOSTS: [&str; 1] = ["app.phylax.systems"];

/// Warning code used in machine output.
pub const V2_SPEC_UNSUPPORTED_CODE: &str = "assertion_spec.v2_unsupported";

/// V2-only identifiers called as functions: triggers, precompiles, and the
/// fork helpers on `Assertion`. A needle containing `.` (`ph.context`) is
/// matched with the prefix, because the bare identifier is too common to be
/// evidence of anything.
const V2_CALL_MARKERS: [&str; 30] = [
    // Triggers
    "registerTxEndTrigger",
    "registerFnCallTrigger",
    "registerErc20ChangeTrigger",
    "watchCumulativeInflow",
    "watchCumulativeOutflow",
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
    "matchingCalls",
    "getLogsForCall",
    "getLogsQuery",
    "getErc20TransfersForTokens",
    "getErc20Transfers",
    "changedErc20BalanceDeltas",
    "reduceErc20BalanceDeltas",
    // Trigger context
    "ph.context",
    "inflowContext",
    "outflowContext",
    // Protection-suite precompiles
    "assetsMatchSharePriceAt",
    "assetsMatchSharePrice",
    "conserveBalance",
    "oracleSanityAt",
    "oracleSanity",
    "normalizeDecimals",
    "ratioGe",
];

/// V2-only types, used in declarations rather than calls.
const V2_TYPE_MARKERS: [&str; 2] = ["PhEvm.ForkId", "PhEvm.TriggerContext"];

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
/// marker lists. A marker that is a substring of another match is dropped, so
/// `ph.assetsMatchSharePriceAt(...)` reports the `At` form only.
pub fn detect_v2_markers(source: &str) -> Vec<String> {
    let code = strip_solidity_comments(source);
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
        .iter()
        .filter(|marker| {
            !found
                .iter()
                .any(|other| other.len() > marker.len() && other.contains(marker.as_str()))
        })
        .cloned()
        .collect()
}

/// Scans the assertion sources declared in `credible.toml` for V2 spec usage.
/// Unreadable files are skipped: a warning check must never be the reason a
/// deploy fails, and the build step reports missing sources with a real error.
pub fn scan_assertion_sources(root: &Path, credible: &CredibleToml) -> Vec<V2SpecFinding> {
    let mut findings: Vec<V2SpecFinding> = Vec::new();

    for contract in credible.contracts.values() {
        for assertion in &contract.assertions {
            let relative = assertion
                .file
                .split_once(':')
                .map_or(assertion.file.as_str(), |(path, _)| path);
            if findings.iter().any(|finding| finding.file == relative) {
                continue;
            }
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
    }

    findings
}

/// The V2-unsupported warning for a deploy, or `None` when the target platform
/// runs V2 or the project's assertions do not use it.
///
/// `chain_id` is `None` when the target chain is not known yet (a `--dry-run`
/// without `--chain-id`); the platform check alone still decides.
pub fn deploy_warning(
    platform_url: &Url,
    chain_id: Option<u64>,
    findings: &[V2SpecFinding],
) -> Option<String> {
    if findings.is_empty() {
        return None;
    }
    let platform_only_v1 = !platform_supports_v2(platform_url);
    let chain_only_v1 = chain_id.is_some_and(|chain_id| !chain_supports_v2(chain_id));
    if !platform_only_v1 && !chain_only_v1 {
        return None;
    }

    let platform = platform_url.as_str().trim_end_matches('/');
    let target = match chain_id.filter(|chain_id| !chain_supports_v2(*chain_id)) {
        Some(chain_id) => format!("{platform} ({})", chain_label(chain_id)),
        None => platform.to_string(),
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
pub fn warning_json(
    message: &str,
    platform_url: &Url,
    chain_id: Option<u64>,
    findings: &[V2SpecFinding],
) -> Value {
    json!({
        "code": V2_SPEC_UNSUPPORTED_CODE,
        "message": message,
        "platform_url": platform_url.as_str(),
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
        "{} (Linea) runs the V1 assertion spec. Do not write assertions against the V2 spec: V2 triggers and precompiles (registerTxEndTrigger, registerFnCallTrigger, ph.staticcallAt, the cumulative-flow circuit breakers) are not supported on this platform.",
        platform_url.as_str().trim_end_matches('/'),
    ))
}

/// Machine-output form of [`login_notice`].
pub fn login_warning_json(message: &str, platform_url: &Url) -> Value {
    json!({
        "code": V2_SPEC_UNSUPPORTED_CODE,
        "message": message,
        "platform_url": platform_url.as_str(),
        "assertion_spec": "v1",
    })
}

/// Replaces Solidity comments with spaces so a commented-out V2 call is not
/// read as usage. Byte positions are preserved, but only presence matters.
fn strip_solidity_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"//") {
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
    fn production_and_linea_are_v1_only() {
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

        let warning = deploy_warning(&url("https://app.phylax.systems"), None, &findings)
            .expect("warning on the V1-only platform");
        assert!(warning.contains("V1 assertion spec"), "{warning}");
        assert!(warning.contains("assertions/src/V2.a.sol"), "{warning}");
        assert!(
            warning.contains("registerTxEndTrigger, staticcallAt"),
            "{warning}"
        );

        let with_chain = deploy_warning(
            &url("https://app.phylax.systems"),
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
                &url("https://dev.phylax.systems"),
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

        assert!(deploy_warning(&url("https://app.phylax.systems"), None, &[]).is_none());
        assert!(
            deploy_warning(&url("https://dev.phylax.systems"), Some(84532), &findings).is_none()
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
        let message =
            deploy_warning(&platform, Some(LINEA_MAINNET_CHAIN_ID), &findings).expect("warning");

        let value = warning_json(&message, &platform, Some(LINEA_MAINNET_CHAIN_ID), &findings);

        assert_eq!(value["code"], V2_SPEC_UNSUPPORTED_CODE);
        assert_eq!(value["chain_id"], 59144);
        assert_eq!(value["files"][0]["file"], "a.sol");
        assert_eq!(value["files"][0]["markers"][0], "staticcallAt");
    }
}
