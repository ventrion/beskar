# Beskar processes take kernel file locks around every read-modify-write

Two terminals, an editor plugin and a coding agent can run Beskar at the same time. Each command that reads shared state, changes it and writes it back first takes an exclusive lock: on the registry, on `config.bsk`, and on the library for profile and skill edits. It waits up to ten seconds, then fails with a "busy" error that says another Beskar process is changing the same files and suggests trying again. The locks are `File::try_lock` from the standard library, held on `registry.bsk.lock` next to the registry and on files in `<home>/locks/` for the settings and the library. The operating system releases one when its process ends, however it ends.

## Considered options

- **No locks.** Review measured it: twenty parallel `beskar profile add` commands all reported success and the profile kept three to five of the skills. Parallel `config set` and `repo add` lost changes the same way.
- **Lock files that hold a process id, with takeover of stale ones.** This is the usual portable answer. Review found the takeover is not atomic: with sixteen threads, several were inside the critical section at once, and 10 to 20 percent of the rounds failed.

## Consequences

The minimum Rust version is 1.89, where `File::try_lock` became stable, and the workspace declares it. A lock left behind by a crashed process blocks nobody, because the lock lives in the kernel and not in the file. Advisory locks depend on the file system: a library or home folder on a network share that does not honour them gets no protection.
