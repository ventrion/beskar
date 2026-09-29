# Beskar lines

Beskar lines is the file format behind every Beskar file: the config, the profiles in the library, and the registry. Files end in `.bsk`.

One fact per line. A line is a key, a space, and a value.

```
# a profile
description Everyday software engineering
skill code-review
skill git
```

That is most of the format. The rest is fine print.

## Rules

1. A file is UTF-8 text. Lines end in LF or CRLF. A leading byte order mark is ignored.
2. A blank line does nothing.
3. A line whose first non-blank character is `#` is a comment. Comments sit on their own line. A `#` anywhere else is data, so `skill c#-review` means what it says.
4. Every other line is an entry: a key, then optionally a value.
   - The key runs up to the first space or tab. It starts with a lowercase ASCII letter and continues with lowercase letters, digits and `-`.
   - The value is everything after the spaces and tabs that follow the key, minus trailing spaces and tabs. It is taken literally. No quotes, no escapes, no `:` and no `=`.
   - A value stays on its line and holds no control characters except tab.
5. An entry that starts with a space or tab is a child of the closest unindented entry above it. There is one level of nesting. Indenting deeper does not nest deeper, and the amount of indentation is irrelevant. A child with no parent is an error.
6. Order is kept. Repeating a key is how you write a list. Each file kind decides whether a repeated key is allowed.
7. Values have no types. `true`, `01`, `no` and `1.10` are the text you see. Each file kind decides what its values mean.

Blank lines and comments never end a group of children, so you can space things out however you like.

## What is left out, on purpose

- No quoting. If a value needs quotes to survive, the format has failed. Paths with spaces work as they are: `library /home/ana/my skills`.
- No trailing comments. A `#` inside a value is part of the value.
- No multi-line values, no deeper nesting, no type inference. Each of these is a place where YAML and TOML files silently mean something other than what they look like.

## Errors

A parse error names the line, shows it, and says what to change. Unknown keys get a suggestion.

```
profiles/coding.bsk:3: unknown key `skils`, did you mean `skill`?
   3 | skils git
```

Writing `description: text` in YAML style is the most common slip, so the error for it says to drop the colon.

## Editing by hand or by program

Every rule is line-local, so one added or removed line is one changed fact. A shell append works:

```
echo "skill pdf" >> profiles/research.bsk
```

Beskar edits your profiles through a lossless editor. Comments, blank lines, key alignment and the order of entries survive an edit, and a file that Beskar has not changed is written back byte for byte. The registry is different. Beskar owns it and regenerates it from its own state, so comments you add there are lost the next time it changes.

## Grammar

```
file     = *line
line     = blank / comment / entry
blank    = *WSP EOL
comment  = *WSP "#" *CHAR EOL
entry    = [indent] key [gap value] *WSP EOL
indent   = 1*WSP
key      = %x61-7A *( %x61-7A / DIGIT / "-" )
gap      = 1*WSP
value    = 1*CHAR            ; no trailing WSP
WSP      = SP / HTAB
CHAR     = any Unicode scalar except control characters other than HTAB
EOL      = LF / CRLF
```
