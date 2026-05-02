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
| `pcl api` | Discover, inspect, and call platform API endpoints |
| `pcl auth` | Authenticate with the Credible Layer platform |
| `pcl config` | Manage CLI configuration |
| `pcl download` | Download assertion source code for a protocol |
| `pcl test` | Run assertion tests |
| `pcl verify` | Verify assertions locally before deployment |

## Agentic API Access

`pcl api` exposes the platform API as structured CLI operations for agents and scripts.
It is designed around the platform workflows documented in the [Phylax docs](https://docs.phylax.systems):
projects, assertions, transparency views, deployment state, integrations, and incidents.
API commands default to compact TOON-style output with `next_actions`; pass `--json` for the full
machine-readable envelope. Successes and errors use the same envelope shape, so agents can recover
from auth, validation, and parser failures without scraping prose diagnostics.
`pcl auth status` also reports token validity, expiry, and platform URL; expired stored tokens return
a nonzero structured error so agents do not mistake stale credentials for a working login.

```bash
# Print an agent-readable command manifest
pcl api manifest
pcl api manifest --json

# Use natural workflow commands first
pcl api incidents --limit 5
pcl api incidents --project-id <project-ref> --environment production
pcl api incidents --project-id <project-ref> --all --limit 50 --output incidents.json
pcl api incidents --incident-id <incident-id>
pcl api incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace
pcl api projects --limit 10
pcl api projects --project-id <project-ref>
pcl api projects --create --project-name demo --chain-id 1
pcl api projects --project-id <project-ref> --update --field github_url=https://github.com/org/repo
pcl api projects --project-id <project-ref> --save
pcl api projects --project-id <project-ref> --widget
pcl api assertions --project-id <project-ref>
pcl api assertions --adopter-address 0x... --network 1
pcl api assertions --project-id <project-ref> --submitted
pcl api assertions --project-id <project-ref> --submit --body-file submitted-assertions.json
pcl api assertions --project-id <project-ref> --remove-info
pcl api account
pcl api account --accept-terms
pcl api contracts --project <project-ref>
pcl api releases --project <project-ref>
pcl api deployments --project <project-ref>
pcl api access --project <project-ref> --members
pcl api integrations --project <project-ref> --provider slack
pcl api protocol-manager --project <project-ref> --pending-transfer
pcl api transfers --pending
pcl api events --project <project-ref> --audit-log
pcl api search --query settler

# Ask for valid mutation bodies before writing
pcl api projects --body-template
pcl api assertions --project-id <project-ref> --body-template
pcl api releases --project <project-ref> --body-template
pcl api access --project <project-ref> --member-user-id <user-id> --update-role --body-template
pcl api protocol-manager --project <project-ref> --confirm-transfer --body-template
pcl api inspect post_projects

# Fall back to OpenAPI discovery for uncovered endpoints
pcl api list --filter integrations
pcl api inspect get_views_projects_project_id_incidents

# Call any endpoint below /api/v1
pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated
pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated
pcl api call get /views/projects/<project-id>/incidents --query environment=production
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
