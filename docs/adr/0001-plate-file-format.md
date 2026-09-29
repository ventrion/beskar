# Beskar files use Plate, a purpose-built format

Beskar's files are hand-edited by people and by coding agents, and Beskar is
built without third-party dependencies. We use Plate
([spec](../../crates/plate/SPEC.md)) rather than YAML, TOML or JSON. Every
line means exactly one thing on its own, values are verbatim strings with no
quoting or type inference, and an edit made by a program keeps the human's
comments. The whole parser is a few hundred lines we own.

## Considered Options

- **YAML**: implicit typing and significant indentation are the two failure modes we most want to avoid, and a parser is a large dependency.
- **TOML**: close, but string quoting is mandatory, arrays of tables are awkward for `[repo <path>]` records, and a correct parser is a dependency.
- **JSON**: no comments, and hand edits break it.

## Consequences

Values cannot contain line breaks, surrounding whitespace, or be wholly
quoted. Nothing Beskar stores needs any of these. A value that looks quoted is
rejected, so a TOML habit gives an error instead of corrupted data.
