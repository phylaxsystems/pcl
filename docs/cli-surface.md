# PCL CLI Surface Contract

This document defines the intended command surface for PCL. It is the review
contract for keeping the CLI broad enough to replace UI workflows without
letting the command tree become accidental bloat.

## Surface Layers

Layer 1: workflow commands

- `incidents`
- `projects`
- `assertions`
- `contracts`
- `releases`
- `deployments`
- `access`
- `integrations`
- `protocol-manager`
- `transfers`
- `events`
- `search`
- `account`

Layer 2: agent and product surface

- `doctor`
- `whoami`
- `workflows`
- `schema`
- `llms`
- `export`
- `jobs`
- `artifacts`
- `requests`

Layer 3: raw, debug, and developer surface

- `api list`
- `api inspect`
- `api call`
- `api coverage`
- `config`
- `completions`
- `build`
- `test`
- `verify`
- `apply`
- `download`

Normal product work should start with Layer 1 or Layer 2. `pcl api` is the raw
escape hatch for debugging, endpoint exploration, parity checks, and operations
that do not yet have a workflow command.

## Product Rule

Full UI replacement does not mean every endpoint becomes a bespoke top-level
command. It means every operation is discoverable, callable, inspectable, safe,
and machine-readable.

An endpoint should become a first-class workflow action only when all of these
are true:

1. It represents a common user job.
2. It appears in the UI or core user lifecycle.
3. It needs better ergonomics than raw `pcl api call`.
4. It has a manifest action.
5. It has schema documentation.
6. It has a body template when it accepts a body.
7. It supports dry-run if it is a write.
8. It has human, TOON, and JSON tests.

Everything else should remain available through `pcl api`.

## Envelope Contract

Agent-facing output uses a root envelope:

```json
{
  "status": "ok | error | action_required | pending | warning",
  "data": {},
  "error": {
    "code": "string",
    "message": "string",
    "recoverable": true
  },
  "next_actions": [],
  "schema_version": "pcl.envelope.v1",
  "pcl_version": "..."
}
```

Root status values are intentionally small. Command-specific outcomes belong in
`data.outcome`, not in root `status`.

Examples:

```json
{
  "status": "ok",
  "data": {
    "outcome": "no_changes"
  }
}
```

```json
{
  "status": "error",
  "data": {
    "total": 4,
    "passed": 3,
    "failed": 1
  },
  "error": {
    "code": "verify.assertions_failed",
    "message": "1 of 4 assertions failed verification",
    "recoverable": true
  }
}
```

## Output Modes

Default human output:

- Success prints readable output to stdout.
- Failure prints readable diagnostics to stderr.
- Root `next_actions` should be useful commands without machine-mode flags.

`--toon`:

- Success prints a compact envelope to stdout.
- Failure prints a compact envelope to stderr.
- Root command `next_actions` should use `--toon` when the suggested command is
  intended to be consumed by the same agent.

`--json`:

- Success prints strict JSON to stdout.
- Failure prints strict JSON to stderr.
- Root command `next_actions` should use `--json` when the suggested command is
  intended to be consumed by the same program.

Fresh `pcl auth login --json` is the exception: it may stream JSONL progress
events. The final event must include `terminal: true` and should be treated as
the final login result.

## Pass-Through Commands

`pcl build` and `pcl test` are developer pass-through commands. Human mode may
use Foundry/Phorge-native output.

In `--toon` or `--json`, pass-through commands must not dump unstructured native
tool output. They should return a structured error explaining that the command
is pass-through-only and pointing agents to structured workflows such as
`pcl verify` and `pcl apply --dry-run`.

## Next Actions

Root `next_actions` are part of the current output contract and must be
mode-aware:

- Human: `pcl auth refresh`
- TOON: `pcl auth refresh --toon`
- JSON: `pcl auth refresh --json`

Do not rewrite examples inside `data`, manifests, schemas, or LLM guides. Those
may intentionally mention `--toon` because they are instructions for agents.

## Anti-Bloat Rules

- Prefer subcommands over adding more mutually exclusive action flags.
- Do not add new `ArgGroup`s with more than five mutually exclusive action
  flags; create subcommands instead.
- New workflow output should prefer `_display` metadata over new human-renderer
  shape-detection branches.
- Keep raw API commands available and documented as the escape hatch.
- Keep `--toon` and `--json` separate. They serve different consumers.
