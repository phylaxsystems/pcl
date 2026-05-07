# PCL Agent Instructions

This repository ships a CLI-first interface for agents. Do not rely on MCP, browser flows, or scraped help text when the CLI can provide a structured contract.

## Start Here

Run these first:

```bash
pcl --llms
pcl doctor
pcl auth ensure --format toon
pcl whoami
pcl api manifest --format toon
```

When changing this repository, run `make ci` before handing work back. It sets `PCL_AUTH_NO_BROWSER=1` for tests so auth flows do not open a browser, and it runs `make agent-smoke` to verify the documented agent discovery path.

Use TOON as the normal machine interface; it is the default compact envelope and is cheaper for agents to consume. Examples in this file prefer default TOON or `--format toon` when the contract should be explicit. Use `--json` or `--format json` only when a downstream tool needs strict JSON.

## Output Contract

Every agent-facing command should be treated as an envelope. In default TOON this is shaped like:

```toon
status: ok
data: {}
next_actions: []
schema_version: pcl.envelope.v1
pcl_version: "..."
```

Errors use the same shape with `status: "error"` and an `error` object. Do not parse prose diagnostics. Check `error.code`, `error.recoverable`, `error.http.status`, `error.request_id`, and `next_actions`.

Output mode rules:

- default: TOON envelope
- explicit TOON: `--format toon`
- JSON: `--json` or `--format json`
- JSONL exception: fresh `pcl auth login --json` streams events and marks the final event with `terminal: true`

Fresh `pcl auth login --json` emits JSONL progress events: first `event: auth.login_instructions`, then a terminal envelope with `terminal: true`. Treat only the terminal event as the final login result. Existing valid auth still returns a single JSON envelope.

## Discovery Order

Prefer the surfaces in this order:

1. `pcl --llms` or `pcl llms` for the current agent guide.
2. `pcl workflows` for task recipes.
3. `pcl schema list` and `pcl schema get <workflow> --action <action>` for workflow action contracts.
4. Top-level workflow commands like `pcl incidents`, `pcl projects`, `pcl assertions`, `pcl account`, `pcl releases`, and `pcl protocol-manager`.
5. `pcl api list`, `pcl api inspect`, `pcl api call`, and `pcl api coverage` only for debugging, API parity checks, internal/service endpoints, or endpoints without `workflow_alternatives`.

## Safe Execution

For mutations:

```bash
pcl <workflow> --body-template
pcl <workflow> --dry-run ...
pcl <workflow> --body-file body.json
```

Use typed flags first. Use `--field key=value` for simple payload fields. Use `--body-file` for nested payloads. Avoid constructing opaque inline JSON unless the command has no typed surface yet.

## Raw API Calls

Raw calls are not the normal product path. Use them for debugging, API parity checks, service/internal endpoint investigation, browser-session bridge investigation, or new endpoint exploration before promotion to a workflow. If `pcl api list` or `pcl api inspect` returns `workflow_alternatives`, use the advertised workflow command instead of `pcl api call`.

Both query forms are valid:

```bash
pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated --format toon
pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated --format toon
```

For simple raw request bodies, `pcl api call` accepts repeated `--field key=value` and merges those fields into a JSON object, matching workflow command behavior. Use `--body-file` for nested payloads.

Use `pcl api inspect <operation-id> --format toon` before calling unfamiliar endpoints. Inspect includes `workflow_alternatives`, `raw_api_use`, auth metadata, and required header placeholders; preserve required `--header` values in generated examples. For required request bodies, inspect the operation and prefer `--body-file`.

Raw API calls persist `operation_id` in request history when the live OpenAPI manifest can resolve the method/path. After exploratory testing, run:

```bash
pcl api coverage --format toon
pcl api coverage --markdown api-coverage.md --format toon
```

Use `no_hit`, `no_2xx`, `write_no_2xx`, and `unmatched_records` to decide what still needs manual reconciliation.

## Long Jobs And Artifacts

For investigations, prefer JSONL exports and local job records:

```bash
pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --checkpoint checkpoint.json --resume --continue-on-error --format toon
pcl jobs list --format toon
pcl jobs status <job-id> --format toon
pcl jobs resume <job-id> --format toon
pcl artifacts list --format toon
```

Export commands record `job_id`, `resume_command`, checkpoint path, output path, and error path. Use those fields instead of rebuilding pagination state manually.

## Auth And Public Endpoints

Use:

```bash
pcl auth status --format toon
pcl auth ensure --format toon
pcl whoami --format toon
```

Do not treat a stored token as valid unless `token_valid` is true and `expired` is false. `pcl doctor --format toon` also checks whether the target API advertises CLI login, refresh, and remote logout/revocation endpoints. Public endpoints should be called with `--allow-unauthenticated` when using raw `pcl api call`.

Use `pcl auth ensure --format toon` before long workflows. It returns `status: ok` when auth is usable, or one `status: action_required` envelope with `device_url`, `code`, `device_secret`, and `poll_command` when user login is needed. Run `poll_command` until it returns `status: ok` or `status: error`.

`expires_soon: true` means the access token has five minutes or less remaining. `pcl auth refresh --format toon` is safe to call and rotates the stored CLI refresh token when available; if the refresh token is missing or rejected, it returns the same login challenge shape. `pcl auth login --no-wait --format toon` also returns a single challenge envelope. Use `pcl auth login --json` only when you specifically want the JSONL streaming login contract. `pcl auth logout` attempts remote logout first, then clears local credentials; use `pcl auth logout --local` only when you explicitly want local cleanup.

Auth commands use `--auth-url`/`PCL_AUTH_URL` when set, otherwise they follow `PCL_API_URL` before falling back to the production app URL.

## Provenance

When reporting results, preserve:

- `request_id` from API errors or response metadata.
- Incident IDs, transaction hashes, trace IDs, project IDs, and artifact paths.
- The exact command used, especially for exports and mutations.

Use `pcl requests list --format toon` to recover recent request metadata.

## Shell Completions

Generate completions with:

```bash
pcl completions bash
pcl completions zsh
pcl completions fish
```

Use `--format json` for completions only when a downstream installer expects the script inside a JSON envelope.
