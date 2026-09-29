# Context map

Beskar is split into three bounded contexts, one per crate. Each has a
`CONTEXT.md` glossary; read the one for the area you are touching.

| Context | Crate | Owns |
| --- | --- | --- |
| [Slate](crates/slate/CONTEXT.md) | `crates/slate` | The file format: lines, sections, entries, documents |
| [Core](crates/beskar-core/CONTEXT.md) | `crates/beskar-core` | Library, skill, profile, registry, repository, fingerprint, reconciliation |
| [CLI](crates/beskar/CONTEXT.md) | `crates/beskar` | Commands, flags, prompts, markers, exit codes |

Dependencies point one way: `beskar` → `beskar-core` → `slate`. Nothing in
`slate` knows about skills; nothing in `beskar-core` knows about terminals.

The design brief that all three implement is [docs/BRIEF.md](docs/BRIEF.md).
The format rationale is [docs/slate.md](docs/slate.md).
