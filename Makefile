# Build the binary
build:
	PCL_BUILD_PHOUNDRY=true PCL_UPDATE_PHOUNDRY=true cargo build --verbose --release

# Install the binary
install:
	PCL_BUILD_PHOUNDRY=true PCL_UPDATE_PHOUNDRY=true cargo install --verbose --path bin/pcl

# Build the contract mocks and run the rust tests
test:
	cargo nextest run --all-features --workspace --locked --no-tests=warn

# Validate formatting
format-check:
	cargo fmt --check

# Format
format:
	cargo +nightly fmt

# Lint
lint:
	cargo +nightly clippy --workspace

# Errors if there is a warning with clippy
lint-check:
	cargo +nightly clippy -- -D warnings

# Can be used as a manual pre-commit check
pre-commit:
	cargo fmt && make lint
