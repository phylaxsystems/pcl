# PCL Agent Instructions

This repository ships a CLI-first interface for agents. Do not rely on MCP, browser flows, or scraped help text when the CLI can provide a structured contract.

## Start Here

Run these first:

```bash
pcl --llms
pcl doctor
pcl whoami
pcl api manifest --json
```

When changing this repository, run `make ci` before handing work back. It sets `PCL_AUTH_NO_BROWSER=1` for tests so auth flows do not open a browser.

Use `--json` whenever you need stable machine parsing. Without `--json`, PCL emits compact TOON envelopes by default.

## Output Contract

Every agent-facing command should be treated as an envelope:

```json
{
  "status": "ok",
  "data": {},
  "next_actions": [],
  "schema_version": "pcl.envelope.v1",
  "pcl_version": "..."
}
```

Errors use the same shape with `status: "error"` and an `error` object. Do not parse prose diagnostics. Check `error.code`, `error.recoverable`, `error.http.status`, `error.request_id`, and `next_actions`.

Fresh `pcl auth login --json` emits JSONL progress events: first `event: auth.login_instructions`, then a terminal envelope with `terminal: true`. Treat only the terminal event as the final login result. Existing valid auth still returns a single JSON envelope.

## Discovery Order

Prefer the surfaces in this order:

1. `pcl --llms` or `pcl llms` for the current agent guide.
2. `pcl workflows` for task recipes.
3. `pcl schema list` and `pcl schema get <workflow> --action <action>` for workflow action contracts.
4. Top-level workflow commands like `pcl incidents`, `pcl projects`, `pcl assertions`, `pcl account`, `pcl releases`, and `pcl protocol-manager`.
5. `pcl api list`, `pcl api inspect`, and `pcl api call` as the raw OpenAPI escape hatch.

## Safe Execution

For mutations:

```bash
pcl <workflow> --body-template
pcl <workflow> --dry-run ...
pcl <workflow> --body-file body.json
```

Use typed flags first. Use `--field key=value` for simple payload fields. Use `--body-file` for nested payloads. Avoid constructing opaque inline JSON unless the command has no typed surface yet.

## Raw API Calls

Both query forms are valid:

```bash
pcl api call get '/views/public/incidents?limit=5' --allow-unauthenticated --json
pcl api call get /views/public/incidents --query limit=5 --allow-unauthenticated --json
```

Use `pcl api inspect <operation-id> --json` before calling unfamiliar endpoints. For required request bodies, inspect the operation and prefer `--body-file`.

## Long Jobs And Artifacts

For investigations, prefer JSONL exports and local job records:

```bash
pcl export incidents --project-id <project-id> --environment production --out incidents.jsonl --errors errors.jsonl --checkpoint checkpoint.json --resume --continue-on-error --json
pcl jobs list --json
pcl jobs status <job-id> --json
pcl jobs resume <job-id> --json
pcl artifacts list --json
```

Export commands record `job_id`, `resume_command`, checkpoint path, output path, and error path. Use those fields instead of rebuilding pagination state manually.

## Auth And Public Endpoints

Use:

```bash
pcl auth status --json
pcl whoami --json
```

Do not treat a stored token as valid unless `token_valid` is true and `expired` is false. Public endpoints should be called with `--allow-unauthenticated` when using raw `pcl api call`.

## Provenance

When reporting results, preserve:

- `request_id` from API errors or response metadata.
- Incident IDs, transaction hashes, trace IDs, project IDs, and artifact paths.
- The exact command used, especially for exports and mutations.

Use `pcl requests list --json` to recover recent request metadata.

## Shell Completions

Generate completions with:

```bash
pcl completions bash
pcl completions zsh
pcl completions fish
```

Under `--json`, completions return the script inside the normal envelope.
