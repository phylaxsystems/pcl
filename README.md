# Phylax Credible Layer (PCL) CLI

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

The Phylax Credible CLI (PCL) is a command-line interface for interacting with the Credible Layer. It allows developers to authenticate, build, test, and submit assertions to the Credible Layer dApp.

## Table of Contents

- [Installation](#installation)
- [Usage Guide](#usage-guide)
  - [Authentication](#authentication)
  - [Building Projects](#building-projects)
  - [Phorge Commands](#phorge-commands)
  - [Assertion Submission](#assertion-submission)
- [Examples](#examples)
- [Configuration](#configuration)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)

## Installation

### Prerequisites

- `Rust >= 1.86 nightly`
- `git`

### Build from Source

1. Clone the repository:

   ```bash
   git clone https://github.com/phylax-systems/pcl.git
   cd pcl
   ```

2. Build the CLI:

   ```bash
   make build
   ```

3. The compiled binary will be available in the `target/release` directory.

4. (Optional) Add to your PATH:

   ```bash
   export PATH="$PATH:$(pwd)/target/release"
   ```

## Usage Guide

### Authentication

Before using most commands, you need to authenticate:

```bash
pcl auth [OPTIONS] <COMMAND>

Commands:
  login   Login to PCL using your wallet
  logout  Logout from PCL
  status  Check current authentication status
  help    Print this message or the help of the given subcommand(s)

Options:
      --base-url <BASE_URL>  Base URL for authentication service [env: AUTH_BASE_URL=] [default: https://credible-layer-dapp.pages.dev]
  -h, --help                 Print help
```

When logging in:

1. A URL and authentication code will be displayed
2. Visit the URL in your browser
3. Connect your wallet and approve the authentication
4. CLI will automatically detect successful authentication

### Building Projects

Build your assertion contracts:

```bash
pcl build [OPTIONS] [ASSERTIONS]...

Arguments:
  [ASSERTIONS]...  Names of assertion contracts to build

Options:
  -h, --help  Print help
```

### Phorge Commands

Phorge is a Foundry-compatible development environment for assertions:

```bash
pcl phorge [OPTIONS] [ARGS]...

Arguments:
  [ARGS]...  Arguments to pass to the phorge command

Options:
  -h, --help  Print help
```

Common phorge subcommands:

```bash
pcl phorge test      # Run tests
pcl phorge script    # Run scripts
pcl phorge deploy    # Deploy contracts
```

### Assertion Submission

#### Store Assertions in Data Availability Layer

```bash
pcl store [OPTIONS] <ASSERTION>

Arguments:
  <ASSERTION>  Name of the assertion contract to submit

Options:
      --url <URL>  URL of the assertion-DA [env: PCL_DA_URL=] [default: http://localhost:3000]
  -h, --help       Print help
```

#### Submit Assertions to dApps

```bash
pcl submit [OPTIONS]

Options:
  -d, --dapp-url <DAPP_URL>                 Base URL for the Credible Layer dApp API [default: https://credible-layer-dapp.pages.dev/api/v1]
  -p, --project-name <PROJECT_NAME>         Optional project name to skip interactive selection
  -a, --assertion-name <ASSERTION_NAME>...  Optional list of assertion names to skip interactive selection
  -h, --help                                Print help
```

## Examples

### Complete Authentication Flow

```bash
# Login
pcl auth login

# Verify status
pcl auth status

# Submit assertion
pcl store my_assertion

# Logout when done
pcl auth logout
```

### Development Workflow

```bash
# Build project
pcl build my_assertion

# Run tests
pcl phorge test

# Deploy and submit
pcl phorge deploy
pcl submit
```

### Working with Assertion Directories

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

## Configuration

- Config file location: `~/.pcl/config.toml`
- Stores authentication and submission history
- Automatically created on first use

## Troubleshooting

### Authentication Issues

- **Error: Not authenticated**: Run `pcl auth login` to authenticate
- **Error: Authentication expired**: Run `pcl auth login` to refresh your authentication
- **Browser doesn't open**: Manually visit the URL displayed in the terminal

### Build Issues

- **Error: Assertion not found**: Ensure the assertion name is correct and exists in your project
- **Compilation errors**: Check your assertion code for syntax errors

### Submission Issues

- **Error: Failed to submit**: Ensure you're authenticated and have network connectivity
- **Error: Project not found**: Create a project in the Credible Layer dApp first

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request
