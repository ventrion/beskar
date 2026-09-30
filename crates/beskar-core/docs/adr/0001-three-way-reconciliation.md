# Reconciliation compares three fingerprints, and local-only changes are kept

For each installed skill the registry stores the fingerprint of the library version the workspace copy is based on (the recorded base). An update compares it with the current library version and the current workspace copy, the way a merge compares two sides with their common ancestor. If only the library changed, the copy is replaced. If only the workspace copy changed, it is left alone, so a repository can diverge on purpose without every update complaining. Only when both changed, or when an unwanted skill has local changes, is there a conflict, settled by keep, replace or promote.

Keeping a local copy records the library version the person declined, as a `kept:` line in the registry, next to the base, which stays as it was. Beskar asks again only when the library moves past the declined version. Because the base still says what the copy came from, promoting the copy later knows the library changed in between and needs `--force`, and undoing the local change turns the copy back into its base, which the next update simply replaces. A directory Beskar never installed that the person keeps has a declined version and no base, so any later library change is a conflict again rather than a silent replacement.

A workspace directory that already matches the library is adopted without copying, which also lets an interrupted update heal itself on the next run.

## Considered options

- **Two fingerprints per skill** (source and installed, as the brief sketches): in a copy-based design they are always equal at install time, so the second adds nothing.
- **Treating every local change as a conflict**: safe, but it forces a decision on each update for repositories that diverge deliberately.
- **Recording the declined library version as the new base**, the first design. One fingerprint fewer, but the base then no longer says what the copy came from. A promote after keeping silently overwrote the library changes the person had declined, and a copy whose local change was undone stayed at its old version, shown as changed, until the library moved again.
- **Not remembering keep at all.** The same conflict comes back on every update, and without a terminal every update of that workspace stops.
