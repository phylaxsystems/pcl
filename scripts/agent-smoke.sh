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

# `platform_url` is recorded because there is no default platform: every
# command that talks to the API resolves one from the flag, the environment, or
# this remembered value, and errors rather than prompting when run non-TTY.
cat > "$config_dir/config.toml" <<'CONFIG'
platform_url = "https://linea.phylax.systems"

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

json_ok() {
  "$bin" --config-dir "$config_dir" --json "$@" | python3 -c 'import json, sys
doc = json.load(sys.stdin)
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") == "ok", doc
' >/dev/null
}

json_error() {
  set +e
  output="$("$bin" --config-dir "$config_dir" --json "$@" 2>&1 >/dev/null)"
  status=$?
  set -e
  test "$status" -ne 0
  python3 -c 'import json, sys
doc = json.loads(sys.argv[1])
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") == "error", doc
' "$output"
}

json_error_envelope() {
  set +e
  output="$("$bin" --config-dir "$1" --json "${@:2}" 2>&1)"
  status=$?
  set -e
  test "$status" -ne 0
  python3 -c 'import json, sys
doc = json.loads(sys.argv[1])
assert doc.get("schema_version") == "pcl.envelope.v1", doc
assert doc.get("status") == "error", doc
' "$output"
}

json_envelope --llms
json_ok --help
json_envelope llms
json_envelope doctor --offline
missing_auth_doctor="$("$bin" --config-dir "$missing_auth_config_dir" --json doctor --offline)"
grep -q "pcl auth ensure --json" <<<"$missing_auth_doctor"
PCL_AUTH_URL=http://127.0.0.1:9 json_error_envelope "$expired_auth_config_dir" auth login
json_envelope auth ensure
json_envelope whoami
json_envelope workflows
json_envelope workflows show incident-investigation
json_envelope schema list
json_envelope schema get incidents --action list_public
json_envelope api manifest
json_envelope projects create --body-template
json_envelope releases preview project-1 --body-template
json_envelope access invite project-1 --body-template
json_envelope completions bash
json_error build

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
