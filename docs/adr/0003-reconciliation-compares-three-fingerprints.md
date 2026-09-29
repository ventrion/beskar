# Reconciliation compares three fingerprints and touches only what it recognises

For each skill, `reconcile::decide` compares the library's fingerprint, the one recorded at install time and the workspace copy's. Files are overwritten or removed only when the workspace copy equals the recorded fingerprint or the library's current version. Anything else is a conflict, and a conflict needs a decision from a person or from an explicit policy (`fail`, `keep`, `replace`). With no terminal and no policy, Beskar stops before changing anything in that repository. Before each destructive step it checks the fingerprint again, because a person may take minutes to answer a prompt.

Beskar removes only skills it installed. Other folders in the skills folder are never touched.

## Choices beyond the brief

- **An edited copy is a conflict even when the library did not change.** Reconciling means making the folder match the library, so an update would overwrite the edit. The alternative, updating only when the library changed, leaves repositories that quietly disagree with the library and gives `--on-conflict replace` no way to reset one.
- **The registry stores one fingerprint per installed skill**, not a source and an installed fingerprint. Installs are byte-exact copies, so the two are equal by construction. A transformation at install time would have to split them.
- **Interpreter caches and Finder files (`__pycache__`, `*.pyc`, `.DS_Store`) are not part of a skill.** They are not copied and not fingerprinted, because an agent running a script creates them, and counting them would report drift nobody caused.
- **A `.git` folder is left out of the library's copy of a skill and counts in an installed copy.** In the library it is history that belongs to the person keeping the library, so it is not copied out to repositories, and replacing a library skill keeps it. In a repository, a `.git` folder inside an installed skill is somebody's clone or history. An earlier version ignored it in the fingerprint and deleted it along with the folder, which lost an unpushed branch in review. Now its presence alone (its contents are not read) makes the copy a local change, so it is a conflict and only the explicit `replace` policy removes it.
- **A link inside a skill is followed only while it stays inside that skill.** The copy contains the file the link points at. A link that leaves the skill, or points at nothing, is an error that names the link, because following it would copy files such as `~/.ssh/id_rsa` into the library and then into every repository that uses the skill.
