# BSK: the Beskar file format

BSK is the text format of Beskar's configuration, its profiles and its registry. Every line is one fact, and the first non-blank character of a line tells you what kind of line it is. There is no nesting beyond sections, no quoting and no escaping, so a person, an LLM agent, `grep` and `sed` all read a BSK file the same way. The parser is a few hundred lines of Rust without dependencies.

```text
# Skills for everyday software work.
description: Everyday software development
skill: code-review
skill: git
skill: testing
```

## Lines

Each line is exactly one of these:

| First non-blank character | Line | Example |
|---|---|---|
| none | blank | |
| `#` | comment | `# anything at all` |
| `[` | section header | `[repo /home/me/code/api]` |
| `a` to `z` | entry | `skill: git` |
| anything else | error | `- git`, `Skill: git`, `"skill": "git"` |

Nothing else exists. A file is a list of lines, and each line can be understood without looking at the lines around it, except for knowing which section it is in.

## Entries

An entry is a key, a colon and a value: `key: value`.

- A key starts with a lowercase ASCII letter and continues with lowercase letters, digits and `-`. `skills-dir` is a key; `skills_dir`, `skillsDir` and `Skills` are not.
- The separator is the first `:` after the key. Spaces around it are optional: `key:value`, `key: value` and `key  :  value` are the same entry.
- The value is the rest of the line with leading and trailing spaces and tabs removed. It is taken verbatim. Quotes are part of the value, `#` is part of the value, and a second `:` is part of the value:

  ```text
  description: C# helpers # for .NET
  synced: 2026-09-29T10:15:03Z
  ```

  The first value is `C# helpers # for .NET`; the second is `2026-09-29T10:15:03Z`.
- A value can be empty (`key:`). Whether that is allowed depends on the key.
- A value fits on one line. There are no multi-line values.

## Lists

A key that appears several times in the same block forms a list, in file order:

```text
skill: code-review
skill: git
```

Adding an item means adding a line, and removing one means deleting a line. No brackets, commas or indentation are involved, so list edits never touch neighbouring lines and merge cleanly in version control. Which keys may repeat is up to the file's schema; a key that takes one value is an error when repeated.

## Sections

A section header is a line `[name]` or `[name label]`. The name follows the rules for keys. The label is everything between the name and the `]` that ends the line, trimmed, so it can hold spaces and even `]`:

```text
[repo /home/me/My Projects/site]
profile: coding
```

Entries before the first header belong to the root block. Entries after a header belong to that section, up to the next header. Sections do not nest.

## Whitespace, encoding, line endings

- Indentation is allowed anywhere and means nothing.
- Files are UTF-8. A byte order mark at the start is ignored.
- Lines end with LF or CRLF. The last line may lack a line ending.
- Control characters other than tab are errors.

## Grammar

In [ABNF](https://www.rfc-editor.org/rfc/rfc5234), after removing a byte order mark and splitting the text into lines at LF, with a CR before the LF removed:

```abnf
line     = *ws [ comment / header / entry ] *ws
comment  = "#" *char
header   = "[" *ws name [ 1*ws label ] *ws "]"
entry    = key *ws ":" *ws value
name     = key
key      = lower *( lower / DIGIT / "-" )
label    = 1*char          ; everything up to the line's final "]", trimmed
value    = *char           ; the rest of the line, trimmed
lower    = %x61-7A         ; a-z
ws       = %x20 / %x09     ; space, tab
char     = %x09 / %x20-7E / %xA0-10FFFF   ; no control characters
```

## Types come from the schema

BSK itself has one type: text. Each file's schema decides what a value means and checks it: a path, a name, one of a few words, a timestamp. Because the syntax never guesses, `no` stays the text `no` and `1.10` stays `1.10`; the schema reports a value it cannot use, with the line and column.

Schemas are strict. An unknown key is an error that suggests the closest known key. A single-valued key given twice is an error that points at both lines.

## Beskar's files

### `~/.beskar/config.bsk`

| Key | Values | Default |
|---|---|---|
| `library` | path to the library | `~/.beskar/library` |
| `registry` | path to the registry file | `~/.beskar/registry.bsk` |
| `skills-dir` | where skills go inside each workspace, relative to its root | `.agents/skills` |
| `on-conflict` | `ask`, `keep`, `replace` or `abort` | `ask` |
| `ignore` | a file or directory name pattern, with `*` and `?`, that does not match `SKILL.md`; repeatable | none |

Paths may start with `~/` for the home directory. Relative paths start at the directory holding `config.bsk`.

### `profiles/<name>.bsk` in the library

| Key | Values |
|---|---|
| `description` | one line of text, optional |
| `skill` | a skill name; repeat the key for each skill |

A profile is named after its file, so the file has no `name` key.

### `~/.beskar/registry.bsk`

```text
version: 1

[repo /home/me/code/api]
profile: coding
profile: backend
synced: 2026-09-29T10:15:03Z
installed: code-review 3f9a2c41d0b7e8f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f70819
```

| Key | Where | Values |
|---|---|---|
| `version` | root | `1` |
| `profile` | `[repo <path>]` | an enabled profile; repeatable |
| `synced` | `[repo <path>]` | UTC time of the last completed update |
| `installed` | `[repo <path>]` | a skill name and the fingerprint of the library version its copy is based on; repeatable |

## How Beskar writes BSK

When Beskar changes a profile or the config, it edits only the lines involved and keeps everything else as you wrote it, comments included:

- changing a value keeps the line where it is, with its indentation and spacing;
- a new list item goes after the last line with the same key, in sorted position if those lines are sorted;
- removing an item deletes its line.

The registry is Beskar's own state. Beskar writes it whole, in a fixed order, on every change, so comments added to it do not survive.

## Mistakes

Errors name the file, line and column, show the line, and usually suggest the corrected line. Common habits from other formats get their own messages:

```text
error: use `:` between a key and its value
 --> ~/.beskar/config.bsk:3:9
  |
3 | library = ~/lib
  |         ^
help: write `library: ~/lib`
```

```text
error: BSK has no `-` list items
 --> ~/.beskar/library/profiles/coding.bsk:3:3
  |
3 |   - git
  |   ^
help: write one `key: value` line per item, repeating the key: `<key>: git`
```

`beskar doctor` checks every Beskar file at once.

## Working with BSK from the shell

Every line stands alone, so line-based tools are enough:

```sh
grep -l '^skill: pdf$' ~/.beskar/library/profiles/*.bsk        # profiles that include pdf
echo 'skill: pdf' >> ~/.beskar/library/profiles/research.bsk   # add pdf to research
sed -i '/^skill: pdf$/d' ~/.beskar/library/profiles/research.bsk
```

## Why not YAML, TOML, JSON or INI

YAML guesses types (`no` becomes false, `1.10` becomes `1.1`), gives indentation meaning, and has many ways to write a string. A tool that edits YAML also tends to drop comments and reformat the file.

TOML fixes the guessing, but strings need quotes, a multi-line list needs brackets and commas, and a full parser is a sizeable dependency. A hand-written subset would be a dialect that looks like TOML without being it.

JSON has no comments and punishes a trailing comma.

INI has no specification; parsers disagree about comments, quoting and lists.

BSK keeps what these formats do well for configuration (readable keys, comments, sections) and drops everything that needs a lookahead or a guess. The price is that BSK cannot express nested data or multi-line values. Beskar's files need neither.
