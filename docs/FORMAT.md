# The .bsk format

Beskar files are UTF-8 text with a required version record. A small grammar makes them easy to edit and parse without a YAML or TOML dependency. The extension is `.bsk`.

```text
beskar 1
# Each subsequent line is one record.
```

## Syntax

- Each record occupies one line. LF and CRLF line endings are accepted.
- Spaces and tabs separate values. Leading and trailing whitespace do not matter.
- Blank lines are ignored. `#` starts a comment anywhere outside a quoted value.
- Bare values contain only ASCII letters, digits, `-`, `_`, `.`, `/`, and `:`.
- Double quotes allow spaces, Unicode, and literal `#`. Quoted and bare values have the same meaning.
- The only escapes are `\\`, `\"`, `\n`, `\r`, and `\t`. Other escapes and literal control characters are errors.
- A closing quote must be followed by whitespace, a comment, or the end of the line.
- There are no single quotes, multiline values, continuations, lists, environment substitutions, or includes.
- A keyword has a fixed number of values. Unknown keywords, extra values, and duplicate records are errors.
- The first non-comment record must be `beskar 1`. Other versions are rejected.

For example:

```text
library "/home/alex/skill library" # A path with a space
registry "C:\\Users\\Alex\\beskar\\registry.bsk"
```

Paths are literal. Beskar does not expand `~`, `$HOME`, or Windows environment variables inside a file. State and workspace paths must be absolute and normalized for the current operating system. Unix and Windows examples cannot be used interchangeably.

Names use 1 to 64 characters matching `[a-z0-9][a-z0-9_-]*`. The same rule applies to skills and profiles. Names cannot contain path separators or traversal components.

## Configuration

`BESKAR_HOME/config.bsk` contains exactly one of each of these records:

```text
beskar 1
library "/home/alex/.beskar/library"
registry "/home/alex/.beskar/registry.bsk"
agent-skills ".agents/skills"
```

| Record | Meaning |
| --- | --- |
| `library PATH` | Absolute directory containing `skills/` and `profiles/`. |
| `registry PATH` | Absolute machine-local state file, outside the library. |
| `agent-skills PATH` | Relative deployment directory within every workspace. No `.` or `..` components. |

The default home is `$HOME/.beskar`, falling back to `$USERPROFILE/.beskar` if `HOME` is absent. `--home PATH` takes precedence over `BESKAR_HOME`. Configure alternative state paths during `init`, or edit this file while Beskar is idle.

The library cannot live within `.agents/skills`, `.claude/skills`, or `.codex/skills`, or directly at `.agents`, `.claude`, or `.codex`, where its own `skills/` directory would be discoverable. Deployment destinations cannot overlap the library, registry, Beskar home, or another deployment. Symlinked paths are rejected.

## Profiles

Each profile is `LIBRARY/profiles/NAME.bsk`. Its filename supplies its identity. No redundant name field is needed.

```text
beskar 1
# Tools for everyday code changes.
skill code-review
skill testing
skill documentation
```

`skill NAME` is the only allowed record. An empty profile is valid. Duplicate skills are rejected within one profile; overlapping skills across enabled profiles collapse into one installation. Record order does not affect the effective skill set. Missing library skills are reported when resolving a profile or running `doctor`.

CLI profile edits preserve existing comments and record order. Adding a skill appends it if missing. Removing a skill removes its record and retains any inline comment as a comment line.

## Registry

The registry groups records in explicit `repo` / `end` blocks. Indentation is optional and does not create nesting.

```text
beskar 1

repo "/home/alex/projects/api"
  destination ".agents/skills"
  profile coding
  profile research
  installed code-review 8a3f4146fa569398b3a6e2f4e3642319874991266482cdf923827aec14339754
  synced 1790700000
end

repo "/home/alex/projects/docs"
  profile research
end
```

The example fingerprint illustrates the field shape; Beskar calculates real fingerprints when deploying.

| Record | Meaning |
| --- | --- |
| `repo PATH` | Start a workspace block. Path must be absolute. |
| `destination PATH` | Relative path where the recorded skills were deployed. Beskar requires it when reconciling tracked installations. |
| `profile NAME` | Enable a portable library profile in this workspace. |
| `installed NAME HASH` | Record the SHA-256 fingerprint of the last installed content. |
| `synced SECONDS` | Optional Unix timestamp for the last reconciliation. |
| `end` | Close the workspace block. |

`destination`, `installed`, and `synced` describe what Beskar deployed. Users and agents should edit desired `profile` records or use the CLI, and leave deployment records to Beskar. Editing a baseline can conceal local drift. A changed configured destination blocks reconciliation of tracked skills.

Repository blocks cannot nest. Each path appears once. Profile and installed-skill names are unique within a block. There is at most one destination and timestamp per block. Fingerprints contain exactly 64 lowercase hexadecimal characters. Unix seconds are unsigned decimal integers.

Registry writes sort paths, profile names, and skill names. They regenerate comments and formatting. Profiles and skill content remain portable; registry paths remain machine-local.

## Errors

```text
beskar: /home/alex/.beskar/library/profiles/coding.bsk:line 4: duplicate skill
```

An invalid configuration, registry, or selected profile blocks the operation. Beskar does not guess what a misspelled record means. A dry run applies the same validation and returns a failure status when it finds conflicts.

## Transaction journal

`BESKAR_HOME/transaction.bsk` is temporary internal state, rather than configuration to edit. It records one `change TARGET STAGING_ROOT OLD_HASH NEW_HASH` per replacement. A `-` hash means absence. `STAGING_ROOT/old` holds the original; `STAGING_ROOT/new` holds a staged replacement until it is installed.

`mkdir PATH` records a missing destination parent directory before Beskar creates it. Rollback removes these directories from deepest to shallowest if they are still empty. Nonempty directories are preserved; completed transactions retain their destination directories.

Only `doctor --recover` reads this journal for recovery. Other commands refuse to proceed while it exists. Recovery validates all entries and preserves post-interruption edits.
