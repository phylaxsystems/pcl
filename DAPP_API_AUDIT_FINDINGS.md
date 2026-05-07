# Credible Layer dApp API Audit Findings

Live notes for the local end-to-end audit using this branch of `pcl` against
`../credible-layer-dapp`.

## Scope

- `pcl` checkout: `/Users/odysseas/code/phylax/pcl`
- dApp checkout: `/Users/odysseas/code/phylax/credible-layer-dapp`
- Local dApp base URL: `http://localhost:3000`
- Isolated local PCL config: `/tmp/pcl-local-dapp-audit`
- Audit artifacts: `/tmp/pcl-api-audit`
- Seeded project under test: `123e4567-e89b-12d3-a456-426614174000`
- Seeded auth user under test: `a0000000-0000-0000-0000-000000000001`

## Setup Evidence

- Ran required PCL orientation commands with the branch binary:
  - `target/debug/pcl --llms`
  - `target/debug/pcl doctor`
  - `target/debug/pcl auth ensure --json`
  - `target/debug/pcl whoami --json`
  - `target/debug/pcl api manifest --json`
- Confirmed installed `pcl` is older (`1.3.0`) than the branch binary
  (`1.4.0`), so all audit commands use `target/debug/pcl`.
- Started dApp services using repo instructions:
  - `pnpm local:full`
  - `pnpm dev:app`
- Confirmed local API health through PCL:
  - `PCL_API_URL=http://localhost:3000 target/debug/pcl --config-dir /tmp/pcl-local-dapp-audit doctor --json`
  - health request id: `req_mourp20l_11d414aba91774dc`
- Confirmed local OpenAPI surface:
  - `/api/v1/openapi` returned 88 paths and 103 operations.
  - `pcl api list --json` returned 103 operations.
  - `pcl api inspect <operation_id> --json` succeeded for all 103 operations.
  - Method coverage from OpenAPI: `GET=59`, `POST=34`, `DELETE=8`, `PATCH=1`, `PUT=1`.

## Bugs And API Issues

This is the condensed issue list from the audit. It is root-caused against the
dApp source in `../credible-layer-dapp`, so it separates dApp/API issues from
CLI-only issues. The sections below keep the raw command evidence and request
IDs.

### Verified dApp / API Issues

1. **Default production auth refresh route still returns 404.**
   - Classification: deployed dApp/API issue, not a CLI path-construction bug.
   - Audit command: `target/debug/pcl auth ensure --json`
   - Observed during audit: `auth.request_failed`, message
     `Refresh endpoint returned HTTP 404: Not Found`.
   - Rechecked directly on 2026-05-07:
     `POST https://app.phylax.systems/api/v1/auth/refresh` returned 404 with
     `{"message":"Not Found"}`.
   - Source evidence: current dApp source defines `POST /auth/refresh` in
     `packages/dapp-api/src/v1/contracts/auth/refresh.ts:21-36`, wires
     `createAuthRefreshRouter` under `auth` in
     `packages/dapp-api/src/v1/routers/index.ts:30-35`, and implements refresh
     in `packages/dapp-api/src/v1/routers/auth/refresh.ts:76-228`.
   - Root cause: the CLI is calling the route implemented by the current dApp
     source (`/api/v1/auth/refresh` after the v1 prefix), but the default
     deployed API does not expose that route. This points to deployment/version
     drift or a missing production route registration, not a malformed CLI
     request.
   - Impact: agents with expired stored credentials cannot recover through the
     documented default refresh path.
   - PCL mitigation added: a refresh 404 is treated as a rejected refreshable
     session and falls back to the login-challenge flow.

2. **Submitted-assertions endpoints are removed at runtime but still exposed in
   the dApp API contract.**
   - Classification: dApp API contract/runtime inconsistency, not a CLI bug.
   - Audit evidence:
     `GET /projects/{project_id}/submitted-assertions` returned 410,
     request id `req_mourvrtc_98407bd90d5aea4c`; `POST` returned 410, request
     id `req_mousb5ye_1c231e6fae0d5756`.
   - Source evidence: the contract still declares both operations in
     `packages/dapp-api/src/v1/contracts/projects/submitted-assertions.ts:17-73`
     and includes them in `projectsContract` via
     `packages/dapp-api/src/v1/contracts/projects/index.ts:23-28`.
   - Runtime evidence: the router intentionally returns 410 stubs in
     `packages/dapp-api/src/v1/routers/projects/index.ts:538-546`; the dApp
     tests also state that the `submitted-assertions` feature has been removed
     in `apps/dapp/tests/integration/assertion-lifecycle.test.ts:98-100`.
   - Root cause: feature removal was implemented in the router, but the
     contract/discovery surface was left in place as if the endpoints were still
     normal API actions.
   - Impact: OpenAPI/PCL discovery makes agents think assertion submission is
     available, then the API rejects the call as removed.

