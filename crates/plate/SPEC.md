# Plate 1

Plate is a line-oriented configuration format for files that people and
coding agents both read and edit. It has five kinds of line, one data type
and no escape sequences. You can explain it in a single screen, and you can
tell what any line means without looking at the lines around it.

```text
# Everything I want when writing code.
description = Everyday software engineering

skills:
  - code-review
  - git
  - testing

[repo /home/me/projects/api]
profiles:
  - coding
synced = 2026-09-29T11:27:00Z
```

## Why another format

Beskar needs a handful of keys, lists of names, and repeated records keyed
by a path. The usual candidates each cost something:

| | Problem for this use |
|---|---|
| YAML | Implicit typing (`no` → false, `1.10` → 1.1), significant indentation, many ways to write a string. Parsers are large. |
| TOML | Quoting required for strings, `[[array.of.tables]]`, several string syntaxes. A correct parser is a real dependency. |
| JSON | No comments, trailing-comma errors, noisy to hand-edit, merges badly. |
| INI | No standard: every dialect treats lists, quotes and comments differently. |

Plate keeps the readable part of all of them (`key = value`, `- item`,
`[section]`, `# comment`) and drops the features that make them ambiguous.

## Lines

A document is UTF-8 text. A leading byte-order mark is ignored, and lines
may end in LF or CRLF. Each line is classified by its first character after
leading spaces and tabs:

| Line | Meaning |
|---|---|
| *(empty)* | Blank. Ignored. |
| `# text` | Comment. Ignored. Comments always take a whole line. |
| `[kind label]` | Section header. Starts a new section. |
| `- value` | List item. Appends `value` to the list opened above it. |
| `key = value` | Scalar entry. |
| `key:` | List entry. Its items follow on the next lines. |

Any other line is an error.

## Grammar

```text
document   = *line
line       = ws ( blank / comment / header / item / scalar / list ) ws EOL
blank      = ""
comment    = "#" *any
header     = "[" ws kind [ 1*wsc label ] ws "]"
item       = "-" 1*wsc value
scalar     = key ws "=" ws [ value ]
list       = key ws ":"
kind       = key
key        = lower *( lower / digit / "-" / "_" )
label      = any text; everything up to the final "]", trimmed
value      = any text; the rest of the line, trimmed
ws         = *wsc
wsc        = SP / HTAB
lower      = %x61-7A
digit      = %x30-39
```

"Trimmed" means that surrounding spaces and tabs are removed. Nothing else
is removed.

## Semantics

* **Values are strings.** There is no quoting, escaping or type inference.
  `enabled = no` is the two-letter string `no`, and `version = 1.10` keeps
  both digits. The application decides what a value means and reports
  errors with the line number.
* **Entries before the first header** belong to the top level. After a
  header, entries belong to that section until the next header.
* **A section** is identified by its kind and its label together.
  `[repo /a]` and `[repo /b]` are two records of kind `repo`. The same
  kind and label may not appear twice.
* **A key** appears at most once per section. A list holds as many values
  as it needs.
* **A list** runs from its `key:` line until the next scalar, list or
  header. Blank and comment lines inside it are allowed. An empty list is
  `key:` followed by no items.
* **Indentation** means nothing. Indent list items for readability if
  you like.

## Deliberate restrictions

These are errors, each reported with a line number and a hint:

* **Quoted values.** `path = "~/x"` is rejected. If quotes were allowed,
  someone used to TOML or YAML would silently get quote characters in their
  data. A value wrapped in one pair of `"…"` or `'…'` (with no other quote
  of that kind inside) is therefore not allowed. Other quotes are fine:
  `say = the "best" one` and `pick = "a" or "b"` are ordinary values.
* **Text after `key:`**, as in `key: value`, which is YAML's scalar syntax.
  The hint suggests `key = value`.
* **Uppercase or dotted keys.** Keys use `a-z 0-9 - _`. Dotted keys would
  suggest nesting that does not exist.
* **`-item` without a space.**
* **Empty list items.**

And one warning, since the line is legal but probably a mistake:

* A value that contains ` #` (for example `dir = x  # default`). The `#`
  and everything after it are part of the value.

## What cannot be written

A value cannot contain a line break, cannot begin or end with whitespace,
and cannot be wrapped in a single pair of quotes. Writers must refuse such values
rather than change them. Plate has no escape hatch for them, and that is
deliberate: in exchange, every value is exactly what is shown.

## Editing

The reference implementation keeps the original lines, each line's own
ending (LF or CRLF, even when a file mixes them) and the byte-order mark.
New lines take the ending most of the file uses. A program can
change one value, or add or remove a list item, and every other line
(comments, blank lines, alignment, ordering) stays byte-for-byte the same.
Since each line carries its own meaning, tools and agents can make these
edits with plain line insertion and deletion, and Git merges them cleanly.
