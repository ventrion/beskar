# Files that are not part of a skill stay where they are

Fingerprints and copies skip ignored names: version control directories, caches such as `__pycache__` and `node_modules`, litter such as `.DS_Store`, and the user's own `ignore:` patterns. Otherwise running a skill's scripts once would mark it as locally changed. Skipping them must not mean deleting them, though: a `.env` or a clone's `.git` can matter more than the skill. So replacing a copy moves its ignored entries into the new copy, a workspace copy that is its own checkout is never replaced or deleted, and a copy is deleted only when everything ignored in it is a known cache or litter. Otherwise the update reports the files and leaves the copy for the user to clear.

## Considered options

- **Deleting ignored entries with the copy**: simplest, and it silently destroys secrets and repository history that Beskar never had a copy of.
- **Counting ignored entries in the fingerprint**: every cache write would become a local change and turn routine updates into conflicts.
