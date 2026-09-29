# Slate

Slate is the file format Beskar uses for its configuration, profiles and
registry. It exists because the files are meant to be edited by hand, by
people and by coding agents, and every established format gets in the way of
that in some manner.

## The whole grammar

There are four kinds of line. Nothing spans more than one line.

```text
# a comment            first non-blank character is '#'
[kind name]            a section header; the name is optional and may contain spaces
key = value            an entry; the value is everything after the first '=', trimmed
                       a blank line
```

That is the complete syntax. A parser fits in a screen of code, and a reader
never has to hold state from a previous line to understand the current one.

## Rules

1. **Every line stands alone.** Indentation is ignored. There are no
   continuation lines, no multi-line strings, no nested blocks.
2. **No quotes, no escapes.** The value of `path = /tmp/#weird = yes` is
   `/tmp/#weird = yes`. A `#` only starts a comment at the beginning of a
   line. Leading and trailing whitespace around a value is trimmed; that is
   the only transformation.
3. **Every value is text.** `true`, `007`, `no`, `2026-09-29` are all just
   strings. The program reading the file decides what they mean, and it says
   so in the header comment it writes. The parser never infers a type, so
   there is no "Norway problem" and nothing quietly turns into a number.
4. **A repeated key is a list.** `skill = git` followed by `skill = pdf` is a
   two-item list. Where the schema expects one value, a repeat is an error
   that names the line.
5. **Names are identifiers.** Keys and section kinds use letters, digits,
   `_`, `-` and `.`. Two sections may not share the same kind and name.
6. **Unknown keys are errors.** A typo is reported with its line number
   rather than silently ignored.

## Why not the usual suspects

| Format | What goes wrong for hand-edited, agent-edited files |
| --- | --- |
| YAML | Indentation-sensitive, type inference (`no` is `false`), a spec so large that few parsers agree. Needs a heavy dependency. |
| TOML | Good, but quoting rules, dotted keys, inline tables, arrays of tables and date types make a hand-written parser large and give agents plenty of ways to write something valid-looking that fails. |
| JSON | Unambiguous but no comments, trailing-comma traps, and every string quoted. Not pleasant to edit. |
| INI | Close, but no spec: comment characters, list handling, quoting and duplicates all vary. |

Slate keeps the readable part of INI and TOML (`[section]`, `key = value`,
`#` comments) and removes every rule that needs a second look.

## Why this suits agents

Coding agents edit files by emitting lines. Slate is designed so that the
smallest useful edit is one line and no edit can break a neighbouring line:

* Adding a skill to a profile is appending `skill = name`.
* Removing it is deleting that line.
* Enabling a profile in a repository is one `profile = name` line under the
  right `[repo ...]` header.
* Because indentation carries no meaning, an editor that re-indents or
  strips whitespace cannot change the meaning.
* Because there are no escapes, a path or a description can be pasted as-is.
* Every file Beskar writes starts with a comment block that states which
  keys it understands and what the values mean, so an agent opening the
  file has the schema in front of it.
* Errors always name the line and list the allowed keys.

## Editing without losing anything

Beskar keeps the lines it read and edits the document surgically. Comments,
blank lines and ordering survive. `beskar profile add coding testing` on this
file:

```text
# my notes
description = Hand written

skill = git
# pdf is handy too
skill = pdf
```

produces:

```text
# my notes
description = Hand written

skill = git
# pdf is handy too
skill = pdf
skill = testing
```

The registry is the one file Beskar rewrites wholesale, because it is
bookkeeping rather than something people author.

## Files Beskar keeps in Slate

| File | Contents |
| --- | --- |
| `<home>/config.slate` | `library`, `registry`, `skills_dir`, `on_conflict` |
| `<library>/library.slate` | `format = 1` marker |
| `<library>/profiles/<name>.slate` | `description = ...` and one `skill = <name>` per skill |
| `<home>/registry.slate` | one `[repo <path>]` section per repository with `profile = ...`, `installed = <skill> <sha256>` and `synced = <utc>` |

Example registry:

```text
[repo /home/me/projects/api]
profile = coding
profile = backend
synced = 2026-09-29T11:47:11Z
installed = code-review 668ccaa02d7c5ea1b087effbcf358d29bb74a2fbe71be43813673e9ab2e57608
installed = git 9957ec71dacab98c3f46bc7810370802501462525b967cd3c3bc1c562ea7575e
```

`beskar help format` prints a condensed version of this document.

## Implementation

The `slate` crate (`crates/slate`) has no dependencies. It exposes a
`Document` that parses text, answers typed queries (`get` for a single value
with duplicate detection, `get_all` for a list, `check_keys` for schema
validation) and supports in-place edits (`set`, `push`, `remove`,
`ensure_section`, `remove_section`) that preserve the surrounding lines.
