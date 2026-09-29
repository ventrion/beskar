# Beskar depends on nothing but the Rust standard library

No crate outside the workspace, in any of the three crates. The project asked for as few dependencies as possible, and a tool that copies files and hashes them does not need many. The price is code that a library would have supplied: argument parsing (`crates/beskar-cli/src/args.rs`), SHA-256 (`crates/beskar-core/src/sha256.rs`, checked against the NIST vectors and `sha256sum`), timestamps, a JSON writer and a text diff.

`std::hash::DefaultHasher` is not an option for fingerprints: it is 64 bits wide and its algorithm may change between Rust releases, while fingerprints are stored on disk and compared across runs.

Relying on the standard library also ties the minimum Rust version to it. Kernel file locks (`File::try_lock`, see ADR 0004) need Rust 1.89, which the workspace declares as `rust-version`.

Adding a dependency later needs a new ADR that supersedes this one.
