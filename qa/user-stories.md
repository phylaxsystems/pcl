# PCL dapp journey validation

Scope: dapp OpenAPI 25.2.0 at `9eec47730154fe9ac5fcf8d9721b5fece449bf18`.
Production validation is read-only, per the requester. A passed local fixture test
is not a claim that its authenticated production journey passed. This table is the
validation source of truth; [API inventory](api-inventory.md) lists the static operations.
New JSON-only endpoints remain discoverable through `api inspect` / `api call`;
not every endpoint has a dedicated top-level command.

| Story ID | Area | Role | User Story | Expected Behavior | Preconditions | Test Steps | Status | Severity | Evidence | Bug / Fix Reference | Retest Result | Owner / Blocker | Last Updated |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| DISC-01 | Discovery | agent | As a agent, I can discover current commands and API contracts | Schemas, recipes and operation IDs match current API | Local binary | workflows; schema list; api manifest | Passed | N/A | Rendered schema/workflow output; live api list on all three hosts | API 25.2 regeneration | Current main: 125 operations; production: 130, none of the pinned IDs missing | Prior-head CI passed; auth continuation under test | 2026-11-06 |
| INV-01 | Invalidations | public reader | As a public reader, I can list invalidations | Read-only public listing succeeds | Production public API | incidents --limit 1 | Passed | N/A | [Production receipts](production-readonly.json) | Existing list workflow | HTTP 200 on app, ethereum and linea; app/linea returned one item, ethereum an empty list | No production login | 2026-11-06 |
| INV-02 | Invalidations | project member | As a project member, I can collect all invalidation context | All raw detail and trace fields, metadata, IDs and numeric strings survive; denied traces are explicit | Mock API; production membership for live check | workflow_cli incident_context_preserves_trace_fields_and_reports_partial_failures | Blocked | N/A | CLI integration fixture | --include-traces; --output | Prior-head CI fixture passed; live unverified | Production member session unavailable | 2026-11-06 |
| INV-03 | Invalidations | project member | As a project member, I can filter incidents and find latest | outcome, sortBy and fromDate reach current routes | Mock API; member session for production | workflow_cli current_incident_routes_use_required_queries | Blocked | N/A | CLI integration fixture | Current query parameters | Prior-head CI fixture passed; live unverified | Production member session unavailable | 2026-11-06 |
| INV-04 | Trace retry | project member | As a project member, I can retry failed trace generation | Retry mutation returns job state and permission errors | Authorized writable test project | incidents --incident-id ID --tx-id TX --retry-trace | Blocked | N/A | Read-only request constraint | Existing retry workflow | Pending | Production mutations prohibited | 2026-11-06 |
| REL-01 | Releases | project member | As a project member, I can compare and rename releases | Diff sends against=active; rename sends exact name body | Mock API | workflow_cli current_release_routes_use_required_queries | Passed | N/A | CLI integration fixture | New release commands | Prior-head CI fixture passed; live unverified | Production writes prohibited | 2026-11-06 |
| AUD-01 | Audit upload | project editor | As a project editor, I can upload a PDF for submission | Validate before mint; exact bytes to signed storage; no platform auth forwarded; return reference | Mock API and storage | workflow_cli audit_pdf_upload_uses_signed_storage_without_forwarding_auth | Passed | N/A | CLI integration fixture | projects upload-audit | Prior-head CI fixture passed; live unverified | Production writes prohibited | 2026-11-06 |
| AUD-02 | Audit log export | project member | As a project member, I can download a PDF | Exact binary bytes saved; HTTP failure does not overwrite artifact | Mock API | workflow_cli raw_api_binary_output_preserves_bytes_and_does_not_overwrite_on_http_error | Passed | N/A | CLI integration fixture | api call --output-raw | Prior-head CI fixture passed; live unverified | Production member session unavailable | 2026-11-06 |
| AUD-03 | Audit lifecycle | project editor/reviewer | As a project editor/reviewer, I can submit, replace, withdraw and review audits | Current schema discoverable; JSON request/response through api call | Project role; audit fixture; reviewer for reviewables | api inspect projects.submitAudit; inspect each lifecycle operation | Blocked | N/A | Static API inventory only | Raw JSON API fallback | Pending | No authenticated writable test project | 2026-11-06 |
| PROJ-01 | Projects/access | member/admin | As a member/admin, I can manage project settings, team and integrations | Existing workflows map to retained operations | Role-specific authenticated fixtures | projects; access; integrations; account | Blocked | N/A | Prior-head regression suite passed; no production mutation | Retired workflow cleanup from PR #131 | Pending | Production read-only; no role sessions | 2026-11-06 |
| DEP-01 | Assertions/contracts/releases | deployer | As a deployer, I can prepare and confirm deployments/removals | Release-oriented workflow replaces retired legacy routes; exact signing flow verified with fixtures | Local signer fixture; authorized chain for live execution | Existing workflow_cli release deployment tests; retired-command tests | Blocked | N/A | Local CLI regression suite | Reuse PR #131 removal of legacy endpoints | Prior-head CI fixture passed; live unverified | Live chain mutations prohibited | 2026-11-06 |
| RISK-01 | Operational risk | project member | As a project member, I can inspect risk, summaries and circuit-breaker state | Current read endpoints available through inspect/call | Member session and project fixture | Inspect operational risk and circuit-breaker operations in inventory | Blocked | N/A | Static API inventory only | Raw JSON API fallback | Pending | Production member session unavailable | 2026-11-06 |
| RISK-02 | Circuit-breaker reset | authorized signer | As a authorized signer, I can prepare reset and record receipt | API prepare/record plus separate chain signing; no implied automatic broadcast | Authorized signer and writable test chain | Inspect projects.prepareCircuitBreakerReset and recordCircuitBreakerResetAttempt | Blocked | N/A | Static API inventory only | Raw JSON API fallback | Pending | Live chain mutations prohibited | 2026-11-06 |
| AUTH-01 | Authentication | user | As a user, I can log in, refresh and revoke CLI sessions | Generated logout signature compiles; existing auth fixtures pass | Local auth fixtures; consenting user for live session | auth_journey process fixtures; auth status | Blocked | N/A | Four connected auth process tests passed locally | Session reuse and confirmed revocation | Approval, refresh, private traces and logout passed against fixtures; live unverified | Production interactive login not performed | 2026-11-06 |
| SVC-01 | Service endpoints | service agent | As a service agent, I can discover protected agent APIs | Inspect identifies service credentials; normal user session grants no service access | Service credential required for execution | api inspect agent.getProjectSummary | Passed | N/A | Live api inspect agent.getProjectSummary output | /agent and /agentic classification | Discovery correctly reports service bearer, no stored CLI auth; execution untested | No service credentials; service mutations unrun | 2026-11-06 |
| AUTH-02 | Logout recovery | CLI user | As a CLI user, I can retry failed revocation without losing credentials | Errors preserve config; successful retry clears it; local-only cleanup is explicit | Mock API and temporary config | auth_journey failed_logout_retains_credentials_until_confirmed_or_explicit_local_cleanup | Passed | N/A | Local process test: 401, 500, success:false, malformed JSON, retry, --local | auth.logout_failed; preserve server error code and refresh guidance | All fixture cases passed | Live revocation enforcement still unverified | 2026-11-06 |
| AUTH-03 | Authorization boundaries | CLI user | As a CLI user, I never fall back to public trace access or send credentials to a different platform | Denial stops trace fetching; wrong-platform logout preserves config and sends no credentials | Mock API and temporary config | auth_journey denial and platform-mismatch process tests | Passed | N/A | Local process tests: 403, revoked/expired credentials, platform mismatch | Existing permission checks; logout platform guard | All fixture cases passed; existing artifact unchanged on denial | These are CLI behavior checks, not live membership enforcement | 2026-11-06 |


