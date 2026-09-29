# The bsk format

bsk is the configuration format beskar uses for its config file, profile
files, and registry. It has no dependencies and no type inference. The
complete parser and writer come to under 400 lines of Rust, tests
excluded.

Design goals, in order:

1. **Unambiguous.** One grammar, no context. A byte sequence parses the
   same way everywhere, so tooling never guesses.
2. **Human friendly.** Newlines are structure. No brackets to balance and
   no indentation rules to trip over.
3. **Machine friendly.** Trivial to lex, trivial to emit, and diffs stay
   small because entries sit on single lines.
4. **Lossless round trips.** Comments and blank lines survive a parse and
   rewrite cycle byte for byte. beskar edits your files without deleting
   your notes.

## The whole format

An entry is a line of words. A block opens with `{` at the end of an
entry's line and closes with a `}` alone on its line. `#` starts a
comment that runs to the end of the line. Blank lines separate things.
That is the entire format.

```bsk
# config.bsk
version 1
library-path ~/.beskar/library

registry-path ~/.beskar/registry.bsk   # machine-local, do not sync
agent-skills-dir .agents/skills

# What wins when a workspace skill was modified locally:
conflict-policy ask
```

Nested data uses blocks instead of punctuation soup:

```bsk
repo /home/u/work/api {
  enabled-profile backend
  enabled-profile coding

  installed code-review {
    source-fingerprint sha256:7ca56a3dcc24f8b7ca435fc4985298e5bda40d3d092524f77317fc0eb8a0133c
    installed-fingerprint sha256:7ca56a3dcc24f8b7ca435fc4985298e5bda40d3d092524f77317fc0eb8a0133c
    installed-at 2026-09-29T11:28:25Z
    status clean
  }
}
```

## Words

Every value is a word: a bare word or a quoted string.

A **bare word** may contain only these characters:

```
A-Z a-z 0-9 . _ / : = , + @ ^ - ~
```

Paths, fingerprints, ISO timestamps, and identifiers all fit in bare
words, so they need no quotes:

```bsk
library-path ~/.beskar/library
installed-at 2026-09-29T11:28:25Z
```

A **quoted string** handles everything else. It uses double quotes and
knows five escapes: `\"`, `\\`, `\n`, `\t`, `\r`. Anything with spaces,
a `#`, a `{` or `}`, or an empty string must be quoted:

```bsk
description "Everyday coding skills"
greeting "line one\nline two"
```

Quoting a word that does not need it is allowed and means exactly the
same thing. The writer emits bare whenever it can, so files stay clean.

There are no numbers, booleans, nulls, arrays, or objects. A word is
text; the consumer decides what it means. `version 1` is the word `1`.
This removes an entire class of parser surprises: `0755` never becomes
493, `yes` never becomes true, and leading zeros survive.

## Blocks and nesting

An entry whose line ends with `{` opens a block. The block's children
are entries at one deeper level. A line containing only `}` (plus
optional whitespace and a trailing comment) closes the block. Depth is
unbounded in the grammar; beskar uses two levels at most.

```bsk
profile work {
  skill code-review      # ends with a comment, still fine
}
```

The `{` must be the last token on the line. A `}` that is not alone on
its line is an error, and so is a `{` glued to other characters. Both
fail with a file name and line number instead of being parsed into
something you did not mean.

## Comments and blank lines

`#` comments out the rest of the line, whether the line holds only a
comment or follows an entry.

The parser keeps the full layout. Every entry remembers the comments
above it, the blank lines above those comments, the blank lines between
the comments and the entry, and any comment on the entry's own line.
Rewriting a document reproduces all of it. beskar relies on this: when
you run `beskar profile add coding rust-help`, your hand-written comments
in `coding.bsk` stay exactly where you put them.

## Grammar

```
document  := (blank | comment | entry)*
entry     := word+ [ "{" ] comment? newline
             entry*
             "}" comment? newline
word      := bare | quoted
bare      := [A-Za-z0-9._/:=,+@^-~]+
quoted    := '"' ( escape | char-not-quote-or-backslash )* '"'
escape    := [\\] ["\\ntr]
comment   := "#" ...to end of line
```

That fits on one screen, which is the point.

## Why not TOML, JSON, or YAML

JSON has no comments, which alone disqualifies it for files humans edit.
YAML is a large language with ambiguous corners (Norway, the sexagesimal
accident, the two-spaces problem) and no parser is ever fully sure it
implements all of it. TOML is genuinely good, but deep nesting means
either repeated table headers or arrays of tables, and diffs across
those get noisy. beskar also wanted one format for three different
files, and a parser small enough to audit in one sitting.

bsk trades generality for those properties. It is not a serialization
format for arbitrary data, and it does not try to be. If you need
TOML inside your skill files, nothing stops you; beskar only manages
directories.
