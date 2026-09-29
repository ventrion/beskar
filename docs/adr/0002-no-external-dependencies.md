# No crates outside this workspace

The project's owner asked for as few dependencies as possible, preferably none. Beskar depends only on the Rust standard library: argument parsing, SHA-256, the line diff, UTC timestamps, SKILL.md front matter reading and the BSK parser are all written here, and `unsafe` code is forbidden workspace-wide. Each of these is small, specified by a public standard or by this repository, and covered by tests (SHA-256 against the NIST vectors), so owning them costs less than auditing and updating a dependency tree. Adding a crate needs a reason that outweighs this.
