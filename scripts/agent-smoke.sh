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
  "$bin" --config-dir "$config_dir" --json "$@" | python3 -c 'import json, sys
doc = json.load(sys.stdin)
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") in {"ok", "warning", "pending", "action_required"}, doc
' >/dev/null
}

toon_envelope() {
  "$bin" --config-dir "$config_dir" --toon "$@" | grep -q "schema_version: pcl.envelope.v1"
}

toon_error() {
  set +e
  output="$("$bin" --config-dir "$config_dir" --toon "$@" 2>&1 >/dev/null)"
  status=$?
  set -e
  test "$status" -ne 0
  grep -q "schema_version: pcl.envelope.v1" <<<"$output"
  grep -q "status: error" <<<"$output"
}

toon_envelope --llms
toon_envelope llms
toon_envelope doctor --offline
toon_envelope auth ensure
toon_envelope whoami
toon_envelope workflows
toon_envelope workflows show incident-investigation
toon_envelope schema list
toon_envelope schema get incidents --action list_public
toon_envelope api manifest
toon_envelope api --dry-run --allow-unauthenticated call get '/health?limit=5'
toon_envelope projects create --project-name demo --chain-id 1 --dry-run
toon_envelope releases preview project-1 --body-template --dry-run
toon_envelope access invite project-1 --body-template --dry-run
toon_envelope completions bash
toon_error build

json_envelope llms
json_envelope api manifest
json_envelope completions bash