3. **Incident detail/trace auth metadata does not match runtime behavior.**
   - Classification: dApp API contract/runtime metadata issue. The PCL fix is a
     workaround, not the root cause.
   - Audit evidence: `GET /views/incidents/{incidentId}` and
     `GET /views/incidents/{incidentId}/transactions/{txId}/trace` were exposed
     through discovery as public-style view calls, but returned 401 without a
     bearer token.
   - Source evidence: the contracts only add OpenAPI tags and response codes:
     `packages/dapp-api/src/v1/contracts/views/incident-detail.ts:84-105` and
     `packages/dapp-api/src/v1/contracts/views/incident-trace.ts:68-90`.
   - Runtime evidence: the routers attach `createProjectMemberAuthMiddleware`
     and return 401/403 when auth or membership is missing:
     `packages/dapp-api/src/v1/routers/views/incident-detail.ts:56-109`,
     `packages/dapp-api/src/v1/routers/views/incident-trace.ts:69-122`, and
     `packages/dapp-api/src/v1/middleware/incidents-auth.ts:176-320`.
   - Root cause: the auth requirement lives only in router middleware. It is
     not represented clearly enough in the contract/OpenAPI metadata that PCL
     consumes.
   - Impact: agents following the generated manifest/OpenAPI examples omit auth
     and get 401s on valid incidents.
   - PCL mitigation added: incident detail/trace workflow calls now require and
     attach stored auth. Local verification passed with request ids
     `req_mousmyhd_cceb08387bfa4b3a` and `req_mousmygz_972e49dccf4fb513`.

4. **Incident trace `txId` is a row UUID, not a chain transaction hash.**
   - Classification: dApp API naming/schema issue, not a CLI bug.
   - Audit evidence: using transaction hash
     `0x32a5993654370879a11a1d9d5dd0eb4112670b40d100fa1a75c7e75270ad17ae`
     returned `400 Invalid uuid`; using invalidating transaction UUID
     `6534fc5b-4145-4286-a69b-97c8e3fd6c93` returned 200.
   - Source evidence: the contract names the path param `txId` but validates it
     as `UUIDSchema` in
     `packages/dapp-api/src/v1/contracts/views/incident-trace.ts:68-78`.
     The trace service documents `txId` as the invalidating transaction UUID and
     queries `invalidating_transactions.id` / `debug_traces.invalidating_transaction_id`
     in `packages/dapp-api/src/v1/services/incidents/trace.ts:20-43` and
     `packages/dapp-api/src/v1/services/incidents/trace.ts:75-93`.
   - Root cause: the endpoint path and parameter name read like a chain
     transaction identifier, but the implementation requires the internal
     invalidating-transaction row UUID.
   - Impact: agents and humans naturally try a chain transaction hash and get a
     validation error. The API should rename/docs the param as
     `invalidating_transaction_id`, or accept transaction hashes and resolve
     them server-side.

### Verified CLI Issues / Not dApp API Bugs

1. **`PCL_API_URL` did not drive auth device URLs.**
   - Classification: PCL CLI/config ergonomics issue.
   - Evidence: API commands used `PCL_API_URL=http://localhost:3000`, but
     `auth login --no-wait --force --json` generated production
     `device_url`/`poll_command` until `auth --auth-url http://localhost:3000`
     was passed explicitly.
   - Root cause: API base URL and auth base URL are separate CLI settings.
   - dApp source is not implicated; local dApp auth worked when the auth URL was
     explicit.
   - PCL fix applied: auth commands now use `--auth-url`/`PCL_AUTH_URL` when
     set, otherwise fall back to `PCL_API_URL` before the production app URL.
     Verified by `auth_login_no_wait_uses_pcl_api_url_when_auth_url_is_unset`.

2. **Adopter remove-calldata initially sent a malformed request.**
   - Classification: PCL workflow issue, not a dApp API bug.
   - API source evidence: the dApp contract explicitly requires
     `query.assertion_ids` in
     `packages/dapp-api/src/v1/contracts/assertions/adopters.ts:106-123`, and
     the router returns 400 when the normalized list is empty in
     `packages/dapp-api/src/v1/routers/assertions/adopters.ts:449-470`.
   - Root cause: the original PCL workflow did not expose/supply required
     `assertion_ids` and did not expose useful `network`/`environment` filters.
   - PCL fix applied: `contracts --remove-calldata` now requires
     `--assertion-id`, supports repeated assertion IDs, and exposes
     `--network`/`--environment`. Local verification passed with request id
     `req_mousmyhd_c08a70cff5569fc2`.

