---
name: deploy-credible-assertions
description: Deploy a Credible Layer assertion project on-chain end-to-end with the pcl CLI, fully agentically. Use when asked to deploy assertions, create or activate an assertion release, set up pcl auth/RPC/wallet for deployment, set or transfer a protocol manager, or deactivate a release. Assumes assertions are already written (see write-protection-assertions for authoring).
---

# Deploying Credible Layer assertions with pcl

`pcl` deploys foundry-based assertion projects to the Credible Layer platform and
activates them on-chain. Every step observes state before acting, so **reruns
resume — they never duplicate work**. When something fails mid-deploy, fix the
cause and rerun the same command.

**Always pass `--json`.** It emits a structured envelope
(`schema_version: pcl.envelope.v1`) with `status`, `data`, and `next_actions`.
Success → exit 0; errors → an error envelope on stderr and exit 1. Machine mode
cannot prompt, so every mutating command needs explicit consent: `--yes` for
`deploy`/`apply`, `--broadcast` for calldata commands. `pcl --json --llms`
prints the CLI's own agent guide. Exception: `pcl test` and `pcl build` are
forge pass-throughs and reject `--json`.

## Project layout

A deployable project is a foundry repo with assertion contracts (conventionally
`*.a.sol`, inheriting `Assertion` from credible-std) and a manifest at
`assertions/credible.toml` (override with `--config`):

```toml
environment = "staging"           # or "production" — see "Environment and project name" below
project_name = "my-protocol-assertions"  # used to create the project on first deploy
# project_id = "uuid"             # absent on first deploy; pcl deploy writes it back

[contracts.my_vault]              # label = stable identifier, any snake_case name
address = "0x1234...5678"         # on-chain address of the protected contract
name = "My Vault"

[[contracts.my_vault.assertions]]
file = "src/VaultAssertion.a.sol" # or "src/File.sol:ContractName"
args = ["0xF31b...6071"]          # constructor args as strings; omit if none
```

No two contract labels may share an address.

## Prerequisites (once per environment)

### 1. Authenticate (device-code flow; human verifies in browser once)

```bash
PCL_API_URL=<platform-url> pcl auth ensure --json
```

Valid token → `status: ok`, nothing to do. Otherwise the envelope
(`status: action_required`) contains `device_url`, `code`, `session_id`,
`device_secret`, `expires_at`, and a ready-to-run `poll_command`.
**Show `device_url` and `code` to the human** and ask them to open the URL and
enter the code. Then run the `poll_command` verbatim (or):

```bash
pcl auth poll --session-id <id> --device-secret <secret> --expires-at <ts> --json
```

Poll returns `event: auth.login_pending` (`terminal: false`) until the human
verifies; sleep ~5s and repeat until `event: auth.login_complete`. Credentials
persist in `~/.config/pcl/config.toml`.

`PCL_API_URL` selects the platform (default `https://app.phylax.systems`);
set it for every pcl command against a non-default platform, or rely on the
default.

### 2. Store an RPC endpoint for the target chain

```bash
pcl config set-rpc <chain-id> <rpc-url> [--confirmations N]   # persisted in config.toml
```

Per-run override: `--rpc-url` / `PCL_RPC_URL`. Resolution: flag/env → stored
config → error telling you to run `set-rpc`. Confirmations: flag → stored →
default 1. pcl verifies the RPC actually serves the expected chain id before
signing.

### 3. Wallet

Either `export PCL_PRIVATE_KEY=<hex-key>`, or a foundry keystore:
`--account <name>` (from `~/.foundry/keystores`) plus its password. Machine
mode cannot prompt for the password — supply it via
`--keystore-password-file <path>` / `PCL_KEYSTORE_PASSWORD_FILE` (foundry
`--password-file` style; trailing newline is trimmed — **preferred**, since an
env var leaks into every child process) or `--keystore-password` /
`PCL_KEYSTORE_PASSWORD`. The wallet becomes the project's protocol manager,
so use the same wallet on every run for a given project.

## Every run: ensure auth freshness

```bash
pcl auth ensure --json
```

Silently refreshes the token when possible. If it returns
`status: action_required`, it includes a fresh device challenge — relay
`device_url` + `code` to the human and poll as above.

## Environment and project name

**Never deploy to production unless the user explicitly asked for production.**
The `environment` field in credible.toml decides where the release goes:

