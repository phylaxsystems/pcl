#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo build -q -p pcl
bin="$repo_root/target/debug/pcl"
config_dir="$(mktemp -d)"
missing_auth_config_dir="$(mktemp -d)"
expired_auth_config_dir="$(mktemp -d)"
verify_project_dir="$(mktemp -d)"
trap 'rm -rf "$config_dir" "$missing_auth_config_dir" "$expired_auth_config_dir" "$verify_project_dir"' EXIT

cat > "$config_dir/config.toml" <<'CONFIG'
[auth]
access_token = "agent-smoke-token"
refresh_token = "agent-smoke-refresh-token"
expires_at = 4102444800
email = "agent-smoke@example.com"
CONFIG

cat > "$expired_auth_config_dir/config.toml" <<'CONFIG'
[auth]
access_token = "expired-token"
refresh_token = "agent-smoke-refresh-token"
expires_at = 1
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

toon_ok() {
  output="$("$bin" --config-dir "$config_dir" --toon "$@")"
  grep -q "schema_version: pcl.envelope.v1" <<<"$output"
  grep -q "status: ok" <<<"$output"
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

toon_error_starts_with_envelope() {
  set +e
  output="$("$bin" --config-dir "$1" --toon "${@:2}" 2>&1)"
  status=$?
  set -e
  test "$status" -ne 0
  test "$(head -n 1 <<<"$output")" = "status: error"
  grep -q "schema_version: pcl.envelope.v1" <<<"$output"
}

toon_envelope --llms
toon_ok --help
toon_envelope llms
toon_envelope doctor --offline
missing_auth_doctor="$("$bin" --config-dir "$missing_auth_config_dir" --toon doctor --offline)"
grep -q "pcl auth ensure --toon" <<<"$missing_auth_doctor"
PCL_AUTH_URL=http://127.0.0.1:9 toon_error_starts_with_envelope "$expired_auth_config_dir" auth login
toon_envelope auth ensure
toon_envelope whoami
toon_envelope workflows
toon_envelope workflows show incident-investigation
toon_envelope schema list
toon_envelope schema get incidents --action list_public
toon_envelope api manifest
toon_envelope projects create --body-template
toon_envelope releases preview project-1 --body-template
toon_envelope access invite project-1 --body-template
toon_envelope completions bash
toon_error build

"$bin" verify --help >/dev/null
cp -R "$repo_root/crates/pcl/cli/tests/fixtures/verify-project/." "$verify_project_dir/"
"$bin" --config-dir "$config_dir" --json apply --root "$verify_project_dir" --dry-run | python3 -c 'import json, sys
doc = json.load(sys.stdin)
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") == "ok", doc
verification = doc.get("data", {}).get("verification")
assert verification and verification.get("status") == "success", doc
assert verification.get("passed") == 1, doc
' >/dev/null

json_envelope llms
json_envelope --help
json_envelope api manifest
json_envelope completions bash
