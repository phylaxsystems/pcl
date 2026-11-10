# Temporary ZIP compatibility patches

These sources carry forward the dependency fix from [PCL #133](https://github.com/phylaxsystems/pcl/pull/133). They keep parent-required crate versions while moving every ZIP dependency to a patched release. `Cargo.lock` resolves ZIP 9.3.0; `make zip-policy` rejects versions below 9.0.5 across the entire lockfile.

| Crate | Published source | Runtime changes |
| --- | --- | --- |
| soldeer-core 0.11.0 | [e4aac286](https://github.com/mario-eth/soldeer/tree/e4aac28659531229105c756d8f76c2afad16d924/crates/core) | Replace zip-extract with ZIP's checked extraction, preserving single-root removal; adapt extraction errors. |
| candle-core 0.10.2 | [7c7a8c57](https://github.com/huggingface/candle/tree/7c7a8c570e6a16ea24cc30a7501c8ffbbdb51680/candle-core) | Adapt fallible archive-name APIs and ZIP writer options. |
| tch 0.22.0 | [90bcf3e9](https://github.com/LaurentMazare/tch-rs/tree/90bcf3e9aaab98db7b698fa949e8b4d271b40608) | Adapt archive-name APIs and writer options; propagate invalid-entry errors instead of panicking. |
| torch-sys 0.22.0 | [90bcf3e9](https://github.com/LaurentMazare/tch-rs/tree/90bcf3e9aaab98db7b698fa949e8b4d271b40608/torch-sys) | Adapt fallible archive-name APIs in libtorch extraction. |

Files are from the matching crates.io packages, with trailing whitespace normalized. Upstream example, benchmark and integration-test targets and their development dependencies are omitted. Source and bundled license files are retained; Soldeer's repository-root MIT license and torch-sys's repository-root MIT/Apache licenses are included explicitly. Most of the large diff is unchanged upstream source and generated bindings.

`tch` and `torch-sys` preserve ZIP 0.6's default AES, bzip2, deflate, time and zstd features explicitly. Soldeer retains deflate support, and Candle retains its no-default-features setting. ZIP's additional legacy compression feature is not enabled.

Remove these patches when Phoundry's exact Soldeer pin and Burn's Candle/Tch requirements permit upstream releases on patched ZIP. Regenerate the lockfile with access to the real private dependency graph, verify `cargo metadata --locked`, and run `make ci`. Do not reconstruct private dependency entries by hand.

Extraction regression tests live in `crates/pcl/phoundry/tests/zip_security.rs` and exercise the patched Soldeer API through the workspace test suite.
