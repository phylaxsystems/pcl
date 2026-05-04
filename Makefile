.PHONY: ci fmt-check clippy test doc diff-check audit regenerate regenerate-dev

ci: fmt-check clippy test doc diff-check

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo +nightly-2026-01-07 clippy --all-targets --workspace --locked --profile dev -- -D warnings -D clippy::pedantic

test:
	PCL_AUTH_NO_BROWSER=1 cargo test -q --workspace --all-targets

doc:
	cargo doc -q --workspace --no-deps

diff-check:
	git diff --check

audit:
	cargo deny --all-features check advisories

# Regenerate dapp-api-client from latest OpenAPI spec
regenerate:
	@echo "Regenerating dapp-api-client from latest OpenAPI spec..."
	cd crates/dapp-api-client && FORCE_SPEC_REGENERATE=true cargo build --features regenerate
	@echo "Client regenerated! Review changes with: git diff crates/dapp-api-client/src/generated/"

# Regenerate dapp-api-client from development environment
regenerate-dev:
	@echo "Regenerating dapp-api-client from development API (localhost:3000)..."
	cd crates/dapp-api-client && DAPP_ENV=development FORCE_SPEC_REGENERATE=true cargo build --features regenerate
	@echo "Client regenerated! Review changes with: git diff crates/dapp-api-client/src/generated/"
