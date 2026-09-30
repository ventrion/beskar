# Every operation lives in the core; front ends parse, ask and render

The brief asks that a TUI, a GUI or another integration can sit on top of the same core as the command line. That holds only if the core owns the rules of each operation, not just the domain types: which workspace a command means, what is checked before a change, what is locked while it happens, what the registry records afterwards, and when a workspace stays registered because a purge could not finish. So `beskar_core::ops` has one method on `Beskar` per command, and each returns a report: plain data saying what was found or done. The command line parses arguments, asks its questions and renders the report, as text or, with `--json`, as one JSON document.

Questions that need a person reach the front end in two ways. Conflicts during an update go through a `Resolver`, which the command line implements with a prompt and every front end can implement with a `ConflictPolicy`. Confirmations, such as discarding local changes or deleting a skill or a profile, come back as a preview that the front end shows before calling the operation that acts on it. The operation checks under the lock that nothing changed since the preview.

JSON is a rendering of the reports, like the text, so it lives in the command line crate. It covers every command, including mutations and usage errors, in one envelope with `ok`, `command`, `exit`, `data`, `error` and `notices`. [docs/JSON.md](../JSON.md) documents it.

## Considered options

- **Orchestration in the command line**, the first design. The core returned plans and outcomes, and each command handler took the lock, loaded and saved the registry and applied its own checks. It worked, but every new front end would have had to copy those rules, and JSON output would have needed a second copy of each handler.
- **Serializing reports in the core**: one JSON shape for every front end, but a presentation format in the domain crate, and a JSON writer every other front end has to carry.
