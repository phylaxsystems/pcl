# Changelog

All notable user-facing changes should be recorded here.

## Unreleased

### Fixed

- Broadcasting no longer fails on a brief `assertions are unavailable` refusal. The transaction is signed once and resubmitted byte for byte, so a retry is deduplicated by hash rather than becoming a second transaction on the same nonce. `--with-credible-rpc` waits out a Credible RPC's full alignment window; without it an unavailable endpoint still gets a short retry. If the retries run out, `onchain.assertions_unavailable` means nothing was submitted, while `onchain.tx_submission_unconfirmed` carries the signed hash to check first.

### Added

- `pcl deploy` warns when the assertions it is about to release use the V2 spec but the target does not support it (the `app.phylax.systems` platform, or a Linea chain). The warning names the files and the V2 triggers/precompiles found in them, prints before the protocol-manager step and again at the end, and appears in `--json` output as `data.warnings`. It never blocks a deploy.
- `pcl auth login` warns when logging in to `app.phylax.systems` that the platform runs the V1 assertion spec and assertions must not be written against V2. Machine output carries it as a `warnings` array on the login envelope.

### Breaking changes

- Removed TOON output entirely: the `--toon` flag, the `--format toon` alias, and the TOON envelope renderer are gone. `--json` is the only machine output mode; agent guidance, manifest examples, and `next_actions` hints now use `--json`.

## 1.5.0 - 2026-05-29

### Breaking changes

- Removed the top-level `pcl transfers` command. Use the per-project workflow commands instead.
- Removed the hidden legacy `--format json` / `--format toon` parser aliases. Use `--json` or `--toon` directly.

### Added

- Routed projects, releases, incidents, and outer workflows through generated OpenAPI operations instead of hand-built request paths, so workflow definitions stay tied to operation IDs.
- Split per-surface workflow modules (incidents, projects, releases, access, integrations, protocol-manager, events, search, and related surfaces) so adding a new API-backed workflow now has an obvious file and pattern.
- Added a build-time OpenAPI spec transform that prunes the generated client surface before progenitor runs.

### Changed

- Improved default human-mode CLI output without changing the `--toon` / `--json` machine contracts; help, parse, auth, workflow, and destructive-command output now stay mode-aware.
- Split the raw `pcl api` command internals by concern (list, inspect, call, coverage, manifest) and refactored CLI output contracts and workflow subcommands on top of the human-output work.
- Refactored workflow/action metadata into clearer contract definitions, keeping schema and manifest output aligned with top-level workflow commands.
- Preserved structured `ErrorResponse` / `UnexpectedResponse` payloads from the generated client in `apply` preview, release, and download paths instead of stringifying errors.

### Removed

- Removed stale raw API aliases, the old manual workflow request constructors, and workflow surface that no longer belongs in the product path.

## 1.4.4 - 2026-05-12

- Fixed expired-auth recovery guidance so human output recommends `pcl auth refresh` before forcing a new login, while TOON/JSON next actions keep their explicit output modes.
- Moved shared envelope rendering into a top-level output module and made `apply`, `download`, and `verify` honor `--toon`/`--json` envelopes consistently.
- Added preferred subcommand forms for the highest-traffic workflow groups: `pcl projects ...`, `pcl access ...`, and `pcl releases ...`, while keeping legacy flag forms working.
- Kept root help clap-native while ordering common workflow commands first, and documented `pcl build`/`pcl test` as human pass-through developer commands in machine modes.

## 1.4.3 - 2026-05-10

- Made default CLI output human-first across command surfaces, including auth, config, workflow, schema, API discovery, dry-run, export, job, artifact, request log, collection, incident, and raw API response views.
- Added `--toon` as the compact agent-readable output mode while preserving `--json` and hidden legacy `--format` compatibility.
- Updated agent-facing docs and smoke checks to use `--toon`.

## 1.4.2 - 2026-05-09

- Updated Credible SDK assertion dependencies to the latest 1.4 assertion runtime graph.
- Fixed CI private dependency handling for the reusable Rust workflow and documented the remaining upstream-pinned advisory exceptions.

## 1.4.1 - 2026-05-08

- Added `pcl apply --dry-run` to build and verify release payloads from `credible.toml` without calling the API.
- Fixed full-feature release builds for local assertion verification.
- Reduced dependency advisory exposure by replacing the `vergen-gix` build dependency and refreshing patched transitive dependencies.

## 1.4.0 - 2026-05-06

- Added agent-oriented CLI discovery, schemas, JSON/TOON envelopes, resumable jobs, and artifact surfaces.
- Added first-class platform workflow commands for common project, assertion, incident, account, release, deployment, access, integration, protocol-manager, transfer, event, and search operations.
- Added formatted generated dApp API client output for reviewable OpenAPI regeneration diffs.
- Added repository-local CI, advisory audit, contributor, and security documentation.
