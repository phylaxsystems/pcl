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

`pcl doctor` checks local configuration and platform connectivity. `pcl whoami` prints the account and platform context the CLI will use.

### Daily Workflow

For assertion development, the core loop is:

```bash
pcl build
pcl test
pcl verify
pcl apply --dry-run
pcl apply
```

For platform work, prefer the natural workflow commands:

```bash
pcl projects --home
pcl projects --project-id <project-ref>
pcl assertions --project-id <project-ref>
pcl incidents --project-id <project-ref> --environment production
pcl releases --project <project-ref>
pcl deployments --project <project-ref>
```

When you need a payload for a write, ask the CLI for the shape before sending it:

```bash
pcl projects --body-template
pcl assertions --project-id <project-ref> --body-template
pcl releases --project <project-ref> --body-template
```

When a workflow is not yet first-class, use the raw API surface:

```bash
pcl api list --filter incidents
pcl api inspect get_views_projects_project_id_incidents
pcl api call get /views/projects/<project-id>/incidents --query environment=production
```

### Command Map

| Command | Description |
|---------|-------------|
| `pcl build` | Build assertion contracts |
| `pcl apply` | Preview and apply declarative deployment changes |
| `pcl incidents`, `pcl projects`, `pcl assertions` | Natural platform workflow commands |
| `pcl account`, `pcl contracts`, `pcl releases`, `pcl deployments` | Account, contract, release, and deployment workflows |
| `pcl access`, `pcl integrations`, `pcl protocol-manager`, `pcl transfers`, `pcl events`, `pcl search` | Access control, integrations, protocol manager, transfer, audit, and search workflows |
| `pcl doctor`, `pcl whoami` | Diagnose local/API readiness and inspect identity state |
| `pcl workflows`, `pcl schema` | Agent-facing workflow recipes and command/action schemas |
| `pcl --llms`, `pcl llms` | Print the CLI-native LLM usage guide |
| `pcl export`, `pcl jobs`, `pcl artifacts`, `pcl requests` | Export JSONL artifacts and inspect local jobs, artifacts, and request logs |
| `pcl completions` | Generate shell completion scripts |
| `pcl api` | Discover, inspect, and call raw platform API endpoints |
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
API commands default to compact TOON-style envelopes with `status`, `data`, and `next_actions`;
pass `--json` for the same machine-readable envelope as JSON. Successes and errors use the same shape, so agents can recover from auth, validation, and parser failures without scraping prose diagnostics.
`pcl auth status` also reports token validity, expiry, and platform URL; expired stored tokens return
a nonzero structured error so agents do not mistake stale credentials for a working login.
When `expires_soon` is true, renew before long-running work with `pcl auth login --force --json`.
`pcl auth logout` revokes the platform session when possible before deleting local credentials;
use `pcl auth logout --local` for local-only cleanup.
Repository-local agent instructions also live in [AGENTS.md](AGENTS.md).

### Start Here

Start with CLI-native discovery. Do not scrape human help text unless the structured surfaces are missing the field you need.

1. `pcl --llms` for the current CLI-native agent guide.
2. `pcl doctor` and `pcl whoami` for readiness and token truthfulness.
3. `pcl workflows`, `pcl schema list`, and `pcl api manifest --json` for discovery.
4. Top-level workflow commands for normal work.
5. `pcl api list`, `pcl api inspect`, and `pcl api call` for raw OpenAPI fallback.

### Output Contract

Every machine-facing command is an envelope. With `--json`, expect:

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

Default output is TOON for compact agent consumption. Use `--json` when you need strict JSON parsing. Do not parse colored or human prose output as a control plane.

`pcl auth login --json` is the one streaming exception: a fresh login emits JSONL events because the command must print device-login instructions and then wait for verification. Read each line as an envelope and trust only the event with `terminal: true` as the final result. If credentials are already valid, `pcl auth login --json` returns a single normal envelope.

### Discovery Commands

```bash
pcl --llms
pcl --json --llms
pcl doctor --json
pcl whoami --json
pcl workflows --json
pcl workflows show incident-investigation --json
pcl schema list --json
pcl schema get incidents --action list_public --json
pcl api manifest --json
```

### Workflow Commands

Prefer top-level commands before raw API calls:

```bash
pcl incidents --limit 5 --json
pcl incidents --project-id <project-ref> --environment production --json
pcl incidents --project-id <project-ref> --all --limit 50 --output incidents.json --json
pcl incidents --incident-id <incident-id> --json
pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace --json
pcl projects --limit 10 --json
pcl projects --project-id <project-ref> --json
pcl projects --create --project-name demo --chain-id 1 --dry-run --json
pcl projects --project-id <project-ref> --update --field github_url=https://github.com/org/repo --dry-run --json
pcl assertions --project-id <project-ref> --json
pcl assertions --adopter-address 0x... --network 1 --json
pcl assertions --project-id <project-ref> --submitted --json
pcl account --json
pcl contracts --project <project-ref> --json
pcl releases --project <project-ref> --json
pcl deployments --project <project-ref> --json
pcl access --project <project-ref> --members --json
pcl integrations --project <project-ref> --provider slack --json
pcl protocol-manager --project <project-ref> --pending-transfer --json
pcl transfers --pending --json
pcl events --project <project-ref> --audit-log --json
pcl search --query settler --json
```

### Mutation Rules

Use `--dry-run` before writes and `--body-template` before constructing mutation payloads.
Prefer typed flags, then `--field key=value`, then `--body-file` for nested payloads.

```bash
pcl projects --body-template --json
pcl assertions --project-id <project-ref> --body-template --json
pcl releases --project <project-ref> --body-template --json
pcl access --project <project-ref> --member-user-id <user-id> --update-role --body-template --json
pcl protocol-manager --project <project-ref> --confirm-transfer --body-template --json
pcl api inspect post_projects --json
```

For complex bodies:

1. Get the template with `--body-template --json`.
2. Fill the returned body into a file.
3. Run the write with `--dry-run --body-file <file> --json`.
4. Execute without `--dry-run` only after the request plan is correct.

### Raw API Fallback

Call any endpoint below `/api/v1`. Query strings and repeated `--query` flags are both valid.

```bash
pcl api list --filter integrations --json
pcl api inspect get_views_projects_project_id_incidents --json
pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated
pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated
pcl api call get /views/projects/<project-id>/incidents --query environment=production
pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --output incidents.json
pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --jsonl --output incidents.jsonl
pcl api call get /views/projects/<project-id>/assertions
pcl api call post /web/auth/logout --body '{}'
```

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
