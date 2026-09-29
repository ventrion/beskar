# CLI: glossary

Presentation only. Everything here turns core data into text and answers into core calls.

- **Group** — the first word after `beskar`: `init`, `doctor`, `library`, `profile`, `repo`, `registry`, `config`, `help`, plus the aliases `status` and `update`.
- **Subcommand** — the second word (`repo update`). Each handler re-parses the full argument list against the flags it accepts, so an unknown flag is a usage error for that group.
- **Flag** — `--long`, `--long value`, `--long=value` or a single short letter (`-y`). Flags do not combine (`-yn` is an error).
- **Current repository** — the registered repository that is the current directory or its closest registered ancestor. Overridden by a positional path or `--repo`.
- **Marker** — the one-character prefix in plan listings: `+` install, `~` update, `-` remove, `=` up to date, `M` modified locally, `!` conflict, `?` missing from library, blank for unmanaged.
- **Dry run** — `--dry-run` / `-n`: print the plan, change nothing, including the registry.
- **Interactive** — both stdin and stdout are terminals. Only then are questions asked; otherwise `--yes` or an explicit `--on-conflict` policy is required and the error says so.
- **Outcome** — how a handler ended: `Ok` (exit 0), `Failed` (exit 1, message already printed), `Usage` (exit 2, help printed). A returned `Error` is exit 1 with `error: ...` on stderr.
- **Help topic** — `beskar help <group>` or `beskar help format` (the Slate rules).

Avoid: colour or emoji in output (it is read by agents as often as by people), and printing paths without `~` abbreviation.
