# PCL - The Credible CLI

The CLI for the [Credible Layer](https://phylax.systems).

## For Humans

Use this section when you are installing PCL, building assertions, deploying projects, or investigating platform state interactively.
The CLI is organized around the jobs people usually do in the Credible Layer: write assertions, verify them, ship them, and inspect what happened after deployment.

For repository development, see [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and [CHANGELOG.md](CHANGELOG.md).

### Install

```bash
brew install phylax/pcl/phylax
```

After installing, authenticate and check the environment:

```bash
pcl auth login
pcl doctor
pcl whoami
```

`pcl doctor` checks local configuration, platform connectivity, and CLI auth endpoint support. `pcl whoami` prints the account and platform context the CLI will use.

### Daily Workflow

For assertion development, the core loop is:

```bash
pcl build
pcl test
pcl verify
pcl apply --dry-run
pcl apply
```

To go all the way to an active on-chain deployment in one command — create/resolve the
project, set the protocol manager (wallet-signed challenge), create the release, wait for
checks, broadcast `StateOracle.batch`, and confirm — use `pcl deploy`:

```bash
# one-time: store an RPC endpoint for the chain
pcl config set-rpc <chain-id> <rpc-url> [--confirmations N]

pcl deploy --dry-run
pcl deploy --private-key $PCL_PRIVATE_KEY            # or --account <foundry-keystore>
pcl deploy --project-name my-protocol --chain-id <id> --private-key ... --yes --toon
```

`pcl deploy` is resumable: every step observes current state first, so re-running after a
failure (network, gas, interrupted confirmation) picks up where it left off instead of
duplicating work. Individual on-chain steps are also available on the underlying commands
via `--broadcast` (`pcl releases calldata deploy|remove`, `pcl protocol-manager
--transfer-calldata|--accept-calldata`) and `--sign` (`pcl protocol-manager --set`).

For platform work, prefer the natural workflow commands:

```bash
pcl projects mine
pcl projects show <project-ref>
pcl assertions --project-id <project-ref>
pcl incidents --project-id <project-ref> --environment production
pcl releases list <project-ref>
pcl deployments --project <project-ref>
```

When you need a payload for a write, ask the CLI for the shape before sending it:

```bash
pcl projects create --body-template
pcl assertions --project-id <project-ref> --body-template
pcl releases preview <project-ref> --body-template
```

When debugging or checking an endpoint that is not yet first-class, use the raw API surface.
If `pcl api list` or `pcl api inspect` returns `workflow_alternatives`, use that workflow command for normal work.

```bash
pcl api list --filter incidents
pcl api inspect get_views_projects_project_id_incidents
pcl incidents --project <project-id> --environment production
pcl api coverage --json
```

### Command Map

| Command | Description |
|---------|-------------|
| `pcl build`, `pcl test` | Developer pass-through commands using native Phorge/Foundry output in human mode |
| `pcl apply`, `pcl verify` | Structured assertion workflow commands for dry-runs, verification, and deployment prep |
| `pcl deploy` | End-to-end deploy: project, protocol manager, release, on-chain activation, confirmation |
| `pcl incidents`, `pcl projects`, `pcl assertions` | Natural platform workflow commands |
| `pcl account`, `pcl contracts`, `pcl releases`, `pcl deployments` | Account, contract, release, and deployment workflows |
| `pcl access`, `pcl integrations`, `pcl protocol-manager`, `pcl events`, `pcl search` | Access control, integrations, protocol manager transfer, audit, and search workflows |
| `pcl doctor`, `pcl whoami` | Diagnose local/API readiness, target auth capability, and identity state |
| `pcl workflows`, `pcl schema` | Workflow recipes and command/action schemas; agents should add `--json` |
| `pcl --json --llms`, `pcl llms --json` | Print the CLI-native LLM usage guide for agents |
| `pcl export`, `pcl jobs`, `pcl artifacts`, `pcl requests` | Export JSONL artifacts and inspect local jobs, artifacts, and request logs |
| `pcl completions` | Generate shell completion scripts |
| `pcl api` | Discover, inspect, call, and audit raw platform API endpoints |
| `pcl auth` | Authenticate with the Credible Layer platform |
| `pcl config` | Manage CLI configuration |
| `pcl download` | Download assertion source code for a protocol |
| `pcl test` | Run assertion tests |
| `pcl verify` | Verify assertions locally before deployment |

### Shell Setup

Generate completions for your shell:

```bash
pcl completions zsh > ~/.zfunc/_pcl
```

For long investigations, export artifacts instead of copying terminal output:

```bash
pcl export incidents \
  --project-id <project-ref> \
  --environment production \
  --out incidents.jsonl \
  --errors errors.jsonl \
  --resume
```

Then inspect resumable jobs and request history:

```bash
pcl jobs list
pcl jobs status <job-id>
pcl requests list --limit 20
```

## For Agents

Use this section when you are consuming PCL from an LLM, automation, script, or coding agent. It is written as a contract, not a tutorial.

Top-level workflow commands expose the platform API as structured CLI operations for agents and scripts.
`pcl api` remains the raw discovery and escape-hatch surface for uncovered endpoints.
The CLI is designed around the platform workflows documented in the [Phylax docs](https://docs.phylax.systems):
projects, assertions, transparency views, deployment state, integrations, and incidents.
API commands default to human-readable output for people. Agents and scripts should pass `--json`
to get structured envelopes with `status`, `data`, and `next_actions`. Successes and errors use the same shape,
so agents can recover from auth, validation, and parser failures without scraping prose diagnostics.
`pcl auth status` also reports token validity, expiry, and platform URL; expired stored tokens return
a nonzero structured error so agents do not mistake stale credentials for a working login.
For preflight checks, prefer `pcl auth ensure --json`: it returns `status: ok` when auth is usable,
or one `status: action_required` envelope with `device_url`, `code`, `device_secret`, and `poll_command`
when user login is needed. `pcl auth refresh --json` is safe to call and rotates the stored CLI
refresh token when available; if the refresh token is missing or rejected,
it returns the same login challenge shape.
Auth commands use `--auth-url`/`PCL_AUTH_URL` when set, otherwise they follow `PCL_API_URL`
before falling back to the production app URL.
When `expires_soon` is true, renew before long-running work with `pcl auth ensure --force --json`
or `pcl auth login --no-wait --json`.
`pcl auth logout` attempts remote logout first, then clears local credentials. Use
`pcl auth logout --local` only when you explicitly want local cleanup.
Repository-local agent instructions also live in [AGENTS.md](AGENTS.md).
The core discovery commands in this section are exercised by `make agent-smoke`, which is part of
`make ci`, so README/agent guidance should not drift from the CLI contract.

### Start Here

Start with CLI-native discovery. Do not scrape human help text unless the structured surfaces are missing the field you need.

1. `pcl --json --llms` for the current CLI-native agent guide.
2. `pcl doctor --json`, `pcl auth ensure --json`, and `pcl whoami --json` for readiness and token truthfulness.
3. `pcl workflows --json`, `pcl schema list --json`, and `pcl api manifest --json` for discovery.
4. Top-level workflow commands for normal work.
5. `pcl api list`, `pcl api inspect`, `pcl api call`, and `pcl api coverage` only for debugging, API parity checks, internal/service endpoints, or endpoints without `workflow_alternatives`.

### Output Contract

Every agent-facing command should be treated as an envelope. With `--json` it looks like:

```json
{
  "status": "ok",
  "data": {},
  "next_actions": [],
  "schema_version": "pcl.envelope.v1",
  "pcl_version": "..."
}
```

Errors use `status: "error"` with:

- `error.code`
- `error.message`
- `error.recoverable`
- optional `error.http`
- optional `request_id`
- `next_actions`

Default output is for humans. Use `--json` for agent and script consumption. Do not parse colored
or human prose output as a control plane.

`pcl auth login --json` is the one streaming exception: a fresh login emits JSONL events because the command must print device-login instructions and then wait for verification. Read each line as an envelope and trust only the event with `terminal: true` as the final result. If credentials are already valid, `pcl auth login --json` returns a single normal envelope. For normal agent flows, prefer a single envelope from `pcl auth ensure --json` or `pcl auth login --no-wait --json`, then run `data.poll_command`.

### Discovery Commands

```bash
pcl --json --llms
pcl doctor --json
pcl auth ensure --json
pcl whoami --json
pcl workflows --json
pcl workflows show incident-investigation --json
pcl schema list --json
pcl schema get incidents --action list_public --json
pcl api manifest --json
```

Use `--json` for these same commands only when strict JSON parsing is required.

### Workflow Commands

Prefer top-level commands before raw API calls:

```bash
pcl incidents --limit 5 --json
pcl incidents --project-id <project-ref> --environment production --json
pcl incidents --project-id <project-ref> --all --limit 50 --output incidents.json --json
pcl incidents --incident-id <incident-id> --json
pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace --json
pcl projects list --limit 10 --json
pcl projects show <project-ref> --json
pcl projects create --body-template --json
pcl projects update <project-ref> --body-template --json
pcl assertions --project-id <project-ref> --json
pcl assertions --adopter-address 0x... --network 1 --json
pcl account --json
pcl contracts --project <project-ref> --json
pcl releases list <project-ref> --json
pcl deployments --project <project-ref> --json
pcl access members <project-ref> --json
pcl integrations --project <project-ref> --provider slack --json
pcl protocol-manager --project <project-ref> --pending-transfer --json
pcl events --project <project-ref> --audit-log --json
pcl search --query settler --json
```

### Mutation Rules

Use `--body-template` before constructing nested mutation payloads.
Prefer typed flags, then `--field key=value`, then `--body-file` for nested payloads.

```bash
pcl projects create --body-template --json
pcl assertions --project-id <project-ref> --body-template --json
pcl releases preview <project-ref> --body-template --json
pcl access role update <project-ref> <user-id> --body-template --json
pcl protocol-manager --project <project-ref> --confirm-transfer --body-template --json
pcl api inspect post_projects --json
```

For complex bodies:

1. Get the template with `--body-template --json`.
2. Fill the returned body into a file.
3. Run the write with `--body-file <file> --json` once the payload is correct.

### Raw API Fallback

Raw calls are an escape hatch for debugging, API parity checks, internal/service endpoints, browser-session bridge investigation, and new endpoint exploration before a workflow is promoted.
For normal product work, inspect `workflow_alternatives` and use the advertised workflow command instead of `pcl api call`.
Call any endpoint below `/api/v1`. Query strings and repeated `--query` flags are both valid.
Known public raw paths do not attach stored tokens by default; pass `--allow-unauthenticated` when you need to force no auth on another public endpoint.
For simple JSON object bodies, repeated `--field key=value` works on raw `pcl api call` the same way it works on workflow commands.

```bash
pcl api list --filter integrations --json
pcl api inspect get_views_projects_project_id_incidents --json
pcl incidents --limit 5 --json
pcl projects create --body-template --json
pcl incidents --project <project-id> --environment production --json
pcl incidents --all --limit 50 --output incidents.json --json
pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --resume --json
pcl assertions --project <project-id> --json
pcl account --logout
```

`pcl api inspect` reports `workflow_alternatives`, `raw_api_use`, auth metadata, and required header placeholders so agents can avoid manual raw calls unless they are intentionally debugging. Raw API calls record `operation_id` in request history when it can be resolved from OpenAPI; use `pcl api coverage --json` or `pcl api coverage --markdown api-coverage.md --json` after an exploration run to find untested endpoints, endpoints hit without a 2xx, and side-effecting operations that need reconciliation.

### Jobs, Artifacts, And Provenance

For long investigations, use JSONL exports, checkpoint files, and `pcl jobs` instead of rebuilding pagination or retry state manually.

```bash
pcl export incidents --project-id <project-ref> --environment production --out incidents.jsonl --errors errors.jsonl --checkpoint checkpoint.json --resume --continue-on-error --json
pcl jobs path --json
pcl jobs list --json
pcl jobs status <job-id> --json
pcl jobs resume <job-id> --json
pcl artifacts list --json
pcl requests list --limit 20 --json
```

When an agent reports a derived result, preserve the command, artifact path, request ID, project ID,
incident ID, transaction hash, and trace context that produced it. `pcl requests list --json` recovers
recent request IDs and HTTP statuses; export outputs include `job_id`, `resume_command`, checkpoint,
output, and error file paths.

## Development

```bash
# Build
cargo build --workspace

# Run tests
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets

# Regenerate API client from latest OpenAPI spec
make regenerate
```

## License

BSL 1.1 — see [LICENSE](LICENSE) for details.
