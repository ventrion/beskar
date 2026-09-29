# Slate: glossary

The configuration file format. Everything here is about text, not about skills.

- **Document** — the parsed form of one file: an ordered list of lines. Keeps every line it read so edits preserve comments and layout.
- **Line** — one of four kinds: *blank*, *comment* (`# ...`), *section header* (`[kind name]`), *entry* (`key = value`). Nothing spans lines.
- **Section** — a header line plus the entries that follow it up to the next header. Identified by *kind* and *name*; the pair is unique within a document.
- **Root** — the entries before the first section header. Config and profile files use only the root.
- **Kind** — the first word inside a section header, an identifier (`repo`). Says what the section describes.
- **Name** — the rest of the header after the kind, optional, may contain spaces (`/home/me/my project`). Says which one.
- **Entry** — `key = value`. The key is an identifier; the value is the trimmed text after the first `=`. Always a string.
- **List** — a key repeated within one section; each occurrence is one item. Not a syntax, a reading of the file.
- **Schema check** — `check_keys` / `check_section_kinds`: rejecting keys or kinds a consumer does not know, with the line number.
- **Surgical edit** — `set`, `push`, `remove`, `ensure_section`, `remove_section`: change one line, leave every other line where it was.
- **Header** — the comment block a producer writes at the top of a file to explain its keys. Convention, not syntax.

Avoid: "table" (TOML vocabulary), "object"/"mapping" (JSON/YAML), "quoted string" (there are none).
