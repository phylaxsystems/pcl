# Credible CLI

The Credible CLI is a command-line interface for the Credible Layer.

## Installation

### Build from Source

It requires the following:

- `just`
- `Rust >= 1.86 nightly`
- `git`

After you have installed the above, you can build the CLI by running the following:

```bash
git clone https://github.com/credible-layer/credible-cli.git
cd pcl
just build-all
```

This will build the CLI and install it in the `target/release` directory.

### Usage

```bash
The Credible CLI for the Credible Layer

Usage: pcl [OPTIONS] <COMMAND>

Commands:
  phorge  
  build   
  help    Print this message or the help of the given subcommand(s)

Options:
  -d, --assertions-dir <ASSERTIONS_DIR>  [env: PCL_ROOT=]
  -h, --help                             Print help
  -V, --version                          Print version
```

To execute `phorge`, a minimal fork of Forge which includes a cheatcode for assertion execution, you can run:

```bash
pcl phorge --help
```

You need to specify the assertions directory, which is the directory containing the assertions source and tests you want to test or build.

```bash
pcl --assertions-dir mock-protocol/assertions phorge
```

Phorge expects the following directory structure:

```text
  assertions/
    src/
    test/
```

Assertion source files should be in the `src` directory, and test files should be in the `test` directory. Test files should have the `.t.sol` extension, and test functions should start with `test_`. As a minimal fork of Forge, it behaves identically to Forge.

By definining the assertions directory, the CLI will automatically add the `src` and `test` directories to the phorge command. That way, you can share a `foundry.toml` and `lib` directory between the sources and tests of the smart contracts and the assertions.
