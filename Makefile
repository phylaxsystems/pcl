.PHONY: ci fmt-check clippy full-check release-check test doc diff-check agent-smoke audit regenerate regenerate-pinned regenerate-dev

ci: fmt-check clippy full-check release-check test doc agent-smoke diff-check

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo +nightly-2026-07-28 clippy --all-targets --workspace --locked --profile dev -- -D warnings -D clippy::pedantic

full-check:
	cargo check --locked --workspace --all-targets --features full
	PCL_AUTH_NO_BROWSER=1 cargo test -q -p pcl --features full --test verify_cli

release-check:
	cargo check --release --locked --package pcl --features full

test:
	PCL_AUTH_NO_BROWSER=1 cargo test -q --workspace --all-targets

doc:
	cargo doc -q --workspace --no-deps

diff-check:
	git diff --check

agent-smoke:
	PCL_AUTH_NO_BROWSER=1 scripts/agent-smoke.sh

audit:
	cargo deny --all-features check advisories

# Spec the committed client was generated from: credible-layer-dapp
# packages/dapp-api/openapi.json (OpenAPI 24.0.0). Bump both together.
DAPP_SPEC_REV ?= e87c2ae3c57028fcf40e0866b5ddea832812d1ca
DAPP_SPEC_SHA256 ?= 95ea3aaad763302f41f4b573f3652011df62fe9b36314ae39c92e81dd4e67a10
DAPP_SPEC_FILE := crates/dapp-api-client/openapi/spec.json

# Regenerate dapp-api-client from the pinned dapp revision (reproducible; needs gh auth)
regenerate-pinned:
	mkdir -p $(dir $(DAPP_SPEC_FILE))
	gh api -H "Accept: application/vnd.github.raw" "repos/phylaxsystems/credible-layer-dapp/contents/packages/dapp-api/openapi.json?ref=$(DAPP_SPEC_REV)" > $(DAPP_SPEC_FILE)
	echo "$(DAPP_SPEC_SHA256)  $(DAPP_SPEC_FILE)" | shasum -a 256 -c
	# Force off: a forced build refetches from the live API over the verified spec.
	cd crates/dapp-api-client && FORCE_SPEC_REGENERATE=false cargo build --features regenerate
	@echo "Client regenerated! Review changes with: git diff crates/dapp-api-client/src/generated/"

# Regenerate dapp-api-client from whatever production serves now (not pinned;
# can revert the committed client to an older spec)
regenerate:
	@echo "Regenerating dapp-api-client from latest OpenAPI spec..."
	cd crates/dapp-api-client && FORCE_SPEC_REGENERATE=true cargo build --features regenerate
	@echo "Client regenerated! Review changes with: git diff crates/dapp-api-client/src/generated/"

# Regenerate dapp-api-client from development environment
regenerate-dev:
	@echo "Regenerating dapp-api-client from development API (localhost:3000)..."
	cd crates/dapp-api-client && DAPP_ENV=development FORCE_SPEC_REGENERATE=true cargo build --features regenerate
	@echo "Client regenerated! Review changes with: git diff crates/dapp-api-client/src/generated/"
