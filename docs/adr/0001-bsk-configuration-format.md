# Beskar's files use bsk, a format made for them

Settings, profiles and the registry are written in bsk: every line stands alone, every value is a string, and nothing is quoted or escaped. The files are edited by people and by coding agents, and reading them must not need a dependency. `crates/bsk/SPEC.md` is the specification, and `beskar help format` prints it.

## Considered options

- **YAML**, which the brief's examples use. Its parser is large. Values change type without warning (`no`, `1.10`, `on`), and indentation decides structure, which is where hand and agent edits go wrong.
- **TOML**. Unambiguous, but every string is quoted and every list needs brackets and commas, so adding one skill to a profile means editing punctuation. A correct parser is also substantial.
- **JSON**. No comments, and a trailing comma breaks the file.
- **INI or properties**. No lists, and escaping rules that differ between implementations.

## Consequences

Adding an item is adding a line and removing one is deleting a line, so version control merges profiles cleanly. Programs edit files in place and keep the comments, blank lines and alignment a person wrote. The registry is the exception: nobody edits it by hand, so Beskar writes it out whole and only `config.bsk` and the profile files keep a person's comments. Values cannot contain line breaks or start or end with whitespace; Beskar refuses such values (a repository path that ends in a space, say) instead of escaping them. Editors have no bsk highlighting, and no other tool reads the format.
