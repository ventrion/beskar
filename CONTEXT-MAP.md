# Context map

## Contexts

- [bsk](./crates/bsk/CONTEXT.md): the file format Beskar uses for its settings, profiles and registry
- [Beskar core](./crates/beskar-core/CONTEXT.md): the library, profiles, registry and reconciliation
- [Beskar CLI](./crates/beskar-cli/CONTEXT.md): the `beskar` command line

## Relationships

- **Beskar core → bsk**: core reads and writes its files as bsk documents and checks them with bsk schemas. bsk knows nothing about skills.
- **Beskar CLI → Beskar core**: the CLI asks core to do the work and renders the structured results. Core never prints or prompts. When a decision needs a person, core asks a `Resolver` that the CLI supplies.