3. **Dry-run placement in the agent manifest was CLI documentation only.**
   - Classification: PCL manifest issue.
   - Root cause: the manifest showed the wrong placement for `--dry-run`.
   - PCL fix applied: the manifest now shows
     `pcl projects --dry-run --create ...`.

### Not API Bugs

- `projects_widget` and `contracts_unassigned` initially returned 500 because
  Ponder was not alive/indexed after `pnpm local:full` exited. Both passed after
  running Anvil/Inngest/Ponder persistently and reseeding on-chain data.
- active-release deploy calldata returning 409 is expected; inactive release
  calldata passed.
- protocol-manager transfer/accept calldata returning 400 is expected for the
  current seeded data because active contracts do not share one StateOracle.
- malformed input tests returned structured envelopes as expected:
  `input.invalid_json`, `input.body_file_read_failed`,
  `openapi.operation_not_found`, `api.bad_request`, and `api.not_found`.

## Confirmed Findings

### 1. Production/default auth refresh currently fails with HTTP 404

Command:

```bash
target/debug/pcl auth ensure --json
```

Observed result:

- Stored token was expired.
- `auth ensure` attempted refresh.
- Refresh failed with:
  - `status: error`
  - `error.code: auth.request_failed`
  - message: `Authentication failed: Refresh endpoint returned HTTP 404: Not Found`
  - next action only: `pcl auth login`

Why this matters:

- The branch docs say `pcl auth refresh --json` and `pcl auth ensure --json`
  should rotate the stored CLI refresh token when available, or return a login
  challenge when refresh is missing/rejected.
- A 404 from `/api/v1/auth/refresh` is neither a successful refresh nor a
  structured invalid-refresh/login challenge.

Source-root-caused classification:

- Current dApp source defines and wires the route, but the default deployed API
  still returns 404 for `POST /api/v1/auth/refresh` as of 2026-05-07.
- This is a deployed dApp/API route availability issue, not a CLI
  path-construction issue.

Status: confirmed dApp deployment/API-version issue. PCL mitigation was added,
but the production/default route still needs a dApp-side fix.

### 2. `PCL_API_URL` does not drive auth device URLs

Command:

```bash
PCL_API_URL=http://localhost:3000 target/debug/pcl --config-dir /tmp/pcl-local-dapp-audit auth login --no-wait --force --json
```

Observed result:

- The generated `device_url` and `poll_command` pointed at
  `https://app.phylax.systems`, not `http://localhost:3000`.

Working command:

```bash
PCL_API_URL=http://localhost:3000 target/debug/pcl --config-dir /tmp/pcl-local-dapp-audit auth --auth-url http://localhost:3000 login --no-wait --force --json
```

Why this matters:

- API commands use `PCL_API_URL`.
- Auth commands now use `--auth-url`/`PCL_AUTH_URL` when set, otherwise they
  follow `PCL_API_URL`.
- For local end-to-end dApp testing, this prevents accidentally creating auth
  sessions on production when the API URL is pointed at the local dApp.

Status: fixed in PCL.

### 3. Local CLI auth and refresh path works when the auth URL is explicit

Commands:

```bash
PCL_API_URL=http://localhost:3000 target/debug/pcl --config-dir /tmp/pcl-local-dapp-audit auth --auth-url http://localhost:3000 login --no-wait --force --json
```

Then the generated local CLI auth session was marked verified for seeded user
`a0000000-0000-0000-0000-000000000001`, and:

```bash
PCL_API_URL=http://localhost:3000 target/debug/pcl --config-dir /tmp/pcl-local-dapp-audit auth --auth-url http://localhost:3000 poll --session-id <session_id> --device-secret <device_secret> --expires-at <expires_at> --json
PCL_API_URL=http://localhost:3000 target/debug/pcl --config-dir /tmp/pcl-local-dapp-audit auth --auth-url http://localhost:3000 refresh --force --json
```

Observed result:

- Poll completed with `status: ok`, `event: auth.login_complete`, and a valid
  app token.
- Refresh completed with:
  - `status: ok`
  - `data.refreshed: true`
  - request id: `req_mourq5ex_c172cebead59ca0c`
  - `refresh_expires_at: 2026-06-06T00:49:01.172152+00:00`

Status: verified working locally.

## Running Verification Log

