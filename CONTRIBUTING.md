# Contributing

This repository is a Rust workspace for the PCL CLI, its core library, the Phoundry integration layer, and the generated dApp API client.

## Local Checks

Run the full local gate before opening or updating a PR:

```bash
make ci
```

That runs formatting, strict clippy, all workspace tests with browser auth disabled, docs, and whitespace checks. Use the narrower targets while iterating:

```bash
make fmt-check
make clippy
make test
make doc
make audit
```

Auth-related tests should run with `PCL_AUTH_NO_BROWSER=1`; `make test` already sets it.

## CLI Contract

PCL is a CLI-first interface for humans and agents. Preserve these compatibility rules:

- Default command output is structured TOON.
- `--json` output is a stable envelope with `status`, `data` or `error`, `next_actions`, `schema_version`, and `pcl_version`.
- Parser, auth, config, validation, network, and API errors should use the same envelope shape as successful commands.
- Do not remove or rename existing JSON fields without a compatibility plan.
- Prefer typed flags and `--body-template` over opaque JSON-only workflow surfaces.

## Generated Code

`crates/dapp-api-client/src/generated/client.rs` is generated from `crates/dapp-api-client/openapi/spec.json`.

Regenerate it with:

```bash
make regenerate
```

For local development API regeneration:

```bash
make regenerate-dev
```

Generated code should stay formatted and marked with `@generated`. Review generated diffs for API-surface changes, but make manual fixes in the generator or OpenAPI transform layer, not in the generated file.

## Review Expectations

For large CLI/API changes, include tests for:

- JSON and default TOON output.
- Error envelopes and nonzero exits.
- Dry-run behavior for mutations.
- Request builders for required path, query, and body fields.
- Manifest/schema output consumed by agents.

Keep unrelated refactors out of feature PRs unless they directly reduce risk for the change.
