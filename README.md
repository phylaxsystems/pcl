# PCL — The Credible CLI

The CLI for the [Credible Layer](https://phylax.systems).

## Installation

```bash
brew install phylax/pcl/phylax
```

## Commands

| Command | Description |
|---------|-------------|
| `pcl build` | Build assertion contracts |
| `pcl apply` | Preview and apply declarative deployment changes |
| `pcl incidents`, `pcl projects`, `pcl assertions` | Natural platform workflow commands |
| `pcl account`, `pcl contracts`, `pcl releases`, `pcl deployments` | Account, contract, release, and deployment workflows |
| `pcl access`, `pcl integrations`, `pcl protocol-manager`, `pcl transfers`, `pcl events`, `pcl search` | Access control, integrations, protocol manager, transfer, audit, and search workflows |
| `pcl api` | Discover, inspect, and call raw platform API endpoints |
| `pcl auth` | Authenticate with the Credible Layer platform |
| `pcl config` | Manage CLI configuration |
| `pcl download` | Download assertion source code for a protocol |
| `pcl test` | Run assertion tests |
| `pcl verify` | Verify assertions locally before deployment |

## Agentic API Access

Top-level workflow commands expose the platform API as structured CLI operations for agents and scripts.
`pcl api` remains the raw discovery and escape-hatch surface for uncovered endpoints.
The CLI is designed around the platform workflows documented in the [Phylax docs](https://docs.phylax.systems):
projects, assertions, transparency views, deployment state, integrations, and incidents.
API commands default to compact TOON-style envelopes with `status`, `data`, and `next_actions`;
pass `--json` for the same machine-readable envelope as JSON. Successes and errors use the same shape, so agents can recover
from auth, validation, and parser failures without scraping prose diagnostics.
`pcl auth status` also reports token validity, expiry, and platform URL; expired stored tokens return
a nonzero structured error so agents do not mistake stale credentials for a working login.

```bash
# Print an agent-readable command manifest
pcl api manifest
pcl api manifest --json

# Use natural workflow commands first
pcl incidents --limit 5
pcl incidents --project-id <project-ref> --environment production
pcl incidents --project-id <project-ref> --all --limit 50 --output incidents.json
pcl incidents --incident-id <incident-id>
pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace
pcl projects --limit 10
pcl projects --project-id <project-ref>
pcl projects --create --project-name demo --chain-id 1
pcl projects --project-id <project-ref> --update --field github_url=https://github.com/org/repo
pcl projects --project-id <project-ref> --save
pcl projects --project-id <project-ref> --widget
pcl assertions --project-id <project-ref>
pcl assertions --adopter-address 0x... --network 1
pcl assertions --project-id <project-ref> --submitted
pcl assertions --project-id <project-ref> --submit --body-file submitted-assertions.json
pcl assertions --project-id <project-ref> --remove-info
pcl account
pcl account --accept-terms
pcl contracts --project <project-ref>
pcl releases --project <project-ref>
pcl deployments --project <project-ref>
pcl access --project <project-ref> --members
pcl integrations --project <project-ref> --provider slack
pcl protocol-manager --project <project-ref> --pending-transfer
pcl transfers --pending
pcl events --project <project-ref> --audit-log
pcl search --query settler

# Ask for valid mutation bodies before writing
pcl projects --body-template
pcl assertions --project-id <project-ref> --body-template
pcl releases --project <project-ref> --body-template
pcl access --project <project-ref> --member-user-id <user-id> --update-role --body-template
pcl protocol-manager --project <project-ref> --confirm-transfer --body-template
pcl api inspect post_projects

# Fall back to OpenAPI discovery for uncovered endpoints
pcl api list --filter integrations
pcl api inspect get_views_projects_project_id_incidents

# Call any endpoint below /api/v1
pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated
pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated
pcl api call get /views/projects/<project-id>/incidents --query environment=production
pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --output incidents.json
pcl api call get /views/public/incidents --paginate incidents --limit 50 --allow-unauthenticated --jsonl --output incidents.jsonl
pcl api call get /views/projects/<project-id>/assertions
pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated --output incidents.json
pcl api call post /web/auth/logout --body '{}'
```

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
