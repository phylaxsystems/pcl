# Security

## Reporting Vulnerabilities

Please report suspected vulnerabilities privately to the Phylax team. Do not open a public issue with exploit details, tokens, private project identifiers, or production request bodies.

Include:

- Affected command or API endpoint.
- PCL version and platform URL.
- Reproduction steps.
- Relevant request IDs, incident IDs, project IDs, or transaction hashes.
- Whether the issue affects auth, signing, deployments, assertion submission, incident data, or generated API clients.

## Dependency Advisories

Run:

```bash
make audit
```

The advisory gate uses `cargo deny check advisories`. Some current ignores are transitive through upstream Foundry/Alloy dependencies and are recorded in `deny.toml`. New ignores should include a reason and an upstream owner or tracking plan.

## Sensitive Data

Do not commit:

- Auth tokens or refresh tokens.
- `.env` files.
- Private assertion project data.
- Incident exports that include private customer data.
- Local PCL artifact directories unless explicitly sanitized.

When sharing CLI failures, prefer structured error envelopes and request IDs over raw authorization headers or full config files.
