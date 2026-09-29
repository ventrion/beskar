# Beskar records, version 1

Beskar uses a small, line-oriented format for configuration, profiles, and the registry. The `.bsk` extension identifies these files. The format is intentionally limited to records the application needs.

## Grammar

1. Documents are UTF-8. Both LF and CRLF line endings are accepted. A final newline is optional.
2. Leading and trailing whitespace on each line is ignored. Blank lines are ignored.
3. A line whose first non-whitespace character is `#` is a comment. Inline comments do not exist.
4. The first nonblank, noncomment line must be exactly `beskar 1`. A byte-order mark is not accepted.
5. Each subsequent record contains a key followed by one or more ASCII spaces and its value. `end` has no value. Indentation is cosmetic. A tab does not separate a key from its value.
6. Values are literal. There are no quotes, escapes, substitutions, multiline values, includes, inheritance, implicit types, or aliases.
7. A document's schema defines its allowed keys and value grammar. Unknown keys, missing required values, duplicate set members, duplicate scalar keys, and invalid nesting are errors.

A path is the whole value after its key. An `installed` record has two explicitly defined fields separated by one ASCII space. Skill names cannot contain spaces, so that record has exactly one interpretation. `yes`, `no`, and `null` are ordinary valid names.

```text
# This is a comment.
library /data/My skills #1
```

The library path above is `/data/My skills #1`. Adding quote marks would add literal quote marks to the path; quotation syntax is not supported.

Paths must be UTF-8, contain no control characters, and have no meaningful leading or trailing whitespace. Persisted absolute paths may not contain parent traversal. `agent-skills` must have at least two normal relative components, such as `.agents/skills`; roots and parent traversal are forbidden.

Names contain 1 to 64 ASCII lowercase letters, digits, hyphens, or underscores and start with a letter or digit. Windows device names such as `con`, `nul`, `com1`, and `lpt1` are reserved. These constraints make names safe single directory components across platforms.

## Configuration

`BESKAR_HOME/config.bsk` contains exactly one of each record:

```text
beskar 1
library /home/ada/skills-library
registry /home/ada/.beskar/registry.bsk
agent-skills .agents/skills
```

| Record | Value |
| --- | --- |
| `library` | Absolute path to the portable library |
| `registry` | Absolute path to the machine-local registry file, outside the library |
| `agent-skills` | Relative destination within each workspace |

The state directory comes from `--home`, then `BESKAR_HOME`, then `$HOME/.beskar`, with `USERPROFILE` as a home-directory fallback. Tilde and environment expansion inside `.bsk` files are not supported. Shell expansion in CLI arguments happens before Beskar receives them.

To move the library, move its complete directory and edit `library`. Installed workspace copies remain usable. To move the registry, move the file, edit `registry`, and retain any pending recovery files until recovery is complete. State is machine-local; do not put it in the library.

## Profiles

A profile named `coding` lives at `library/profiles/coding.bsk`:

```text
beskar 1

# Apply these together.
skill code-review
skill testing
```

`skill NAME` is the only profile record. Empty profiles are valid. Duplicate members are errors within one profile, while overlapping members across enabled profiles are deduplicated. Names reference directories at `library/skills/NAME`.

Membership commands retain comments and existing record order. Added members append in sorted order. Unknown files without the `.bsk` extension in `profiles/` are ignored, so documentation can live alongside profiles.

## Registry

The registry records desired profiles and the last installed baseline independently:

```text
beskar 1
agent-skills .agents/skills

repo /home/ada/projects/api
  profile coding
  installed code-review e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
  synced 1790640000
end

repo /home/ada/projects/docs
end
```

The checksum above illustrates the field's shape. It is not a valid checksum for any particular skill tree in this example.

| Record | Meaning |
| --- | --- |
| `agent-skills PATH` | Destination associated with tracked copies; outside repo blocks, defaults to `.agents/skills` if omitted |
| `repo PATH` | Start a workspace block using its absolute path |
| `profile NAME` | Desired enabled profile within the current workspace |
| `installed NAME SHA256` | Baseline fingerprint of a managed copy, using 64 lowercase hexadecimal digits |
| `synced SECONDS` | Optional time of the last reconciliation that changed installed state, in Unix seconds |
| `end` | Close the current workspace |

