# The bsk file format

bsk is the file format Beskar uses for its configuration, profiles and registry. It has three rules. Every line stands alone. Every value is a string. Nothing is quoted or escaped.

A bsk file is meant to be read and edited by people and by coding agents. Both can write a correct file after seeing one example.

## Example

```bsk
# Everyday software engineering.
description Skills for writing, reviewing and testing code

skill code-review
skill git
skill testing
```

## The four kinds of line

Each line is exactly one of these. Whitespace (spaces and tabs) at the start or end of a line is ignored.

| Line | Looks like | Meaning |
| --- | --- | --- |
| Blank | nothing, or only whitespace | Ignored. |
| Comment | `# any text` | Ignored. The `#` must be the first character after any indentation. |
| Section header | `[kind name]` | Starts a section. Entries below it belong to it. |
| Entry | `key value` | One fact. |

There is nothing else. There are no continuation lines, no nesting, no quoting and no escape sequences.

## Entries

An entry is a key, whitespace, and a value.

```bsk
description Skills for writing, reviewing and testing code
on-conflict keep
```

The key is a word: a letter, then letters, digits, `-` or `_`. Keys are case sensitive and by convention lowercase.

The value is the rest of the line after the whitespace that follows the key, with trailing whitespace removed. It can contain anything, including `#`, `:`, `=`, quotes, brackets and spaces. Nothing in it is special.

```bsk
description C# and F# tooling, see "docs" [draft]
library ~/my skills/library
```

A key with nothing after it has the empty string as its value.

### Repeating a key makes a list

A key that appears on several lines holds a list of values, in file order. The program reading the file says which keys may repeat.

```bsk
skill code-review
skill git
skill testing
```

Adding an item is adding a line. Removing an item is deleting a line. Two people who each add a line in different places merge cleanly in version control.

## Sections

A section header starts a group of entries.

```bsk
[repo /home/me/projects/api]
profile coding
profile backend
```

The header is a kind (a word, like a key) followed by an optional name. The name is the rest of the header up to the final `]`, with surrounding whitespace removed. It can contain spaces and even `]`. Only whitespace may follow the final `]`.

```bsk
[repo /home/me/my projects/api]
```

Entries that come before the first header belong to the top level. A section runs until the next header or the end of the file.

## Comments

A comment is a whole line. A `#` anywhere else is part of the value.

```bsk
# This is a comment.
skill git # This is not a comment. The value is "git # This is not a comment."
```

This is deliberate. It is the only way to keep values free of escaping. To annotate an entry, put the comment on the line above it. Comments directly above a line, with no blank line between, are attached to that line.

## Indentation and spacing

Indentation and the amount of space between key and value never change the meaning of a line. Use them to line things up.

```bsk
[repo /home/me/api]
  profile  coding
  synced   2026-09-29T10:15:00Z
```

## Text encoding

A file is UTF-8. It may begin with a byte order mark. Each line ends with LF or CRLF, and one file may use both. The last line may have no line ending. Every line keeps its own ending when a program reads and writes the file.

Control characters are errors. That means the C0 controls except tab (U+0000 to U+001F), DEL (U+007F) and the C1 controls (U+0080 to U+009F). A carriage return that is not part of CRLF is a C0 control, so it is an error too. Tab is allowed.

## Everything is a string

bsk has no numbers, booleans, dates or null. `true`, `1.10`, `no` and `2026-09-29` are all plain text. The program reading the file decides what a value means, so `no` is never silently turned into `false`.

## What a value cannot be

A value cannot contain a line break, and it cannot start or end with whitespace, because a reader could not tell the whitespace from the formatting around it. Programs that write bsk must refuse such values. They do not escape them.

## Grammar

```abnf
file          = [BOM] *(line EOL) [nonempty-line]
line          = blank / comment / header / entry
nonempty-line = 1*blankchar / comment / header / entry
blank         = *blankchar
comment       = *blankchar "#" *textchar
header        = *blankchar "[" *blankchar kind [1*blankchar name] *blankchar "]" *blankchar
entry         = *blankchar key [1*blankchar value] *blankchar

key           = ALPHA *(ALPHA / DIGIT / "-" / "_")
kind          = key
name          = textchar *textchar        ; no blankchar at either end
value         = textchar *textchar        ; no blankchar at either end

BOM           = %xFEFF
blankchar     = SP / HTAB
textchar      = %x20-7E / %xA0-10FFFF / HTAB    ; no control characters except tab
EOL           = LF / CRLF
```

A line break at the end of a file ends the last line and does not start another, so `a` followed by a line break is one line, not two.

The grammar is unambiguous. The first non-blank character of a line decides its kind: `#` is a comment, `[` is a header, anything else is an entry. No line depends on any other line.

## Schemas

A parser only checks syntax. A program that reads bsk also declares which keys and sections it accepts and whether each key may repeat. A file that breaks those rules is rejected. An unknown key is always an error, so a typo such as `skil` is caught instead of ignored.

Errors name the line and column and, where possible, the fix:

```text
error: unknown key 'skil'
 --> coding.bsk:4:1
  |
4 | skil git
  | ^^^^
  = hint: did you mean 'skill'?
```

## Common mistakes

| Written | Problem | Write instead |
| --- | --- | --- |
| `skill: git` | bsk has no colon after the key | `skill git` |
| `skill = git` | bsk has no equals sign; the value would be `= git` | `skill git` |
| `- git` | bsk has no bullet lists | `skill git` |
| `skill "git"` | quotes are part of the value | `skill git` |
| `skills [a, b]` | bsk has no inline lists | one `skill` line per item |
| `skill git # my tool` | the comment is part of the value | put the comment on its own line |
| `[repo /x] # my repo` | text after the closing `]` is an error | put the comment on its own line |
| `[repo]` | the section needs a name | `[repo /path/to/repo]` |

## For programs that edit bsk files

A program that changes a bsk file should change only the lines it means to change. The `bsk` crate does this. It keeps every line's original text and edits entries in place.

* A comment directly above a line, with no blank line between, belongs to that line. Edits never delete a comment, and they never put a new line between a comment and the line it belongs to. Removing an entry leaves its comment in place.
* Blank lines, indentation and column alignment belong to the person who wrote them. A new line copies the indentation of its neighbours. It lines its value up with the others only when the file already shows alignment, meaning at least two entries with keys of different lengths have their values in the same column. Otherwise it puts one space between key and value.
* Every line keeps its own line ending. A new line uses the ending of the first line in the file that has one, or LF if none has.

Programs that write a file from scratch should write keys in a stable order so that version control diffs stay small.
