# Installed skills are copies tracked by one fingerprint, and conflicts need a decision

Skills are installed as plain copies. The registry records, for each installed skill, one fingerprint: the hash of what Beskar wrote. Comparing three hashes (the library's now, the recorded one, and the files on disk) gives every state the brief names, and decides what an update may do.

| Library vs recorded | Disk vs recorded | State | Update does |
| --- | --- | --- | --- |
| same | same | clean | nothing |
| different | same | library changed | overwrites |
| same | different | local drift | leaves it alone |
| different | different | diverged | asks |

**Update leaves drift alone.** The brief says repository skills "can temporarily diverge" and that an update must not destroy local changes. Overwriting drifted skills on every update would turn each one into a prompt, and a deliberate local tweak into a permanent nuisance. So drift alone is reported (`status`, `doctor`) and left in place. A conflict only arises when an update would destroy something: the library also changed, the skill is no longer wanted, or an unmanaged directory is in the way.

**Beskar removes only what it installed.** A directory in the skills directory with no record in the registry is left alone, even when no profile wants it. A repository can hold skills from other tools or written by hand.

**One fingerprint, not two.** The brief's model has a source fingerprint and an installed fingerprint. Installs are byte-exact copies, verified against the library's fingerprint before they replace anything, so the two are equal by construction. If an install ever transforms content, add the second field then.

**Promotion replaces the library's version, so it asks.** The conflict prompt offers `promote` for a diverged skill and for a skill that is no longer wanted. When the library changed since install, choosing it asks a second question, because the library's newer work is what gets replaced. Outside the prompt, `beskar repo promote` refuses in that situation unless `--force` is given. Both re-check that the copy still matches what was reviewed before anything reaches the library.

**Fingerprints cannot see everything, so some things count as modified.** They ignore `.git`, `.DS_Store` and empty directories. A skill directory that holds a `.git` is treated as modified, so deleting or replacing it needs a decision. A directory Beskar cannot read or hash is treated the same way, per skill, so one bad skill does not stop a repository from updating. `.DS_Store` files and empty directories are deleted along with the skill without comment.

**Swaps check twice.** Copying a large skill takes time. Just before the old copy would be lost it is moved aside and fingerprinted again against what the plan saw. If it changed, it is moved back and that skill fails. If moving it back also fails, it stays in `.beskar-staging` and the error names the path, since it may then be the only copy.

**The skills directory has to be a plain part of the repository.** A skills directory that resolves through a symlink to somewhere outside the repository, or into the library, blocks the update. Otherwise removing an "installed" skill could delete library files or another repository's.

**Nothing is guessed without a terminal.** The default policy is `ask`, which prompts on a terminal and aborts elsewhere. An aborted repository is left completely untouched, including the skills that had no conflict, so a run is all or nothing per repository.

## Considered options

- **Symlinks instead of copies.** Ruled out by the brief: repositories should stay self-contained and keep working when the library moves.
- **Overwrite drift on every update.** Simpler, and it fits "update makes the repository match the library". It also makes local edits fragile, which the brief argues against.
- **Keep the installed baseline on disk, to enable three-way merges.** More capable, and not needed to detect drift. It doubles the disk use and adds a place for state to disagree with itself. Revisit if merging becomes a goal.
