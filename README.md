# PCL — The Credible CLI

The CLI for the [Credible Layer](https://phylax.systems).

## Installation

```bash
brew install phylax/pcl/phylax
```

## Commands

| Command | Description |
|---------|-------------|
| `pcl build` | Build assertion contracts |
| `pcl apply` | Preview and apply declarative deployment changes |
| `pcl auth` | Authenticate with the Credible Layer platform |
| `pcl config` | Manage CLI configuration |
| `pcl download` | Download assertion source code for a protocol |
| `pcl test` | Run assertion tests |
| `pcl verify` | Verify assertions locally before deployment |

## Development

```bash
# Build
cargo build --workspace

# Run tests
cargo test --workspace

# Lint
cargo clippy --workspace --all-targets

# Regenerate API client from latest OpenAPI spec
make regenerate
```

## License

BSL 1.1 — see [LICENSE](LICENSE) for details.