- OpenAPI discovery and inspection: complete, 103/103 operation inspections
  succeeded.
- Local services: full local stack and dApp server are running.
- Auth: local login/poll/refresh verified with isolated config.
- Workflow/read-path audit: complete and classified.
  - 50 seeded workflow read commands executed through `pcl`.
  - 41 returned `status: ok`.
  - 9 returned structured error envelopes; API/CLI classifications are in
    `Bugs And API Issues`.
- Mutation dry-run and malformed-input audit: complete.
- PCL fixes: complete; focused tests and `make ci` passed.

## Read Sweep Results

Command summary artifact:

- `/tmp/pcl-api-audit/read-summary.txt`
- `/tmp/pcl-api-audit/read-commands.txt`
- individual responses under `/tmp/pcl-api-audit/calls/read/`

Successful seeded read surfaces:

- account: `pcl account`
- projects: explorer, home, resolve, detail by UUID, detail by slug, saved, list
- assertions: project list, production filter, detail, adopter lookup,
  registered, remove info, remove calldata
- incidents: public list, project production/staging lists, stats
- contracts: all, project list, detail
- releases: list, detail, remove calldata
- deployments: project deployments
- access: members, my role, invitations, pending invitations
- integrations: Slack get, PagerDuty get
- protocol manager: pending transfer, nonce
- transfers: pending transfers
- events: project events, audit log
- search/misc: search, stats, system status, health, whitelist,
  verified-contract lookup

Read sweep errors under classification:

- `projects_widget`: `500`, request id
  `req_mourvrlc_f431c9b24349d66d`, body
  `{"error":"Failed to fetch project widget data"}`.
- `assertions_submitted`: `410`, request id
  `req_mourvrtc_98407bd90d5aea4c`, body
  `{"error":"This feature has been removed"}`.
- `incidents_detail`: `401`, request id
  `req_mourvs6t_7b8bfa0a1d476e8b`, body `{"error":"Unauthorized"}`.
- `incidents_trace`: `401`, request id
  `req_mourvs7t_6d8489d17a4bfdc4`, body `{"error":"Unauthorized"}`.
- `contracts_unassigned`: `500`, request id
  `req_mourvsdt_09e352b36dc9742e`, body
  `{"error":"Failed to fetch no-project assertion adopters"}`.
- `contracts_remove_calldata`: `400`, request id
  `req_mourvsev_d877f2760481985c`, validation body says missing
  `assertion_ids`.
- `releases_deploy_calldata`: `409`, request id
  `req_mourvsjz_d848fbe399ed2cf7`, body
  `{"error":"Only inactive releases can be deployed"}`.
- `protocol_transfer_calldata`: `400`, request id
  `req_mourvt2l_f66c9c8d7e3eb6e2`, body says active contracts do not share a
  single StateOracle.
- `protocol_accept_calldata`: `400`, request id
  `req_mourvt3w_9dc6da86d07f9c8e`, body says active contracts do not share a
  single StateOracle.

Environment caveat:

- After `pnpm local:full` exited, `local:doctor` only reported Supabase ports
  and the Next dev server. Anvil, Inngest, and Ponder were no longer listening
  even though `local:full` had printed them as ready.
- Because `projects_widget` and `contracts_unassigned` depend on Ponder GraphQL,
  their 500s need a retry with Ponder kept alive in a persistent session before
  they are classified as API defects.

Retry after persistent Anvil/Inngest/Ponder and `scripts/seed-onchain-test-data.sh`:

- `projects_widget`: passed with `status: ok`; the earlier 500 was caused by
  the local Ponder dependency not being alive/indexed.
- `contracts_unassigned`: passed with `status: ok`, empty data, request id
  `req_mous5jmy_910441c1b37bdbfc`; the earlier 500 was caused by the local
  Ponder dependency not being alive/indexed.
- `releases_deploy_calldata`: the active release still correctly returns 409;
  retrying inactive release `ba000000-0000-0000-0000-000000000053` passed with
  `status: ok`.
- `incidents_detail`: still failed 401 with a valid local CLI token, request id
  `req_mous6yqr_0cde95fbc6bca613`.
- `incidents_trace`: still failed 401 with a valid local CLI token, request id
  `req_mous6yqx_0ae18fd807ba7990`.
- `contracts_remove_calldata`: still failed 400, request id
  `req_mous6yu3_449a306907053332`; API validation requires
  `query.assertion_ids`, but the workflow command does not expose an
  assertion-id flag for this path.
- `protocol_transfer_calldata`: still failed 400, request id
  `req_mous6yu9_5d1808a46047d84c`; seeded active contracts do not share a
  single StateOracle.
