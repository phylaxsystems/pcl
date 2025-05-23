# Build the binary
build:
	cargo build --verbose --release

# Install the binary
install:
	cargo install --locked --verbose --path bin/pcl

# Build the contract mocks and run the rust tests
test:
	cargo nextest run --all-features --workspace --locked 

# Validate formatting
format-check:
	cargo +nightly fmt --check

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
	make format && make lint
