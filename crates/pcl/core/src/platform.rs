//! Platform (network) resolution for every command that talks to the API.
//!
//! `pcl` deliberately has no compiled-in default platform. A hardcoded default
//! pins one network into the binary, and the moment that network moves, the
//! released CLI silently points every request at a host that no longer serves
//! the dApp. Users upgrade via `brew` on their own schedule, so a bad default
//! outlives the fix that removes it. A stale interactive *list* still works; a
//! stale *default* does not.
//!
//! Resolution order:
//!
//! 1. `-u` / `--api-url` / `--auth-url`, or `PCL_API_URL` / `PCL_AUTH_URL`
//!    (clap populates flag and env into the same `Option<Url>`). Any URL is
//!    accepted, so shadow and staging targets keep working.
//! 2. The platform remembered from the last login or selection.
//! 3. On a terminal, with human output: pick between the production networks.
//!    The choice is remembered, so the prompt is one-time.
//! 4. Otherwise: a hard error naming `-u` and `PCL_API_URL`. Never a hanging
//!    prompt in CI.

use crate::{
    config::CliConfig,
    error::PlatformError,
};
use inquire::Select;
use std::{
    fmt,
    io::{
        IsTerminal,
        stdin,
        stdout,
    },
};
pub use url::Url;

/// A production network offered by the interactive selector.
///
/// The list is deliberately hardcoded and deliberately short: it exists only
/// so a first-run user can get to a working platform without reading docs.
/// Fetching it from the selector page would make the CLI depend on the very
/// host it is trying to discover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    EthereumMainnet,
    LineaMainnet,
}

/// Every network the selector offers, in display order.
pub const SELECTABLE_NETWORKS: [Network; 2] = [Network::EthereumMainnet, Network::LineaMainnet];

impl Network {
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::EthereumMainnet => "Ethereum Mainnet",
            Self::LineaMainnet => "Linea Mainnet",
        }
    }

    pub const fn host(self) -> &'static str {
        match self {
            Self::EthereumMainnet => "ethereum.phylax.systems",
            Self::LineaMainnet => "linea.phylax.systems",
        }
    }

    /// Platform URL for this network.
    ///
    /// # Panics
    ///
    /// Never in practice: the hosts are compile-time constants that form valid
    /// `https` URLs, and `selector_hosts_are_valid_urls` covers every variant.
    pub fn url(self) -> Url {
        let host = self.host();
        format!("https://{host}")
            .parse()
            .expect("network host forms a valid https URL")
    }

    /// Recognises a resolved URL as one of the known networks, so it can be
    /// named in output. Unknown hosts (shadow, staging, localhost) return
    /// `None` and are shown as-is.
    pub fn from_url(url: &Url) -> Option<Self> {
        let host = url.host_str()?;
        SELECTABLE_NETWORKS
            .into_iter()
            .find(|network| network.host() == host)
    }
}

impl fmt::Display for Network {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({})", self.display_name(), self.host())
    }
}

/// Human label for a resolved platform: the network name when the host is a
/// known network, otherwise the bare URL.
pub fn describe_platform(url: &Url) -> String {
    Network::from_url(url).map_or_else(|| trim_platform_url(url), |network| network.to_string())
}

/// Canonical stored form of a platform URL — trailing slash removed so
/// comparisons against remembered values and credential platforms are stable.
pub fn trim_platform_url(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_string()
}

/// Whether the resolver is allowed to prompt when nothing is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    /// A terminal is attached and output is human — the selector may prompt.
    Prompt,
    /// Machine output or no terminal — resolution must fail rather than hang.
    Forbidden,
}

impl Interaction {
    /// Prompting requires both a terminal to draw on and a human-output run:
    /// `--json` is machine consumption even from an interactive shell.
    pub fn detect(json_output: bool) -> Self {
        if !json_output && stdin().is_terminal() && stdout().is_terminal() {
            Self::Prompt
        } else {
            Self::Forbidden
        }
    }
}

/// Where a resolved platform came from. Determines whether the choice needs
/// writing back to the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSource {
    /// An explicit `-u`/`--api-url`/`--auth-url` flag or `PCL_*` env var.
    Explicit,
    /// The platform remembered from a previous login or selection.
    Remembered,
    /// Freshly picked from the interactive selector.
    Selected,
}

/// A resolved platform and its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub url: Url,
    pub source: PlatformSource,
}

