# Beskar files use their own one-fact-per-line format, not YAML, TOML or JSON

Beskar's config, profiles and registry are written in Beskar lines: `key value` on one line, one level of indentation for children, `#` comment lines, no quoting and no types. The full rules are in `crates/beskar-lines/SPEC.md`.

The brief asked for as few dependencies as possible, and for files that people and coding agents can both edit and read without surprises. A YAML or TOML crate would be the workspace's first dependency and would come with a large surface. Writing a subset parser instead would leave files that look like YAML or TOML and mean something different, which is worse for an agent than a format that plainly is not either.

Beskar lines gives up nesting depth, multi-line values and typed values. None of the three file kinds needs them. In return:

- The grammar has twelve productions, and the parser and lossless editor together are about 400 lines of Rust with no dependencies.
- One fact per line means one added or removed line is one changed fact. `echo "skill pdf" >> profiles/research.bsk` is a valid edit, and two people editing different skills in a Git-managed library rarely conflict.
- Beskar edits profiles through a lossless editor, so `beskar profile add` never reflows a person's comments.
- There is little to mistype. Values are never reinterpreted (`no` stays `no`), indentation carries no meaning beyond "child or not", and the errors for the likely slips (a `:` after a key, `- ` bullets, `[sections]`) say what to write instead.

## Considered options

- **YAML.** The brief's examples use it, but the parsing rules are the ambiguity we wanted to avoid, and no small subset is safe.
- **TOML.** Well specified and readable, but a correct parser is large, and profiles with one skill per line are clumsier as arrays.
- **JSON.** Unambiguous, but no comments and easy to break with a trailing comma, which hurts both audiences.

## Consequences

A tool that wants to read Beskar files has to parse them itself. A reader is short in any language: split each line at the first space or tab, treat indented lines as children of the entry above, skip lines starting with `#`. The spec is one page, and `beskar help format` prints it.