- If the user specified an environment, use it.
- Otherwise, ask the user whether to deploy to **staging** (default) or
  production before deploying.
- If you cannot ask (headless run, no response), use `staging`.
- If credible.toml already says `production` but the user did not explicitly
  request production, stop and confirm before deploying.

Edit `environment` in credible.toml before running `pcl deploy` — there is no
CLI flag for it.

**Always set `project_name` in credible.toml** when the project does not yet
exist (no `project_id`). Ask the user for a name; if you cannot ask, derive it
from the protocol/repository name (e.g. `my-protocol-assertions`) and tell the
user what you chose. The `--project-name` flag overrides the toml value if
both are present. The chain still comes from the CLI: pass `--chain-id <id>`
on the first `pcl deploy`.

## The one-shot path (preferred)

Dry-run first — builds and verifies locally, touches nothing remote:

```bash
pcl deploy --root <foundry-project> --dry-run --json
```

Then the real thing:

```bash
pcl deploy --root <foundry-project> --yes --json \
  [--chain-id <id>]   # required only on first deploy (credible.toml has no project_id yet)
```

One command runs the whole chain, observing state first at each step:

1. **Project** — `project_id` present → fetch it; absent → create from
   `project_name` in credible.toml (or `--project-name` override) and
   `--chain-id`, then **write `project_id` back into credible.toml**
   (commit this change).
2. **Protocol manager** — already the wallet → skip; unset → EIP-191-sign the
   challenge nonce and submit (off-chain, no gas); set to a *different*
   address → hard error (see recovery below).
3. **Build + verify** — forge-builds each assertion in credible.toml, runs
   local trigger-detection verification.
4. **Release** — an *inactive* release with identical contents exists →
   resume it (checked first: preview only diffs against the *active* release,
   so this is what makes reruns duplicate-safe); otherwise preview shows a
   diff → create a release; no diff and nothing pending → exit `up_to_date`.
5. **Checks** — polls the release until
   `checkSummary.deployBlockingStatus == "all_passed"`. `no_checks`/missing
   passes with a warning (dev/local). `has_failed` errors — inspect with
   `pcl releases show`, then `pcl releases retry-check <project> <release-id>
   <check-id> --json`. Timeout after `--check-timeout-secs` (default 600).
6. **On-chain** — fetches deploy calldata for the wallet; if `isNoop`, confirms
   with no transaction; else broadcasts `StateOracle.batch(...)` and waits for
   confirmations.
7. **Confirm** — reports the tx hash to the platform, activating the release.

`data.outcome` on success: `dry_run` | `up_to_date` | `resumed_and_deployed` |
`released_and_deployed`.

### Failure modes and recovery

| Symptom | Meaning | Action |
|---|---|---|
| error code `broadcast.confirm_failed` | Tx landed on-chain but platform confirm failed; tx hash is in the error | Rerun `pcl deploy` — it heals via the noop path |
| "Machine output requires `--yes`" | Mutation without consent flag | Add `--yes` |
| Protocol manager mismatch | Project managed by a different wallet | Use that wallet, or transfer (below); `--skip-protocol-manager` skips step 2 if managed out-of-band |
| Checks `has_failed` | A deploy-blocking check failed | `pcl releases show <project> <release-id> --json`, fix or `retry-check` |
| RPC url missing | No RPC for the chain | `pcl config set-rpc <chain-id> <url>` |
| Chain id mismatch | RPC serves a different chain | Fix the stored RPC url |

## Granular commands (when one step is needed)

```bash
pcl projects create --project-name <n> --chain-id <id> --json
pcl protocol-manager --project <ref> --set --sign --chain-id <id> --json   # off-chain, no gas
pcl apply --root <dir> --yes --json                        # build + create release only, no on-chain step
pcl releases list <ref> --json
pcl releases show <ref> <release-id> --json
pcl releases backtest-progress <ref> <release-id> --json
pcl releases calldata deploy <ref> <release-id> --broadcast --yes --json   # activate on-chain + confirm
pcl releases calldata remove <ref> <release-id> --broadcast --yes --json   # deactivate
pcl protocol-manager --project <ref> --transfer-calldata --new-manager 0x... --broadcast --yes --json
pcl protocol-manager --project <ref> --accept-calldata --broadcast --yes --json   # run as the new manager
```

`<ref>` is the project id (or saved reference). Without `--broadcast`, the
calldata commands print raw calldata for external signing
(`calldata deploy` then needs `--signer-address`).
