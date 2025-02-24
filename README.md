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

### Usage

```bash
The Credible CLI for the Credible Layer

Usage: pcl [OPTIONS] <COMMAND>

Commands:
  phorge
  build
  store   Submit the Assertion bytecode and source code to be stored by the Assertion DA of the Credible Layer
  submit  Submit assertions to the Credible Layer dApp
  auth    Authenticate the CLI with your Credible Layer dApp account
  help    Print this message or the help of the given subcommand(s)

Options:
  --root <ROOT_DIR>              The root directory for the project. Defaults to the current directory [env: PCL_ROOT_DIR=]
  --assertions <ASSERTIONS_DIR>  The directory containing assertions 'src' and 'test' directories. Defaults to '/assertions' in the root directory [env: PCL_ASSERTIONS_DIR=]
  -h, --help                         Print help
  -V, --version                      Print version
```

To execute `phorge`, a minimal fork of Forge which includes a cheatcode for assertion execution, you can run:

```bash
pcl phorge --help
```

Phorge expects to be ran from the root directory, with the following directory structure:

```text
root-dir/
  assertions-dir/
    src/
    test/
```

Assertion source files should be in the `src` directory, and test files should be in the `test` directory. Test files should have the `.t.sol` extension, and test functions should start with `test_`. As a minimal fork of Forge, it behaves identically to Forge.

By definining the assertions directory, the CLI will automatically add the `src` and `test` directories to the phorge command. That way, you can share a `foundry.toml` and `lib` directory between the sources and tests of the smart contracts and the assertions.
