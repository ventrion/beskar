# Beskar's files use BSK, a line format defined here

Beskar's config, profiles and registry are read and edited by people and by LLM coding agents, and the tool must not pull in dependencies. We defined BSK, a format where every line is a comment, a `[section label]` header or a `key: value` entry, lists are repeated keys, and values are verbatim text typed only by each file's schema. A line-oriented format lets Beskar edit a profile by adding or deleting one line while keeping comments, lets agents and shell tools edit files safely, and needs a parser small enough to write and test in full. The specification is [docs/FORMAT.md](../FORMAT.md).

## Considered options

- **YAML**, the format the brief sketches: implicit typing and significant indentation cause silent misreads, and no dependency-free parser handles real YAML.
- **TOML**: strict and well liked, but a full parser is large, and a partial one would accept a dialect that users could not check against the TOML specification.
- **JSON**: no comments, and too much punctuation to edit by hand.

## Consequences

BSK cannot express nesting beyond one level of sections or values that span lines. Data that needs more structure goes into compound values (the registry's `installed: <skill> <fingerprint>`) or into separate files, not into a richer syntax.
