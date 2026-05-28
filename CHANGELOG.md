# Changelog

All notable user-facing changes should be recorded here.

## Unreleased

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
