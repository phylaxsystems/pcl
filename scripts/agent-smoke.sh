#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo build -q -p pcl
bin="$repo_root/target/debug/pcl"
config_dir="$(mktemp -d)"
trap 'rm -rf "$config_dir"' EXIT

cat > "$config_dir/config.toml" <<'CONFIG'
[auth]
access_token = "agent-smoke-token"
refresh_token = "agent-smoke-refresh-token"
expires_at = 4102444800
email = "agent-smoke@example.com"
CONFIG

json_envelope() {
  "$bin" --config-dir "$config_dir" --format json "$@" | python3 -c 'import json, sys
doc = json.load(sys.stdin)
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") in {"ok", "warning", "pending", "action_required"}, doc
' >/dev/null
}

toon_envelope() {
  "$bin" --config-dir "$config_dir" --format toon "$@" | grep -q "schema_version: pcl.envelope.v1"
}

"$bin" --config-dir "$config_dir" --format json --llms | python3 -c 'import json, sys
doc = json.load(sys.stdin)
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") == "ok", doc
' >/dev/null
json_envelope llms
json_envelope doctor --offline
json_envelope auth ensure
json_envelope whoami
json_envelope workflows
json_envelope workflows show incident-investigation
json_envelope schema list
json_envelope schema get incidents --action list_public
json_envelope api manifest
json_envelope api --dry-run --allow-unauthenticated call get '/health?limit=5'
json_envelope completions bash

toon_envelope doctor --offline
toon_envelope auth ensure
toon_envelope api --dry-run --allow-unauthenticated call get '/health?limit=5'
