# API inventory

Static inventory from dapp revision [`9eec477`](https://github.com/phylaxsystems/credible-layer-dapp/commit/9eec47730154fe9ac5fcf8d9721b5fece449bf18), `packages/dapp-api/openapi.json`, OpenAPI 25.2.0.
This is schema coverage, not evidence of runtime success. Live production can lag this revision.
Use `pcl api inspect <operation-id> --json` for current parameters, auth, body, and workflow alternatives.
Use `pcl api coverage --json` for observed request coverage; uncalled mutations are not validated by this inventory.

125 operations across 109 paths.

Live check on 2026-11-06: app, ethereum and linea each serve OpenAPI **23.3.0** and advertise **130** operations, including every pinned ID plus five retired routes. This PR targets current dapp main; raw API calls remain available for deployments retaining legacy routes. See [read-only receipts](production-readonly.json).

| Operation | Method | API path |
|---|---|---|
| `admin.projectTier.preview` | GET | `/admin/projects/{projectId}/tier` |
| `admin.projectTier.change` | POST | `/admin/projects/{projectId}/tier` |
| `admin.reviewables.queue` | GET | `/admin/reviewables` |
| `admin.reviewables.detail` | GET | `/admin/reviewables/{reviewable_ref}` |
| `admin.reviewables.decision` | POST | `/admin/reviewables/{reviewable_ref}/decision` |
| `agent.getAssertionSummaryManifest` | GET | `/agent/assertion-summaries/{id}` |
| `agent.getAssertionSource` | GET | `/agent/assertions/{assertionId}/source` |
| `agent.getAuditDriftManifest` | GET | `/agent/audit-drift-reports/{id}/manifest` |
| `agent.health` | GET | `/agent/health` |
| `agent.getProjectSummary` | GET | `/agent/project-summaries/{id}` |
| `agent.getOpRiskContext` | GET | `/agent/projects/{projectId}/op-risk-context` |
| `agentic.reportResult` | POST | `/agentic/events` |
| `assertions.getAssertions` | GET | `/assertions` |
| `auth.refresh` | POST | `/auth/refresh` |
| `backtesting.reportResult` | POST | `/backtesting/events` |
| `cli.auth.requestCode` | GET | `/cli/auth/code` |
| `cli.auth.status` | GET | `/cli/auth/status` |
| `cli.auth.verify` | POST | `/cli/auth/verify` |
| `enforcer.reportCircuitBreakerState` | POST | `/enforcer/circuit-breaker-state` |
| `enforcer.reportInvalidation` | POST | `/enforcer/invalidations` |
| `enforcer.reportTraceInvalidation` | POST | `/enforcer/invalidations/traces` |
| `enforcer.reportNetworkStatus` | POST | `/enforcer/network-status` |
| `health` | GET | `/health` |
| `indexer.reportAssertionRegistered` | POST | `/indexer/assertion-registered` |
| `indexer.reportAssertionRemoved` | POST | `/indexer/assertion-removed` |
| `indexer.reportStorageReset` | POST | `/indexer/storage-reset` |
| `indexer.reportTransferAccepted` | POST | `/indexer/transfer-accepted` |
| `getInvalidation` | GET | `/invalidations/{invalidation_id}` |
| `getTrace` | GET | `/invalidations/{invalidation_id}/transactions/{invalidating_transaction_id}/trace` |
| `retryTrace` | POST | `/invalidations/{invalidation_id}/transactions/{invalidating_transaction_id}/trace/retry` |
| `getPendingInvitations` | GET | `/invitations/pending` |
| `acceptInvitation` | POST | `/invitations/{token}/accept` |
| `previewInvitation` | GET | `/invitations/{token}/preview` |
| `openapi` | GET | `/openapi` |
| `projects.getProjects` | GET | `/projects` |
| `projects.createProject` | POST | `/projects` |
| `projects.resolveProjectRef` | GET | `/projects/resolve/{project_ref}` |
| `projects.unsaveProject` | DELETE | `/projects/saved` |
| `projects.getSavedProjects` | GET | `/projects/saved` |
| `projects.saveProject` | POST | `/projects/saved` |
| `projects.deleteProject` | DELETE | `/projects/{project_id}` |
| `projects.getProject` | GET | `/projects/{project_id}` |
| `projects.updateProject` | PUT | `/projects/{project_id}` |
| `projects.triggerAuditDrift` | POST | `/projects/{project_id}/audit-drift` |
| `projects.listAudits` | GET | `/projects/{project_id}/audits` |
| `projects.submitAudit` | POST | `/projects/{project_id}/audits` |
| `projects.mintAuditUploadUrl` | POST | `/projects/{project_id}/audits/upload-url` |
| `projects.getAuditDetail` | GET | `/projects/{project_id}/audits/{audit_report_ref}` |
| `projects.replaceAudit` | POST | `/projects/{project_id}/audits/{audit_report_ref}/replace` |
| `projects.withdrawAudit` | POST | `/projects/{project_id}/audits/{audit_report_ref}/withdraw` |
| `projects.recordCircuitBreakerResetAttempt` | POST | `/projects/{project_id}/circuit-breaker-resets/attempts` |
| `projects.prepareCircuitBreakerReset` | POST | `/projects/{project_id}/circuit-breaker-resets/prepare` |
| `deletePagerDutyIntegration` | DELETE | `/projects/{project_id}/integrations/pagerduty` |
| `getPagerDutyIntegration` | GET | `/projects/{project_id}/integrations/pagerduty` |
| `configurePagerDutyIntegration` | POST | `/projects/{project_id}/integrations/pagerduty` |
| `testPagerDutyIntegration` | POST | `/projects/{project_id}/integrations/pagerduty/test` |
| `deleteSlackIntegration` | DELETE | `/projects/{project_id}/integrations/slack` |
| `getSlackIntegration` | GET | `/projects/{project_id}/integrations/slack` |
| `configureSlackIntegration` | POST | `/projects/{project_id}/integrations/slack` |
| `testSlackIntegration` | POST | `/projects/{project_id}/integrations/slack/test` |
| `getInvalidations` | GET | `/projects/{project_id}/invalidations` |
| `getInvalidationStats` | GET | `/projects/{project_id}/invalidations/stats` |
| `getProjectInvitations` | GET | `/projects/{project_id}/invitations` |
| `createInvitation` | POST | `/projects/{project_id}/invitations` |
| `revokeInvitation` | DELETE | `/projects/{project_id}/invitations/{invitation_id}` |
| `resendInvitation` | POST | `/projects/{project_id}/invitations/{invitation_id}/resend` |
| `getProjectMembers` | GET | `/projects/{project_id}/members` |
| `removeMember` | DELETE | `/projects/{project_id}/members/{member_user_id}` |
| `updateMemberRole` | PATCH | `/projects/{project_id}/members/{member_user_id}` |
| `getMyProjectRole` | GET | `/projects/{project_id}/my-role` |
| `projects.deleteProtocolManager` | DELETE | `/projects/{project_id}/protocol-manager` |
| `projects.setProtocolManager` | POST | `/projects/{project_id}/protocol-manager` |
| `projects.getAcceptTransferCalldata` | GET | `/projects/{project_id}/protocol-manager/accept-calldata` |
| `projects.confirmTransfer` | POST | `/projects/{project_id}/protocol-manager/confirm-transfer` |
| `projects.confirmTransferRequest` | POST | `/projects/{project_id}/protocol-manager/confirm-transfer-request` |
| `projects.getProtocolManagerNonce` | GET | `/projects/{project_id}/protocol-manager/nonce` |
| `projects.getPendingTransfer` | GET | `/projects/{project_id}/protocol-manager/pending-transfer` |
| `projects.getTransferManagerCalldata` | GET | `/projects/{project_id}/protocol-manager/transfer-calldata` |
| `projects.listReleases` | GET | `/projects/{project_id}/releases` |
| `projects.createRelease` | POST | `/projects/{project_id}/releases` |
| `projects.previewRelease` | POST | `/projects/{project_id}/releases/preview` |
| `projects.getRelease` | GET | `/projects/{project_id}/releases/{release_id}` |
| `projects.getBacktestProgress` | GET | `/projects/{project_id}/releases/{release_id}/backtest-progress` |
| `projects.retryReleaseCheck` | POST | `/projects/{project_id}/releases/{release_id}/checks/{check_id}/retry` |
| `projects.deployRelease` | POST | `/projects/{project_id}/releases/{release_id}/deploy` |
| `projects.getReleaseDeployCalldata` | GET | `/projects/{project_id}/releases/{release_id}/deploy-calldata` |
| `projects.getReleaseDiff` | GET | `/projects/{project_id}/releases/{release_id}/diff` |
| `projects.renameRelease` | PATCH | `/projects/{project_id}/releases/{release_id}/name` |
| `projects.confirmRemove` | POST | `/projects/{project_id}/releases/{release_id}/remove` |
| `projects.removeCalldata` | GET | `/projects/{project_id}/releases/{release_id}/remove-calldata` |
| `projects.getProjectWidget` | GET | `/projects/{project_id}/widget` |
| `public.getInvalidations` | GET | `/public/invalidations` |
| `search` | GET | `/search` |
| `stats` | GET | `/stats` |
| `tracer.reportResult` | POST | `/tracer/events` |
| `views.invalidationDetail.getInvalidationDetail` | GET | `/views/invalidations/{invalidationId}` |
| `views.invalidationTrace.getInvalidationTrace` | GET | `/views/invalidations/{invalidationId}/transactions/{invalidatingTransactionId}/trace` |
| `views.projectsExplorer.getProjectsExplorer` | GET | `/views/projects` |
| `views.projectsHome.getProjectsHome` | GET | `/views/projects/home` |
| `views.projectAssertions.getProjectAssertions` | GET | `/views/projects/{projectId}/assertions` |
| `views.assertionDetail.getAssertionDetail` | GET | `/views/projects/{projectId}/assertions/{assertionId}` |
| `views.assertionDetail.getAssertionCircuitBreakers` | GET | `/views/projects/{projectId}/assertions/{assertionId}/circuit-breakers` |
| `views.assertionDetail.getAssertionCode` | GET | `/views/projects/{projectId}/assertions/{assertionId}/code` |
| `views.assertionDetail.getAssertionDetailCore` | GET | `/views/projects/{projectId}/assertions/{assertionId}/core` |
| `views.assertionSummary.getAssertionSummary` | GET | `/views/projects/{projectId}/assertions/{assertionId}/summary` |
| `views.auditLog.getAuditLog` | GET | `/views/projects/{projectId}/audit-log` |
| `views.auditLog.exportAuditLogPdf` | POST | `/views/projects/{projectId}/audit-log/pdf` |
| `views.projectAssertions.getProjectCircuitBreakers` | GET | `/views/projects/{projectId}/circuit-breakers` |
| `views.projectContracts.getProjectContracts` | GET | `/views/projects/{projectId}/contracts` |
| `views.contractDetail.getContractDetail` | GET | `/views/projects/{projectId}/contracts/{adopterId}` |
| `views.contractAuditCoverage.getContractAuditCoverage` | GET | `/views/projects/{projectId}/contracts/{chainId}/{contractAddress}/audit-coverage` |
| `views.projectEvents.getProjectEvents` | GET | `/views/projects/{projectId}/events` |
| `views.projectInvalidations.getProjectInvalidations` | GET | `/views/projects/{projectId}/invalidations` |
| `views.projectInvalidations.getLatestRecentInvalidation` | GET | `/views/projects/{projectId}/invalidations/latest-recent` |
| `views.projectOperationalRisk.getProjectOperationalRisk` | GET | `/views/projects/{projectId}/operational-risk` |
| `views.projectSummary.getProjectSummary` | GET | `/views/projects/{projectId}/summary` |
| `views.publicInvalidations.getPublicInvalidations` | GET | `/views/public/invalidations` |
| `web.auth.acceptTerms` | POST | `/web/auth/accept-terms` |
| `web.auth.bootstrapSession` | POST | `/web/auth/bootstrap-session` |
| `web.auth.logout` | POST | `/web/auth/logout` |
| `web.auth.me` | GET | `/web/auth/me` |
| `web.auth.waitlistStatus` | GET | `/web/auth/waitlist` |
| `web.auth.joinWaitlist` | POST | `/web/auth/waitlist` |
| `web.verifiedContract` | GET | `/web/verified-contract` |
| `whitelist` | GET | `/whitelist` |