/// Resolves the platform URL for this invocation, delegating the interactive
/// pick to `select` so the precedence rules can be tested without a terminal.
pub fn resolve_platform_url_with<S>(
    explicit: Option<&Url>,
    remembered: Option<&str>,
    interaction: Interaction,
    select: S,
) -> Result<Resolution, PlatformError>
where
    S: FnOnce() -> Result<Network, PlatformError>,
{
    if let Some(url) = explicit {
        return Ok(Resolution {
            url: url.clone(),
            source: PlatformSource::Explicit,
        });
    }

    if let Some(remembered) = remembered
        && let Ok(url) = remembered.parse::<Url>()
    {
        return Ok(Resolution {
            url,
            source: PlatformSource::Remembered,
        });
    }

    if interaction == Interaction::Forbidden {
        return Err(PlatformError::NoPlatformResolved {
            networks: network_list(),
        });
    }

    Ok(Resolution {
        url: select()?.url(),
        source: PlatformSource::Selected,
    })
}

/// Resolves the platform URL, prompting interactively when allowed.
pub fn resolve_platform_url(
    explicit: Option<&Url>,
    remembered: Option<&str>,
    interaction: Interaction,
) -> Result<Resolution, PlatformError> {
    resolve_platform_url_with(explicit, remembered, interaction, select_network)
}

/// Resolves the platform for this invocation and records the choice in
/// `config` when it must be remembered.
///
/// A fresh selection is always remembered, so the prompt is one-time. An
/// explicit `-u` is remembered only when `persist_explicit` is set — that is,
/// only for `pcl auth login`. On every other command `-u` is a one-shot
/// override that must not move the user's platform out from under them.
pub fn resolve_for_invocation(
    explicit: Option<&Url>,
    config: &mut CliConfig,
    persist_explicit: bool,
    interaction: Interaction,
) -> Result<Url, PlatformError> {
    resolve_for_invocation_with(
        explicit,
        config,
        persist_explicit,
        interaction,
        select_network,
    )
}

/// [`resolve_for_invocation`] with an injectable selector, so the persistence
/// rules can be tested without a terminal.
pub fn resolve_for_invocation_with<S>(
    explicit: Option<&Url>,
    config: &mut CliConfig,
    persist_explicit: bool,
    interaction: Interaction,
    select: S,
) -> Result<Url, PlatformError>
where
    S: FnOnce() -> Result<Network, PlatformError>,
{
    let resolution = resolve_platform_url_with(
        explicit,
        config.platform_url.as_deref(),
        interaction,
        select,
    )?;
    let should_remember = match resolution.source {
        PlatformSource::Selected => true,
        PlatformSource::Explicit => persist_explicit,
        PlatformSource::Remembered => false,
    };
    if should_remember {
        config.platform_url = Some(trim_platform_url(&resolution.url));
    }
    Ok(resolution.url)
}

fn select_network() -> Result<Network, PlatformError> {
    let options: Vec<String> = SELECTABLE_NETWORKS
        .into_iter()
        .map(|network| network.to_string())
        .collect();
    let selected = Select::new("Select the network to use:", options)
        .with_help_message("Remembered for future commands. Skip with -u <url>.")
        .prompt()
        .map_err(PlatformError::SelectionFailed)?;

    SELECTABLE_NETWORKS
        .into_iter()
        .find(|network| network.to_string() == selected)
        .ok_or_else(|| {
            PlatformError::NoPlatformResolved {
                networks: network_list(),
            }
        })
}

