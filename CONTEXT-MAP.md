# Context Map

## Contexts

- [Skill management](./crates/beskar-core/CONTEXT.md): library, profiles, registry and reconciliation of workspaces
- [Plate](./crates/plate/CONTEXT.md): the line-oriented file format every Beskar file is written in

## Relationships

- **Skill management → Plate**: config, profiles, registry and the library marker are Plate documents. Plate knows nothing about Beskar; schemas (allowed keys, what a value means) live in `beskar-core`.
- **CLI → Skill management**: `beskar-cli` presents plans and collects conflict resolutions; all decisions about state live in `beskar-core`.