Validation evidence for the recovered head (`6125fe8`):
[GitHub CI](https://github.com/phylaxsystems/pcl/actions/runs/42159093629)
passed all 587 tests, formatting, strict Clippy, full/release checks, docs and agent
smoke. Cargo deny failed on existing dependency advisories; this continuation does
not modify or suppress them. Rows marked Passed with mock preconditions cover
CLI behavior only. Authenticated live journeys remain blocked as recorded above.

Recovery follow-up (2026-11-06): INV-02 inspection found that incident `next_actions`
omitted the supplied project, so following them could lose scoped authorization.
The fix preserves and quotes the project in incident commands; regression fixtures
cover latest-to-detail, detail-to-trace/context, and retry-to-trace handoffs. Restored
the final unpublished retry fixture and schema change from the old session events.
Prior-head CI passed all 587 tests, including project-scope handoffs. The local dapp cannot start its Supabase stack because
this sandbox has no Docker daemon (`/var/run/docker.sock` is absent).

Auth continuation (2026-11-06): using the existing browser-approved CLI session,
without a new API-key type, backend route, or permission model. Review found that
failed remote logout discarded local credentials and reported success. The CLI
now retains credentials and fails explicitly until revocation succeeds or the
user requests local-only cleanup. Connected process fixtures in
`crates/pcl/cli/tests/auth_journey.rs` cover approval → refresh → private detail/trace
→ logout, denied membership, expired/revoked credentials, and failed revocation.
All four connected CLI process tests passed locally. Live membership/revocation enforcement remains blocked:
no authenticated dapp fixture and no local Docker daemon for Supabase.