There can be at most one `agent-skills` record. The registry rejects duplicate workspace paths, duplicate enabled profiles, duplicate installed names, and duplicate timestamps. Workspace blocks cannot nest. The final block must have an `end`.

Paths registered by the CLI are canonical absolute paths. Relative aliases and workspace-root symlinks resolve to that identity during registration. Beskar checks registered boundaries on subsequent updates and refuses newly introduced symlinks. Registry writers sort repositories, profiles, and skills. Registry comments and formatting are normalized when the CLI saves state, so keep annotations in profiles or separate documentation.

The installed fingerprint is also the original source fingerprint: an installation is a verified byte copy. Storing both would duplicate the same value. Keeping that baseline when the library changes is what lets Beskar distinguish a source update from a local edit.

## Fingerprints

Beskar computes SHA-256 over a framed directory-tree encoding, not over `SKILL.md` alone. Files are streamed in 64 KiB chunks.

- The stream starts with the bytes `beskar-tree-v1` followed by a zero byte.
- Each directory is visited in depth-first order, with child paths sorted by Rust's platform path ordering.
- Every directory record has a `d` byte and its relative path, including empty directories.
- Every file record has an `f` byte, its relative path, a four-byte big-endian executable permission mask, an eight-byte big-endian file length, and its contents.
- Each path uses `/` separators and is prefixed with its UTF-8 byte length as an eight-byte big-endian integer.
- The executable mask is Unix mode bits `0o111`, or zero on other platforms.
- The root directory itself is not a record. Timestamps, ownership, and other permission bits are not hashed.

The encoding separates paths, record types, lengths, and content so distinct trees cannot collide through ambiguous concatenation. Cross-platform executable permissions and filename ordering can produce different fingerprints; the registry is machine-local.

## Locks and interrupted updates

Beskar holds operating-system locks on `BESKAR_HOME/.lock`, `library/.beskar.lock`, and `<registry-path>.lock`. Locks are released by the operating system when a process exits; the files stay in place. Do not delete lock files to bypass an active process.

An update first plans every selected workspace. It verifies sources and stages copies beside each workspace's skills directory. Each staging directory contains:

```text
.beskar-update-<unique>/
  transaction.bsk
  new/
  old/
```

`transaction.bsk` records `target PATH`, then `skill NAME BEFORE AFTER` lines. `BEFORE` and `AFTER` are tree fingerprints or `-` for absence. `old/` holds original directories moved out of the workspace. `new/` holds replacements until they are moved into place.

A coordinator directory beside the registry contains `registry-before.bsk`, `registry-after.bsk`, and `transactions.bsk`. The pending marker replaces the registry's extension with `.pending.bsk`, for example `registry.pending.bsk`. It lists the coordinator and each workspace transaction directory. Beskar writes that marker before the first installed directory changes.

Ordinary errors trigger rollback, preserving any moved replacement in staging. Successful rollback removes the pending marker and reports the coordinator directory. Interrupted or incomplete rollback retains the marker and prevents further mutations. A committed update removes the marker and then clears backups. Failure to clear a backup is reported as a warning.

### Recovering an interrupted update

Recovery is manual in this version. Keep the pending marker until state is consistent.

1. Finish or stop any Beskar, editor, or synchronization process touching the affected paths. Read the marker to find the coordinator and all transaction directories.
2. Preserve copies of those directories, the current registry, and affected workspace skills before changing anything. Backups can contain local edits from before or during the interruption.
3. If the current registry equals `registry-after.bsk`, the update committed. Check the workspace contents against the intended result before removing the marker. Keep any backups containing edits you need.
4. Otherwise restore the original state. For each skill with an entry in `old/`, move its current workspace copy to a separate recovery directory if it exists, then move the old copy back. For a new installation whose `BEFORE` is `-`, move its workspace copy aside if the staged `new/` copy has already been consumed. If neither move happened, leave that workspace skill alone. Never discard a current or backup copy just to make paths match.
5. Restore `registry-before.bsk` to the configured registry location after restoring the original workspace state. Retain the staged replacements separately until you have reviewed them.
6. Remove the pending marker, run `beskar doctor`, and inspect `beskar status --all`. Pending library changes and preserved local edits may still appear. Resolve them using the normal conflict policies.

Temporary import or update directories left before the marker was written never replaced installed files. Retain them until you have inspected their contents; they are outside agent skill directories.
