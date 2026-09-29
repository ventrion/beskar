# Context map

## Contexts

- [BSK notation](./crates/bsk/CONTEXT.md): the line format of Beskar's files; parsing, lossless edits and diagnostics, with no knowledge of what the files mean
- [Skill management](./crates/beskar-core/CONTEXT.md): the library, profiles, the registry and reconciliation of workspaces
- Command line (`crates/beskar`): the `beskar` binary; it turns commands into skill-management operations and their results into terminal output, and uses the skill-management language

## Relationships

- **Skill management → BSK notation**: the config, profiles and registry are BSK documents. Skill management owns the schema of each file and turns BSK diagnostics into its own errors.
- **Command line → Skill management**: the command line calls core operations and renders what they return. Conflicts are decided in the command line (a prompt or a conflict policy) and handed to the core as resolutions; the core never prints or prompts.
