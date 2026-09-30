# Beskar

Better Skill Arrangement. Keep one curated library of agent skills, group them into profiles, and give each workspace copies of exactly the skills its profiles name, in `.agents/skills/`. Beskar tracks what it installed where, updates every copy when you improve a skill, and never overwrites a local change without asking.

```text
LIBRARY      "What skills do I own?"                  ~/.beskar/library/skills/<name>/
PROFILE      "Which skills belong together?"          ~/.beskar/library/profiles/<name>.bsk
REGISTRY     "Where should those profiles be active?" ~/.beskar/registry.bsk
WORKSPACE    "Materialize exactly those skills here." <workspace>/.agents/skills/<name>/
```

Beskar is a single binary with no dependencies outside the Rust standard library. It works offline and needs no Git, although a library kept in Git works well.

## Install

```sh
cargo install --path crates/beskar
```

This puts `beskar` in `~/.cargo/bin`. Beskar keeps its files in `~/.beskar`; set `BESKAR_HOME` to move them.

## Quick start

```sh
beskar init                                   # config, registry and an empty library
beskar library scan ~/my-skills               # find skills (directories with a SKILL.md) and import them
beskar profile create coding git code-review testing
cd ~/projects/api
beskar repo add .                             # register this workspace
beskar repo enable coding                     # say what it should have
beskar repo update                            # copy the skills into .agents/skills/
```

```text
~/projects/api
  +  code-review
  +  git
  +  testing
```

Later, improve `code-review` in the library and bring every workspace that uses it up to date:

```sh
beskar update --all --dry-run                 # see what would change
beskar update --all
```

Only workspaces whose profiles include `code-review` change.

## Concepts

- **Library.** Your curated skills (`skills/<name>/`) and profiles (`profiles/<name>.bsk`). It is the source every copy comes from and holds nothing specific to one machine, so it can be a Git repository you sync. Agents never read it directly.
- **Skill.** Any directory. A `SKILL.md` with `name` and `description` front matter is what agents expect, and Beskar reads it for descriptions, but it manages the directory as a whole. Skill names use lowercase letters, digits and hyphens, as in the Agent Skills format.
- **Profile.** A named set of skills for a use case. A workspace can enable several; it gets the union, and a skill in two profiles is installed once.
- **Workspace.** Any directory you register, Git repository or not. The CLI calls it a repo. Its skills directory (`.agents/skills` unless the config says otherwise) holds real copies, so the workspace works on its own, travels with the repository, and keeps working if the library moves.
- **Registry.** The machine-local record of workspaces, their enabled profiles, and what Beskar installed in each. It lives outside the library because it holds local paths.

`enable` and `disable` change what a workspace should have. `update` changes the files.

## Updates and local changes

For every installed skill the registry keeps a fingerprint (SHA-256 over paths, contents and executable bits) of the library version the copy came from. An update compares that recorded base with the library and with the copy in the workspace:

| Situation | What `update` does |
|---|---|
| nothing changed | nothing |
| the library changed, the copy did not | replaces the copy |
| the copy changed, the library did not | keeps the copy: the change is local to this workspace |
| both changed | conflict |
| both changed, and you kept your copy over this library version before | keeps the copy |
| no enabled profile includes the skill any more, copy untouched | deletes the copy |
| no enabled profile includes the skill any more, copy changed | conflict |

A skill directory Beskar did not install is left alone, unless a profile wants a skill of that name; then it is adopted if it matches the library and is a conflict otherwise.

Conflicts are settled by `--on-conflict` (or `on-conflict:` in the config):

- `ask`: ask for each one, offering keep, replace, promote and a diff. Without a terminal this behaves like `abort`, unless you pass `--on-conflict ask` explicitly, which reads the answers from standard input.
- `keep`: keep the local copy. Beskar remembers the library version you declined and asks again only when the library changes again. The copy's recorded base stays, so promoting it later over the newer library version needs `--force`, and undoing your change lets the next update bring the library version in.
- `replace`: take the library version and discard the local change.
- `abort`: change nothing in that workspace and exit with status 3.

Questions are asked before Beskar takes its lock, so a person thinking about a conflict does not hold up other processes. Once the answers are in, Beskar plans the workspace again and applies an answer only if the skill is still in exactly the state it was asked about.

Three commands deal with local changes by hand:

```sh
beskar repo diff code-review       # unified diff, library → workspace
beskar repo promote code-review    # make this copy the library version, then `beskar update --all`
beskar repo restore code-review    # discard local changes
```

