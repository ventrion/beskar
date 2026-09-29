# Beskar file kinds

All three are Beskar lines files. See the format rules above.

## config.bsk

Lives in the Beskar home (`~/.beskar`, `$BESKAR_HOME` or `--home`). Every key is optional and may appear once.

```
library      library         # where skills and profiles live
registry     registry.bsk    # machine-local state
skills-dir   .agents/skills  # install location inside each repository
on-conflict  ask             # ask, abort, keep or replace
```

Paths are absolute, start with `~/`, or are relative to the directory holding the config. The registry must stay out of the library, and the library must not sit inside a `skills-dir`.

## profiles/NAME.bsk

One file per profile in the library. The file name is the profile name. Keys:

```
description Everyday software engineering    # optional, once
skill code-review                            # repeat for each skill
skill git
```

Each `skill` names a directory in the library's `skills/`. Repeating a skill is harmless. Comments and blank lines are free, and Beskar keeps them when it edits the file.

## registry.bsk

Written by Beskar, kept outside the library. Each `repo` line holds an absolute path and owns the indented lines under it:

```
repo /home/ana/projects/api
    profile coding                      # enabled profile, in the order enabled
    skill code-review fp1:9f2c...       # installed skill and its fingerprint
    synced 2026-09-29T10:00:00Z         # last update without failures
```

The fingerprint is a hash of the skill directory as Beskar installed it. Comparing it with the library and with the files on disk is how Beskar tells a library change from a local edit.
