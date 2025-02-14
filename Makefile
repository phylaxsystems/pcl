# Build the binary
build:
	cargo build --verbose --release

# Build the contract mocks and run the rust tests
test:
	forge build --root contract-mocks && cargo test --verbose

# Build the contract mocks and run the rust tests using the optimism feature flag
test-optimism:
	forge build --root contract-mocks && cargo test --verbose --features optimism

# Validate formatting
format:
	cargo fmt --check

# Errors if there is a warning with clippy
lint:
	cargo clippy  -- -D warnings

# Run foundry tests against the contract mocks
test-mocks:
	forge test --root contract-mocks -vvv

# Can be used as a manual pre-commit check
pre-commit:
	cargo fmt && make lint