- `protocol_accept_calldata`: still failed 400, request id
  `req_mous72ba_6d662dd90750fa71`; seeded active contracts do not share a
  single StateOracle.

## Mutation And Edge-Case Sweep

Mutation dry-run/body-template artifact:

- `/tmp/pcl-api-audit/calls/mutation/body-template-dry-run-summary.tsv`

Dry-run and body-template coverage passed for:

- project create/update/save/unsave/delete
- assertion submit planning
- contract create/assign/remove
- release preview/deploy
- deployment confirmation
- access invite/update-role/remove
- Slack integration configure/test/delete
- protocol-manager set/clear/confirm-transfer
- transfer reject

Actual local mutation cycle passed:

- Created disposable project `b732c8f8-9739-4766-8f3a-a2b356c78a59`, request
  id `req_mous9yo0_ebe577d1670ee91c`.
- Updated, fetched, saved, unsaved, deleted it, then confirmed detail returns
  404 after delete.
- Created and revoked a seeded-project invitation:
  - invite request id `req_mousagvl_910db1ccdce16a52`
  - revoke request id `req_mousaoaw_d95edae651702963`
- Configured, read, deleted, and re-read Slack integration settings:
  - configure request id `req_mousb01c_37ebe0a28f5ad229`
  - delete request id `req_mousb05o_4437ca1cb93db715`

Removed feature behavior:

- `pcl assertions --project 123e4567-e89b-12d3-a456-426614174000 --submit --body '{"assertions":[]}' --json`
  returned 410, request id `req_mousb5ye_1c231e6fae0d5756`, body
  `{"error":"This feature has been removed"}`.
- This confirms both GET and POST submitted-assertions paths are removed while
  the PCL workflow manifest still presents them as normal actions.

Malformed/auth edge artifact:

- `/tmp/pcl-api-audit/calls/edge/summary.tsv`

Edge-case results:

- no stored auth on `projects --home`: `auth.no_token`
- public incidents with no auth and `--allow-unauthenticated`: `status: ok`,
  request id `req_mousniyr_8b8151a61e531c15`
- protected endpoint with `--allow-unauthenticated`: `auth.unauthorized`, 401,
  request id `req_mousnj0v_e1c1cb2627a703d4`
- malformed JSON body in dry-run: `input.invalid_json`
- project create with `{}` body: `api.bad_request`, 400, request id
  `req_mousnj2p_f2180ee99bb2ba49`
- missing body file: `input.body_file_read_failed`
- raw public incidents with invalid `limit`: `api.bad_request`, 400, request id
  `req_mousnj4s_d9ee42b05aa47bf6`
- unknown raw path: `api.not_found`, 404, request id
  `req_mousnj60_310573fc7b1d76ce`
- unknown `api inspect`: `openapi.operation_not_found`

## Fixes Applied In PCL

- Incident detail and trace workflow calls now require auth and attach the
  stored CLI token. Verified against local dApp:
  - detail passed 200, request id `req_mousmyhd_cceb08387bfa4b3a`
  - trace passed 200 using invalidating transaction UUID
    `6534fc5b-4145-4286-a69b-97c8e3fd6c93`, request id
    `req_mousmygz_972e49dccf4fb513`
- `contracts --remove-calldata` now requires `--assertion-id`, supports
  repeated assertion IDs, and supports `--network`/`--environment` query
  inputs. Verified against local dApp:
  - fixed command passed 200, request id `req_mousmyhd_c08a70cff5569fc2`
  - missing `--assertion-id` now fails locally as
    `workflow.invalid_arguments` instead of sending malformed API requests.
- `auth refresh`/`auth ensure` now treat refresh endpoint 404 as a rejected
  refreshable session, clearing stale credentials and returning the normal
  login-challenge path instead of a hard `auth.request_failed` error.
- Agent manifest dry-run copy now shows the correct top-level workflow
  placement: `pcl projects --dry-run --create ...`.

Focused tests run after the patch:

- `cargo test -p pcl-core incident_detail_and_trace_require_auth`
- `cargo test -p pcl-core contracts_remove_calldata_requires_and_sends_assertion_ids`
- `cargo test -p pcl-core missing_refresh_endpoint_returns_refresh_rejected_and_clears_credentials`

All three passed.

Final repo gate:

- `make ci` passed.
- Gate covered `cargo fmt --check`, clippy with `-D warnings`, workspace tests
  with `PCL_AUTH_NO_BROWSER=1`, docs, `scripts/agent-smoke.sh`, and
  `git diff --check`.
