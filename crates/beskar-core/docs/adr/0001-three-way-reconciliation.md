# Reconciliation compares three fingerprints, and local-only changes are kept

For each installed skill the registry stores one fingerprint: the library version the workspace copy is based on (the recorded base). An update compares it with the current library version and the current workspace copy, the way a merge compares two sides with their common ancestor. If only the library changed, the copy is replaced. If only the workspace copy changed, it is left alone, so a repository can diverge on purpose without every update complaining. Only when both changed, or when an unwanted skill has local changes, is there a conflict, settled by keep, replace or promote.

Keeping a local copy records the current library version as its new base, so Beskar asks again only when the library changes again. A workspace directory that already matches the library is adopted without copying, which also lets an interrupted update heal itself on the next run.

## Considered options

- **Two fingerprints per skill** (source and installed, as the brief sketches): in a copy-based design they are always equal at install time, so the second adds nothing.
- **Treating every local change as a conflict**: safe, but it forces a decision on each update for repositories that diverge deliberately.
