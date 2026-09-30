# Context map

- [bsk](crates/bsk/CONTEXT.md) defines the language of editable records.
- [Beskar core](crates/beskar-core/CONTEXT.md) defines skill management and deployment.
- [Beskar CLI](crates/beskar-cli/CONTEXT.md) defines commands and their presentation.

The CLI calls core operations. Core interprets bsk records as configuration, profiles, and registry state. The format has no knowledge of skills. [Architecture](docs/ARCHITECTURE.md) describes the implementation and its trade-offs.