Before replacing or deleting a copy, Beskar checks its fingerprint again, and once more after moving it out of the way, so an edit made while an update runs is not lost. New copies are assembled in a `.beskar` work directory inside the skills directory, one level below where agents look for skills, and moved into place in one rename, so agents never see a half-written skill; a deleted skill is renamed away before it is deleted, so it is never half there either. A workspace copy with its own `.git` (a clone of a skill's repository, say) is never replaced or deleted; `beskar status` shows it as blocked and says how to unblock it. A library skill with its own `.git` keeps it when a promotion or `--replace` import overwrites the skill, so the change shows up there as a Git diff, and a skill symlinked into the library is updated where the link points.

Some files are not part of a skill: version control (`.git`, `.hg`, `.svn`), caches (`__pycache__`, `*.pyc`, `node_modules`, `.venv`) and litter (`.DS_Store`, `Thumbs.db`), plus whatever your `ignore:` lines in the config name. They never count as changes and are never copied, but Beskar leaves them where they are. An update moves them into the new copy, and a copy is deleted only when all it holds besides the skill is caches and litter. An unwanted copy that holds more than that stays where it is, and Beskar stops managing it.

If Beskar dies halfway through a change, whether from a crash, `kill -9` or a full disk, the next command that changes that workspace or the library undoes or finishes the interrupted step. A copy that was moved aside goes back, a leftover copy is deleted, and files carried into an unfinished copy return. `beskar doctor` lists anything it could not clean up.

Beskar refuses setups that would let it damage its own files:

- a skills directory that leads into the library or into Beskar's home, for example through a symlinked `.agents`;
- a library inside a directory agents load skills from, such as `~/.claude/skills` or a workspace's `.agents/skills`;
- a registry inside the library;
- two workspaces sharing one skills directory;
- a changed `skills-dir` setting while skills are still installed in the old directory.

## Commands

```text
beskar init [--library PATH]            set up config, registry and library
beskar doctor                           check everything; exit status 1 on errors

beskar library init [PATH]              create a library and use it
beskar library add <PATH>               import one skill (--name, --replace)
beskar library scan <DIR>               find skills and import them (--yes, --dry-run, --replace)
beskar library list | show <SKILL>
beskar library remove <SKILL>           (--yes, --force also removes it from profiles)

beskar profile create <NAME> [SKILL...] (--description TEXT)
beskar profile add <PROFILE> <SKILL>... | remove <PROFILE> <SKILL>...
beskar profile list | show <NAME> | delete <NAME>

beskar repo add [PATH] | remove [PATH]  (remove --purge also deletes installed skills)
beskar repo enable | disable | toggle <PROFILE>...
beskar repo status [--all]
beskar repo update [--all] [--dry-run] [--on-conflict POLICY]
beskar repo diff [SKILL] | promote <SKILL> | restore <SKILL>
beskar repo list

beskar registry list [--profile NAME | --skill NAME]
beskar registry status | stats
beskar registry update [PATH...] | prune

beskar status [--all]                   same as repo status
beskar update [--all]                   same as repo update
```

Repo commands act on the registered workspace around the current directory; `--repo PATH` picks another. Every command takes `--json` and `--no-color`. `beskar help <command>` shows details and examples.

## Files

Beskar's files use BSK, a line format made for this tool. Every line is a `# comment`, a `[section label]` header or a `key: value` entry. Values are verbatim text: no quotes, escapes or type guessing. A list is a repeated key:

```text
# ~/.beskar/library/profiles/coding.bsk
description: Everyday software development
skill: code-review
skill: git
skill: testing
```

When Beskar edits a profile or the config it changes only the affected lines and keeps your comments. Mistakes are reported with file, line, column and a suggested fix. [docs/FORMAT.md](docs/FORMAT.md) is the full specification, and `beskar help format` prints a summary.

## Using Beskar from agents and scripts

- `--json` on any command prints one JSON document on standard output and never asks a question. The document has `ok`, `command`, `exit`, `data`, `error` and `notices`. [docs/JSON.md](docs/JSON.md) describes every command's data, and `beskar help json` summarizes it.

  ```sh
  beskar update --all --on-conflict keep --json | jq '.data.repos[] | {repo, result}'
  ```

- Beskar asks questions only on a terminal. Without one, confirmations need `--yes` and conflicts need `--on-conflict`, and Beskar never waits for input.
- Exit status: 0 success, 1 error, 2 usage error, 3 a decision is needed: a conflict (`--on-conflict`) or a confirmation (`--yes`).
- Errors name the file, line and column and end with `help:` lines stating the next command to run. In JSON these are `error.hints`.
- Output is plain text without color when it does not go to a terminal, or with `NO_COLOR` or `--no-color`. Lists come in a stable, sorted order.
- Several agents can run Beskar at once. Every change takes an operating-system file lock in `~/.beskar`, which the kernel releases even if a process crashes, and reads the registry fresh under it, so no change is lost. `update --all` locks one workspace at a time. A command that finds the lock taken says who holds it and waits up to `lock-timeout` seconds, 60 unless the config or `BESKAR_LOCK_TIMEOUT` says otherwise.
- [skills/beskar](skills/beskar/SKILL.md) is a skill that teaches coding agents how to work in a Beskar-managed workspace. Add it with `beskar library add skills/beskar` and put it in your profiles.

## Development

```sh
cargo test                        # unit tests, randomized safety scenarios, end-to-end tests of the binary
cargo clippy --all-targets -- -D warnings
cargo fmt
```

The workspace has three crates, one per bounded context (see [CONTEXT-MAP.md](CONTEXT-MAP.md)):

- `crates/bsk`: the BSK parser, lossless editor and diagnostics.
- `crates/beskar-core`: library, profiles, registry, fingerprints and reconciliation, and in `ops` one operation per command that owns its checks, locking and registry changes and returns a report. It never prints or prompts. Conflicts go through a `Resolver` the caller provides, and confirmations are previews the caller shows first, so a TUI or an editor integration can reuse every rule.
- `crates/beskar`: the command line. It parses, asks and renders each report as text or JSON.

Decisions with lasting consequences are recorded in [docs/adr](docs/adr) and [crates/beskar-core/docs/adr](crates/beskar-core/docs/adr).
