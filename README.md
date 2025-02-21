# Credible CLI

The Credible CLI is a command-line interface for the Credible Layer.

## Installation

### Build from Source

It requires the following:

- `Rust >= 1.86 nightly`
- `git`

After you have installed the above, you can build the CLI by running the following:

```bash
make build
```

This will build the CLI in `target/release` directory.

## Usage Guide

### Authentication

Before using most commands, you need to authenticate:

```bash
pcl auth login    # Start the authentication process
pcl auth logout   # Remove stored credentials
pcl auth status   # Check current authentication status
```

When logging in:

1. A URL and authentication code will be displayed
2. Visit the URL in your browser
3. Connect your wallet and approve the authentication
4. CLI will automatically detect successful authentication

### Assertion Commands

#### DA Submit

Submit assertions to the Data Availability Layer:

```bash
pcl da-submit [OPTIONS] <CONTRACT_ADDRESS> <ASSERTION_ID>
```

Options:

- `--chain`: Specify the target chain
- `--rpc-url`: Custom RPC endpoint

#### Dapp Submit

Submit assertions to dapps:

```bash
pcl dapp-submit [OPTIONS] <CONTRACT_ADDRESS> <ASSERTION_ID>
```

Options:

- `--chain`: Target chain for submission
- `--rpc-url`: Custom RPC endpoint

### Development Commands

#### Build

Build your project:

```bash
pcl build [OPTIONS]
```

Options:

- `--optimize`: Enable optimization
- `--debug`: Include debug information

#### Phorge

Foundry-compatible development commands:

```bash
pcl phorge test      # Run tests
pcl phorge script    # Run scripts
pcl phorge deploy    # Deploy contracts
```

To specify the assertions directory:

```bash
pcl --assertions-dir <PATH> phorge
```

Example:

```bash
pcl --assertions-dir mock-protocol/assertions phorge
```

Phorge expects the following directory structure:

```text
assertions/
  src/     # Source files
  test/    # Test files with .t.sol extension
```

By defining the assertions directory, the CLI will automatically:

- Add the `src` and `test` directories to the phorge command
- Allow sharing of `foundry.toml` and `lib` directory between contracts and assertions

### Configuration

- Config file location: `~/.pcl/config.toml`
- Stores authentication and submission history
- Automatically created on first use

### Common Options

These options work with most commands:

```bash
--help              # Show help for any command
--version           # Show CLI version
--verbose          # Enable verbose output
```

### Examples

#### Complete Authentication Flow

```bash
# Login
pcl auth login

# Verify status
pcl auth status

# Submit assertion
pcl da-submit 0x123... 456

# Logout when done
pcl auth logout
```

#### Development Workflow

```bash
# Build project
pcl build

# Run tests
pcl phorge test

# Deploy and submit
pcl phorge deploy
pcl dapp-submit <deployed-address> <assertion-id>
```

### Error Handling

- Authentication errors: Re-run `pcl auth login`
- Network errors: Check connection and RPC URL
- Build errors: Check contract syntax and dependencies

### Best Practices

1. Always check auth status before submitting
2. Use appropriate chain parameters
3. Keep credentials secure
4. Regular logout when done
