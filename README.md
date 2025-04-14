# Phylax Credible Layer (PCL) CLI

[![License: GPL-3.0](https://img.shields.io/badge/License-GPL--3.0-blue.svg)](https://opensource.org/licenses/GPL-3.0)
[![Rust](https://github.com/phylaxsystems/pcl/actions/workflows/rust.yml/badge.svg)](https://github.com/phylaxsystems/pcl/actions/workflows/rust.yml)
[![Clippy](https://github.com/phylaxsystems/pcl/actions/workflows/clippy.yml/badge.svg)](https://github.com/phylaxsystems/pcl/actions/workflows/clippy.yml)
[![Format](https://github.com/phylaxsystems/pcl/actions/workflows/format.yml/badge.svg)](https://github.com/phylaxsystems/pcl/actions/workflows/format.yml)

The Phylax Credible CLI (PCL) is a command-line interface for interacting with the Credible Layer. It allows developers to authenticate, build, test, and submit assertions to the Credible Layer dApp.

## Table of Contents

- [Phylax Credible Layer (PCL) CLI](#phylax-credible-layer-pcl-cli)
  - [Table of Contents](#table-of-contents)
  - [Installation](#installation)
    - [Prerequisites](#prerequisites)
    - [Build from Source](#build-from-source)
  - [Usage Guide](#usage-guide)
    - [Authentication](#authentication)
    - [Configuration](#configuration)
    - [Testing](#testing)
    - [Assertion Submission](#assertion-submission)
      - [Store Assertions in Data Availability Layer](#store-assertions-in-data-availability-layer)
      - [Submit Assertions to dApps](#submit-assertions-to-dapps)
  - [Examples](#examples)
    - [Complete Authentication Flow](#complete-authentication-flow)
    - [Development Workflow](#development-workflow)
  - [Troubleshooting](#troubleshooting)
    - [Authentication Issues](#authentication-issues)
    - [Submission Issues](#submission-issues)
  - [Contributing](#contributing)
    - [Development Setup](#development-setup)

## Installation

### Prerequisites

- `Rust >= 1.80`
- `git`

### Build from Source

1. Clone the repository:

   ```bash
   git clone https://github.com/phylaxsystems/pcl.git
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

Options:
      --base-url <BASE_URL>  Base URL for authentication service [env: AUTH_BASE_URL=] [default: https://credible-layer-dapp.pages.dev]
  -h, --help                 Print help
```

When logging in:

1. A URL and authentication code will be displayed
2. Visit the URL in your browser
3. Connect your wallet and approve the authentication
4. CLI will automatically detect successful authentication

### Configuration

Manage your PCL configuration:

```bash
pcl config [COMMAND]

Commands:
  show    Display the current configuration
  delete  Delete the current configuration
```

Configuration is stored in `~/.pcl/config.toml` and includes:

- Authentication token
- Pending assertions for submission
- Project settings

### Testing

Run tests using Phorge (a Forge-compatible development environment). It's a minimal fork of forge to support out assertion execution cheatcodes, so `pcl test` behaves identically to `forge test`.

```pcl test -h
Run tests using Phorge

Usage: pcl test [OPTIONS] [PATH]

Options:
  -h, --help  Print help (see more with '--help')

Display options:
  -v, --verbosity...                Verbosity level of the log messages.
  -q, --quiet                       Do not print log messages
      --json                        Format log messages as JSON
      --color <COLOR>               The color of the log messages [possible values: auto, always, never]
  -s, --suppress-successful-traces  Suppress successful test traces and show only traces for failures [env: FORGE_SUPPRESS_SUCCESSFUL_TRACES=]
      --junit                       Output test results as JUnit XML report
  -l, --list                        List tests instead of running them
      --show-progress               Show test execution progress
      --summary                     Print test summary table
      --detailed                    Print detailed test summary table

Test options:
  -j, --threads <THREADS>                        Number of threads to use. Specifying 0 defaults to the number of logical cores [aliases: jobs]
      --debug                                    Run a single test in the debugger
      --flamegraph                               Generate a flamegraph for a single test. Implies `--decode-internal`
      --flamechart                               Generate a flamechart for a single test. Implies `--decode-internal`
      --decode-internal                          Identify internal functions in traces
      --dump <PATH>                              Dumps all debugger steps to file
      --gas-report                               Print a gas report [env: FORGE_GAS_REPORT=]
      --gas-snapshot-check <GAS_SNAPSHOT_CHECK>  Check gas snapshots against previous runs [env: FORGE_SNAPSHOT_CHECK=] [possible values: true, false]
      --gas-snapshot-emit <GAS_SNAPSHOT_EMIT>    Enable/disable recording of gas snapshot results [env: FORGE_SNAPSHOT_EMIT=] [possible values: true, false]
      --allow-failure                            Exit with code 0 even if a test fails [env: FORGE_ALLOW_FAILURE=]
      --fail-fast                                Stop running tests after the first failure
      --etherscan-api-key <KEY>                  The Etherscan (or equivalent) API key [env: ETHERSCAN_API_KEY=]
      --fuzz-seed <FUZZ_SEED>                    Set seed used to generate randomness during your fuzz runs
      --fuzz-runs <RUNS>                         [env: FOUNDRY_FUZZ_RUNS=]
      --fuzz-timeout <TIMEOUT>                   Timeout for each fuzz run in seconds [env: FOUNDRY_FUZZ_TIMEOUT=]
      --fuzz-input-file <FUZZ_INPUT_FILE>        File to rerun fuzz failures from
      --rerun                                    Re-run recorded test failures from last run. If no failure recorded then regular test run is performed
  [PATH]                                         The contract file you want to test, it's a shortcut for --match-path

Test filtering:
      --match-test <REGEX>         Only run test functions matching the specified regex pattern [aliases: mt]
      --no-match-test <REGEX>      Only run test functions that do not match the specified regex pattern [aliases: nmt]
      --match-contract <REGEX>     Only run tests in contracts matching the specified regex pattern [aliases: mc]
      --no-match-contract <REGEX>  Only run tests in contracts that do not match the specified regex pattern [aliases: nmc]
      --match-path <GLOB>          Only run tests in source files matching the specified glob pattern [aliases: mp]
      --no-match-path <GLOB>       Only run tests in source files that do not match the specified glob pattern [aliases: nmp]
      --no-match-coverage <REGEX>  Only show coverage for files that do not match the specified regex pattern [aliases: nmco]

EVM options:
  -f, --fork-url <URL>                Fetch state over a remote endpoint instead of starting from an empty state [aliases: rpc-url]
      --fork-block-number <BLOCK>     Fetch state from a specific block number over a remote endpoint
      --fork-retries <RETRIES>        Number of retries
      --fork-retry-backoff <BACKOFF>  Initial retry backoff on encountering errors
      --no-storage-caching            Explicitly disables the use of RPC caching
      --initial-balance <BALANCE>     The initial balance of deployed test contracts
      --sender <ADDRESS>              The address which will be executing tests/scripts
      --ffi                           Enable the FFI cheatcode
      --always-use-create-2-factory   Use the create 2 factory in all cases including tests and non-broadcasting scripts
      --create2-deployer <ADDRESS>    The CREATE2 deployer address to use, this will override the one in the config

Fork config:
      --compute-units-per-second <CUPS>  Sets the number of assumed available compute units per second for this provider
      --no-rpc-rate-limit                Disables rate limiting for this node's provider [aliases: no-rate-limit]

Executor environment config:
      --code-size-limit <CODE_SIZE>    EIP-170: Contract code size limit in bytes. Useful to increase this because of tests. By default, it is 0x6000 (~25kb)
      --chain <CHAIN>                  The chain name or EIP-155 chain ID [aliases: chain-id]
      --gas-price <GAS_PRICE>          The gas price
      --block-base-fee-per-gas <FEE>   The base fee in a block [aliases: base-fee]
      --tx-origin <ADDRESS>            The transaction origin
      --block-coinbase <ADDRESS>       The coinbase of the block
      --block-timestamp <TIMESTAMP>    The timestamp of the block
      --block-number <BLOCK>           The block number
      --block-difficulty <DIFFICULTY>  The block difficulty
      --block-prevrandao <PREVRANDAO>  The block prevrandao value. NOTE: Before merge this field was mix_hash
      --block-gas-limit <GAS_LIMIT>    The block gas limit [aliases: gas-limit]
      --memory-limit <MEMORY_LIMIT>    The memory limit per EVM execution in bytes. If this limit is exceeded, a `MemoryLimitOOG` result is thrown
      --disable-block-gas-limit        Whether to disable the block gas limit checks [aliases: no-gas-limit]
      --isolate                        Whether to enable isolation of calls. In isolation mode all top-level calls are executed as a separate transaction in a separate EVM context, enabling more precise gas
                                       accounting and transaction state changes
      --odyssey                        Whether to enable Odyssey features

Cache options:
      --force  Clear the cache and artifacts folder and recompile

Build options:
      --no-cache              Disable the cache
      --dynamic-test-linking  Enable dynamic test linking
      --eof                   Whether to compile contracts to EOF bytecode
      --skip <SKIP>...        Skip building files whose names contain the given filter

Linker options:
      --libraries <LIBRARIES>  Set pre-linked libraries [env: DAPP_LIBRARIES=]

Compiler options:
      --ignored-error-codes <ERROR_CODES>  Ignore solc warnings by error code
      --deny-warnings                      Warnings will trigger a compiler error
      --no-auto-detect                     Do not auto-detect the `solc` version
      --use <SOLC_VERSION>                 Specify the solc version, or a path to a local solc, to build with
      --offline                            Do not access the network
      --via-ir                             Use the Yul intermediate representation compilation pipeline
      --use-literal-content                Changes compilation to only use literal content and not URLs
      --no-metadata                        Do not append any metadata to the bytecode
      --ast                                Includes the AST as JSON in the compiler output
      --evm-version <VERSION>              The target EVM version
      --optimize [<OPTIMIZE>]              Activate the Solidity optimizer [possible values: true, false]
      --optimizer-runs <RUNS>              The number of runs specifies roughly how often each opcode of the deployed code will be executed across the life-time of the contract. This means it is a trade-off
                                           parameter between code size (deploy cost) and code execution cost (cost after deployment). An `optimizer_runs` parameter of `1` will produce short but expensive code.
                                           In contrast, a larger `optimizer_runs` parameter will produce longer but more gas efficient code
      --extra-output <SELECTOR>...         Extra output to include in the contract's artifact
      --extra-output-files <SELECTOR>...   Extra output to write to separate files

Project options:
  -o, --out <PATH>               The path to the contract artifacts folder
      --revert-strings <REVERT>  Revert string configuration
      --build-info               Generate build info files
      --build-info-path <PATH>   Output path to directory that build info files will be written to
      --root <PATH>              The project's root path
  -C, --contracts <PATH>         The contracts source directory
  -R, --remappings <REMAPPINGS>  The project's remappings
      --remappings-env <ENV>     The project's remappings from the environment
      --cache-path <PATH>        The path to the compiler cache
      --lib-paths <PATH>         The path to the library folder
      --hardhat                  Use the Hardhat-style project layout [aliases: hh]
      --config-path <FILE>       Path to the config file

Watch options:
  -w, --watch [<PATH>...]    Watch the given files or directories for changes
      --no-restart           Do not restart the command while it's still running
      --run-all              Explicitly re-run all tests when a change is made
      --watch-delay <DELAY>  File update debounce delay
```

### Assertion Submission

#### Store Assertions in Data Availability Layer

```bash
pcl store [OPTIONS] <ASSERTION>

Arguments:
  <ASSERTION>  Name of the assertion contract to submit

Options:
      --url <URL>  URL of the assertion-DA [env: PCL_DA_URL=] [default: http://localhost:5001]
  -h, --help       Print help
```

#### Submit Assertions to dApps

```bash
pcl submit [OPTIONS]

Options:
  -u, --dapp-url <DAPP_URL>                 Base URL for the Credible Layer dApp API [default: http://localhost:3003/api/v1]
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

# Store assertion
pcl store my_assertion

# Submit to dApp
pcl submit -a my_assertion -p my_project

# Logout when done
pcl auth logout
```

### Development Workflow

```bash
# Run tests
pcl test

# Store and submit assertion
pcl store my_assertion
pcl submit -a my_assertion -p my_project
```

## Troubleshooting

### Authentication Issues

- **Error: Not authenticated**: Run `pcl auth login` to authenticate
- **Error: Authentication expired**: Run `pcl auth login` to refresh your authentication
- **Browser doesn't open**: Manually visit the URL displayed in the terminal

### Submission Issues

- **Error: Failed to submit**: Ensure you're authenticated and have network connectivity
- **Error: Project not found**: Create a project in the Credible Layer dApp first
- **Error: Assertion not found**: Ensure the assertion name is correct and exists in your project

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

### Development Setup

```bash
# Install dependencies
cargo build

# Run tests
make test

# Check formatting
make format-check

# Run linter
make lint
```
