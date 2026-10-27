# Remote collections

A **collection** is a Git repository that publishes skills for Beskar
libraries. Track one on each machine that should receive it:

```sh
beskar remote add acme https://github.com/acme/agent-skills --yes
beskar remote sync acme
beskar update --all
```

The library stays locally maintained. A remote only supplies the skills you
track from it; everything else in the library remains yours. Sync is
pull-only. To contribute back, print the clone path, improve the skill in a
branch there, and use Git yourself:

```sh
beskar remote where acme
```

## Collection layout

A collection is library-shaped. The repository root (or the folder named by
`--path`) contains:

```text
agent-skills/            any Git repository, or a folder inside one
├── skills/              required: one directory per skill
│   ├── code-review/
│   │   └── SKILL.md
│   └── tdd/
│       └── SKILL.md
├── profiles/            optional: named setups other machines can enable
│   └── devel.bsk
└── metadata/            optional: requirements between collection skills
    └── code-review.bsk
```

- `skills/` must exist and hold one directory per skill. Directory names
  follow Beskar's name rules: 1–64 lowercase letters, digits, `-` or `_`.
- `profiles/*.bsk` use the ordinary profile format. They import on every
  sync; a name that already exists locally is a conflict, never an
  overwrite.
- `metadata/NAME.bsk` records requirements. A skill and its metadata file
  sync as one unit: either both arrive, and a local metadata addition counts
  as drift.
- Symlinks and special files are rejected, exactly like library skills.
  `.git` inside a clone is fine because the collection root sits above it.

The `template/collection/` directory in this repository is a starter you can
copy, commit, and push.

## What sync does

`beskar remote sync NAME` (or `--all`) fetches the reference the remote
follows, moves the clone to it, and compares three fingerprints per tracked
skill: what the remote last synced, the library copy now, and the collection
now. The states are the ones workspace updates use, recast one level up:

| State | Meaning | Sync action |
| --- | --- | --- |
| clean | library matches the collection | nothing |
| new in the collection | tracked skill missing locally | import |
| collection changed | upstream moved, library untouched | update |
| library drift | you edited the library copy | conflict policy |
| diverged | both sides changed | conflict policy |
| removed upstream | collection dropped the skill | remove, if unreferenced |

Drift follows the familiar `--conflict keep|replace|abort` policies, or an
interactive keep/replace/diff/abort prompt. Keeping drift retains its
baseline, so it is reported again next sync. Removals refuse while a local
profile references the skill, another skill requires it, or — for profiles —
a workspace enables it.

Skills the collection offers but you have not tracked are listed as
untracked; `beskar remote track NAME SKILL` starts following one, and
`beskar remote untrack` stops, leaving the library copy locally maintained.
Tracking refuses a name the library already owns, so ownership is never
inferred from equal bytes.

## Onboarding a team

1. Copy `template/collection/`, add your skills under `skills/`, and push.
2. Everyone runs `beskar remote add your-name URL --yes` once.
3. Skill improvements land as pull requests to the collection; every machine
   picks them up with `beskar remote sync --all && beskar update --all`.

Personal multi-machine sharing works the same way: publish your curated
skills in a collection repository of your own and track it everywhere. The
registry, remotes book, and clones are machine-local and never sync; only
the collection content travels through Git.