/// The networks named in the non-interactive error, so a CI failure tells the
/// operator exactly what to pass.
fn network_list() -> String {
    SELECTABLE_NETWORKS
        .into_iter()
        .map(|network| format!("{} ({})", network.display_name(), network.url()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Storage for the platform resolved at startup.
///
/// In a real run this is set once, before dispatch, and never changes. Unit
/// tests run concurrently in a single process, so test builds keep it
/// per-thread: each test installs its own platform without racing the others.
#[cfg(not(test))]
mod active {
    use super::Url;
    use std::sync::OnceLock;

    static ACTIVE_PLATFORM: OnceLock<Url> = OnceLock::new();

    pub(super) fn set(url: Url) {
        let _ = ACTIVE_PLATFORM.set(url);
    }

    pub(super) fn get() -> Option<Url> {
        ACTIVE_PLATFORM.get().cloned()
    }
}

#[cfg(test)]
mod active {
    use super::Url;
    use std::cell::RefCell;

    thread_local! {
        static ACTIVE_PLATFORM: RefCell<Option<Url>> = const { RefCell::new(None) };
    }

    pub(super) fn set(url: Url) {
        ACTIVE_PLATFORM.with(|slot| *slot.borrow_mut() = Some(url));
    }

    pub(super) fn get() -> Option<Url> {
        ACTIVE_PLATFORM.with(|slot| slot.borrow().clone())
    }
}

/// Records the platform resolved during startup so argument accessors can read
/// it without re-resolving — and without prompting a second time.
pub fn set_active_platform(url: Url) {
    active::set(url);
}

/// The platform resolved during startup.
///
/// # Panics
///
/// Panics when startup resolved no platform. Every command that reports
/// `needs_platform_url` gets one before dispatch, so this fires only if an
/// argument accessor runs for a command that declared it needs no platform.
pub fn active_platform() -> Url {
    active::get()
        .expect("startup resolves the platform URL before dispatching a command that needs one")
}

/// The platform resolved during startup, if any.
///
/// For commands that report on a platform without needing one — `pcl auth
/// status` answering "am I logged in?" before any platform has been chosen.
pub fn active_platform_opt() -> Option<Url> {
    active::get()
}

/// The platform an argument struct should use: its own explicit flag when
/// given, otherwise the platform resolved at startup.
pub fn platform_url_or_active(explicit: Option<&Url>) -> Url {
    explicit.cloned().unwrap_or_else(active_platform)
}

/// The platform resolved at startup, as a recoverable error when none exists.
///
/// For paths a command reaches only sometimes — `pcl apply --dry-run` needs a
/// platform only when it has to pick a project, which is not knowable before
/// `credible.toml` is read.
pub fn require_active_platform() -> Result<Url, PlatformError> {
    active_platform_opt().ok_or_else(|| {
        PlatformError::NoPlatformResolved {
            networks: network_list(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        value.parse().expect("test URL parses")
    }

    fn unreachable_select() -> Result<Network, PlatformError> {
        panic!("selector must not run when a platform is already resolvable")
    }

    #[test]
    fn networks_render_name_and_host() {
        assert_eq!(
            Network::EthereumMainnet.to_string(),
            "Ethereum Mainnet (ethereum.phylax.systems)"
        );
        assert_eq!(
            Network::LineaMainnet.to_string(),
            "Linea Mainnet (linea.phylax.systems)"
        );
    }

    #[test]
    fn selector_offers_exactly_the_two_production_networks() {
        assert_eq!(
            SELECTABLE_NETWORKS,
            [Network::EthereumMainnet, Network::LineaMainnet]
        );
    }

    #[test]
    fn network_urls_are_https_hosts() {
        assert_eq!(
            Network::EthereumMainnet.url().as_str(),
            "https://ethereum.phylax.systems/"
        );
        assert_eq!(
            Network::LineaMainnet.url().as_str(),
            "https://linea.phylax.systems/"
        );
    }

    #[test]
    fn known_hosts_are_named_and_custom_hosts_are_shown_verbatim() {
        assert_eq!(
            describe_platform(&url("https://linea.phylax.systems")),
            "Linea Mainnet (linea.phylax.systems)"
        );
        assert_eq!(
            describe_platform(&url("https://shadow.phylax.example/")),
            "https://shadow.phylax.example"
        );
    }

    #[test]
    fn explicit_url_wins_over_remembered() {
        let explicit = url("https://shadow.phylax.example");
        let resolution = resolve_platform_url_with(
            Some(&explicit),
            Some("https://linea.phylax.systems"),
            Interaction::Prompt,
            unreachable_select,
        )
        .expect("explicit URL resolves");

        assert_eq!(resolution.url, explicit);
        assert_eq!(resolution.source, PlatformSource::Explicit);
    }

    #[test]
    fn explicit_url_is_accepted_without_being_a_known_network() {
        // The internal team targets shadow and staging via -u; the selector
        // must never restrict what an explicit flag accepts.
        let explicit = url("http://localhost:3000");
        let resolution = resolve_platform_url_with(
            Some(&explicit),
            None,
            Interaction::Forbidden,
            unreachable_select,
        )
        .expect("arbitrary explicit URL resolves even without a terminal");

        assert_eq!(resolution.url, explicit);
    }

    #[test]
    fn remembered_url_is_used_when_no_flag_is_given() {
        let resolution = resolve_platform_url_with(
            None,
            Some("https://ethereum.phylax.systems"),
            Interaction::Forbidden,
            unreachable_select,
        )
        .expect("remembered URL resolves without prompting");

        assert_eq!(resolution.url, url("https://ethereum.phylax.systems"));
        assert_eq!(resolution.source, PlatformSource::Remembered);
    }

    #[test]
    fn unparseable_remembered_url_falls_through_to_selection() {
        let resolution =
            resolve_platform_url_with(None, Some("not-a-url"), Interaction::Prompt, || {
                Ok(Network::LineaMainnet)
            })
            .expect("corrupt remembered value falls through to the selector");

        assert_eq!(resolution.source, PlatformSource::Selected);
        assert_eq!(resolution.url, Network::LineaMainnet.url());
    }

    #[test]
    fn nothing_resolved_without_a_terminal_is_an_error_naming_the_overrides() {
        let error =
            resolve_platform_url_with(None, None, Interaction::Forbidden, unreachable_select)
                .expect_err("non-interactive runs must fail rather than prompt");

        let message = error.to_string();
        assert!(message.contains("-u"), "should name -u: {message}");
        assert!(
            message.contains("PCL_API_URL"),
            "should name PCL_API_URL: {message}"
        );
        assert!(
            message.contains("ethereum.phylax.systems") && message.contains("linea.phylax.systems"),
            "should list both networks: {message}"
        );
    }

    #[test]
    fn nothing_resolved_on_a_terminal_prompts() {
        let resolution = resolve_platform_url_with(None, None, Interaction::Prompt, || {
            Ok(Network::EthereumMainnet)
        })
        .expect("selection resolves");

        assert_eq!(resolution.source, PlatformSource::Selected);
        assert_eq!(resolution.url, Network::EthereumMainnet.url());
    }

    #[test]
    fn json_output_never_prompts_even_on_a_terminal() {
        // A --json run is machine consumption; a prompt would corrupt it.
        assert_eq!(Interaction::detect(true), Interaction::Forbidden);
    }

    #[test]
    fn a_fresh_selection_is_always_remembered() {
        // The prompt has to be one-time, so the pick is recorded even on a
        // command that never persists an explicit `-u`.
        let mut config = CliConfig::default();
        let resolved =
            resolve_for_invocation_with(None, &mut config, false, Interaction::Prompt, || {
                Ok(Network::EthereumMainnet)
            })
            .expect("selection resolves");

        assert_eq!(resolved, Network::EthereumMainnet.url());
        assert_eq!(
            config.platform_url.as_deref(),
            Some("https://ethereum.phylax.systems")
        );
    }

    #[test]
    fn explicit_url_is_one_shot_off_the_login_path() {
        let mut config = CliConfig {
            platform_url: Some("https://linea.phylax.systems".to_string()),
            ..CliConfig::default()
        };
        let explicit = url("https://shadow.phylax.example");

        let resolved =
            resolve_for_invocation(Some(&explicit), &mut config, false, Interaction::Forbidden)
                .expect("explicit URL resolves");

        assert_eq!(resolved, explicit);
        assert_eq!(
            config.platform_url.as_deref(),
            Some("https://linea.phylax.systems"),
            "a one-shot -u must not move the remembered platform"
        );
    }

    #[test]
    fn explicit_url_persists_on_the_login_path() {
        let mut config = CliConfig {
            platform_url: Some("https://linea.phylax.systems".to_string()),
            ..CliConfig::default()
        };
        let explicit = url("https://shadow.phylax.example/");

        let resolved =
            resolve_for_invocation(Some(&explicit), &mut config, true, Interaction::Forbidden)
                .expect("explicit URL resolves");

        assert_eq!(resolved, explicit);
        assert_eq!(
            config.platform_url.as_deref(),
            Some("https://shadow.phylax.example"),
            "pcl auth login records the platform it logged into"
        );
    }

    #[test]
    fn resolving_from_the_remembered_platform_leaves_the_config_untouched() {
        let mut config = CliConfig {
            platform_url: Some("https://linea.phylax.systems".to_string()),
            ..CliConfig::default()
        };
        let before = config.clone();

        let resolved = resolve_for_invocation(None, &mut config, true, Interaction::Forbidden)
            .expect("remembered URL resolves");

        assert_eq!(resolved, url("https://linea.phylax.systems"));
        assert_eq!(config, before, "a no-op resolve must not dirty the config");
    }

    #[test]
    fn active_platform_is_absent_until_startup_resolves_one() {
        assert!(active_platform_opt().is_none());
        assert!(require_active_platform().is_err());

        set_active_platform(Network::LineaMainnet.url());

        assert_eq!(active_platform_opt(), Some(Network::LineaMainnet.url()));
        assert_eq!(
            require_active_platform().expect("resolved"),
            Network::LineaMainnet.url()
        );
        // An explicit flag wins over the startup platform.
        let explicit = url("https://shadow.phylax.example");
        assert_eq!(platform_url_or_active(Some(&explicit)), explicit);
        assert_eq!(platform_url_or_active(None), Network::LineaMainnet.url());
    }
}
