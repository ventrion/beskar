# Context map

## Contexts

- [Core](./crates/beskar-core/CONTEXT.md): the library, profiles, repositories, the registry, and the reconciliation that keeps a repository's skills in line with its profiles
- [Lines](./crates/beskar-lines/CONTEXT.md): the file format every Beskar file is written in

## Relationships

- **Core → Lines**: Core writes and reads its config, profile and registry files as Lines documents. Lines knows nothing about skills.
- **CLI → Core**: `beskar-cli` has no vocabulary of its own. It parses arguments, asks questions and formats what Core returns, so it has no context of its own.
