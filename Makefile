# Build the binary
build:
	PCL_BUILD_PHOUNDRY=1 cargo build --verbose --release

# Build the contract mocks and run the rust tests
test:
	cargo test --verbose --workspace

# Validate formatting
format-check:
	cargo fmt --check

# Format
format:
	cargo fmt

# Lint
lint:
	cargo clippy --workspace

# Errors if there is a warning with clippy
lint-check:
	cargo clippy  -- -D warnings

# Can be used as a manual pre-commit check
pre-commit:
	cargo fmt && make lint