# JSON output

Every `beskar` command takes `--json`, anywhere before a `--` argument. With it, the command prints exactly one JSON document on standard output, and nothing else. It never asks a question: conflicts follow `--on-conflict` (where `ask` stops, like `abort`), and confirmations need `--yes`. Notices, such as waiting for another beskar process, still go to standard error for a person watching, and they are also listed in the document.

This is the interface for scripts and coding agents. Field names are stable. New fields may be added, so ignore the ones you do not know. Paths are absolute. Fingerprints are 64 lowercase hexadecimal digits.

## The document

```json
{
  "ok": false,
  "command": "repo update",
  "exit": 3,
  "data": { },
  "error": { },
  "notices": [ ]
}
```

| Field | Meaning |
|---|---|
| `ok` | `true` exactly when `exit` is 0 |
| `command` | the command that ran, such as `repo update`; `null` when the command line was not understood |
| `exit` | the process exit status: 0 success, 1 error, 2 usage error, 3 a decision is needed |
| `data` | the command's result, whenever it produced one; present on success and also, for example, for an update that stopped at a conflict |
| `error` | why the command failed as a whole; absent on success |
| `notices` | present when something happened along the way: a wait for the lock, or a leftover of an interrupted run that was cleaned up |

### Errors

```json
{
  "kind": "invalid",
  "message": "unknown key `skils`",
  "hints": ["did you mean `skill`?"],
  "file": "/home/me/.beskar/library/profiles/coding.bsk",
  "line": 4,
  "column": 1
}
```

`kind` is one of `usage`, `not_initialized`, `not_found`, `already_exists`, `invalid`, `conflict`, `locked` or `io`. `hints` are next steps, most useful first, often a command to run. `file`, `line` and `column` appear for a mistake in one of Beskar's own files.

### Notices

```json
{"notice": "waiting", "holder": "pid 4242, since 2026-09-30T10:15:03Z, command update"}
{"notice": "recovered", "action": "put_back", "leftover": "…/.agents/skills/.beskar/old-git-4242-0", "target": "…/.agents/skills/git", "reason": null}
```

`action` is `put_back` (an interrupted replacement was undone), `deleted` (a leftover copy was removed) or `kept` (the leftover holds files that could not go back; `reason` says which).

## Plans

`repo status`, `repo update`, `repo enable` and the other profile changes, and `registry status` describe a workspace with one step per skill that is wanted, recorded or present:

```json
{
  "skill": "code-review",
  "action": "conflict",
  "conflict": "diverged",
  "profiles": ["coding"],
  "library": "3f9a…",
  "recorded": "8d1e…",
  "kept": null,
  "present": "51b7…",
  "blocked": null,
  "stays": null
}
```

| Field | Meaning |
|---|---|
| `action` | `install`, `restore` (installed before, deleted here since), `update`, `remove`, `release` (unwanted, but it holds files that are not part of the skill, so it stays and is no longer managed), `forget`, `record`, `unchanged`, `keep_local` (changed here only), `conflict`, `missing_source` (wanted, not in the library) or `unmanaged` |
| `conflict` | for `conflict`: `diverged` (changed here and in the library), `untracked` (a directory Beskar does not manage is where a wanted skill goes) or `orphaned` (changed here and no longer wanted); otherwise `null` |
| `profiles` | enabled profiles that want the skill |
| `library`, `recorded`, `kept`, `present` | the library version, the recorded base, a library version the person chose not to take, and the workspace copy; `null` where there is none |
| `blocked` | `{"reason", "error"}` when the step cannot go ahead because the copy is its own checkout |
| `stays` | for `release`, what in the copy is not part of the skill |

A plan also lists `ignored_dirs`: directories in the skills directory whose names are not skill names.

## Commands

### `update`, `repo update`, `registry update`

```json
{
  "dry_run": false,
  "policy": "keep",
  "repos": [
    {
      "repo": "/home/me/code/api",
      "result": "applied",
      "steps": [ ],
      "outcomes": [
        {"skill": "git", "done": "updated"},
        {"skill": "pdf", "error": {"kind": "conflict", "message": "…", "hints": []}}
      ],
      "ignored_dirs": []
    }
  ],
  "summary": {"updated": 1, "up_to_date": 0, "stopped": 0, "failed": 0}
}
```

`result` is `up_to_date`, `planned` (a dry run), `stopped` (conflicts needed a decision; nothing changed), `applied`, `failed` (applied, but a skill failed, is blocked or is missing from the library) or `error` (the workspace could not be planned; see its `error`). When the command updates a single workspace and it cannot be planned, that is also the document's top-level `error`. In a dry run, conflict steps carry `would`: `keep`, `replace` or `stop`, and the summary counts `to_update` instead of `updated`. `done` is one of `installed`, `restored`, `updated`, `removed`, `forgotten`, `recorded`, `kept_local`, `released`, `replaced` or `promoted`.

### `status`, `repo status`

`{"repos": [...]}`, each a workspace (see `repo list`) with `plan` (steps and `ignored_dirs`) and `leftovers` of interrupted runs, or `{"path", "error"}`.

### `repo list`, `registry list`

An array of workspaces:

```json
{
  "path": "/home/me/code/api",
  "exists": true,
  "profiles": ["coding"],
  "skills_dir": ".agents/skills",
  "installed": [{"skill": "git", "base": "8d1e…", "kept": null}],
  "synced": "2026-09-30T10:15:03Z"
}
```

`registry list --profile coding` gives `{"profile", "workspaces": [paths]}`; `registry list --skill git` gives `{"skill", "uses": [{"repo", "profiles", "installed"}]}`.

### `registry status`

`{"repos": [...]}`: each workspace with `state` (`up_to_date`, `pending`, `needs_attention`, `gone` or `error`), `counts` (`to_install`, `to_update`, `to_remove`, `to_release`, `conflicts`, `missing`, `changed_here`, `blocked`) and `plan`.

### `registry stats`

`{"repositories", "profiles", "library_skills", "installed_skills", "unused_skills": [...], "unprofiled_skills": [...], "unused_profiles": [...]}`.

### `registry prune`

`{"dry_run", "forgotten": [paths]}`.

### `repo add`

`{"path", "new", "inside", "existing_skills", "profiles"}`: `new` is `false` if it was registered already, `inside` is the registered workspace around it or `null`.

### `repo remove`

`{"path", "exists", "unregistered", "left_in_place", "purge"}`, where `purge` is an update (see above) for `--purge` and `null` otherwise.

### `repo enable`, `repo disable`, `repo toggle`

`{"repo", "enabled", "disabled", "already_enabled", "not_enabled", "pending"}`, where `pending` is the plan an update would now carry out, or `{"error"}`.

### `repo diff`

An array, one entry per skill: `{"skill", "comparison"}` with `comparison` one of `differs` (with `files`), `same`, `link` (with `target`), `not_installed` (with `in_library`, `wanted`) or `not_in_library` (with `copy`). Each file is `{"path", "change", "mode_changed", "hunks"}`, `change` being `added`, `removed`, `modified`, `mode_changed` or `type_changed`, and each hunk `{"old_start", "old_len", "new_start", "new_len", "lines"}` with lines prefixed `' '`, `'-'` or `'+'`. `hunks` is `null` for binary files.

### `repo promote`

`{"repo", "skill", "imported", "wanted", "other_workspaces"}`; `imported` is `added`, `replaced` or `unchanged`.

### `repo restore`

`{"repo", "skill", "done"}`; `done` is `recorded`, `installed`, `restored` or `replaced`.

### `library list`

An array of skills: `{"skill", "path", "description", "metadata", "problems"}`, where `metadata` holds the other front matter fields of `SKILL.md` (`tags`, `category`, `version`, …) as text.

### `library show`

A skill as above plus `fingerprint`, `files`, `profiles` and `uses`.

### `library add`

`{"skill", "source", "imported", "problems", "installed_in"}`.

### `library scan`

`{"root", "replace", "candidates": [{"path", "skill", "naming", "status", "duplicate_of", "description"}], "importable": [names], "imported"}`, where `status` is `new`, `identical`, `differs`, `duplicate` or `unnamed`, and `imported` is `null` for a dry run or `{"added", "replaced", "unchanged", "failed"}`.

### `library remove`

`{"skill", "removed", "profiles", "installed_in"}`.

### `profile list`

An array: `{"profile", "path", "description", "skills", "workspaces"}`, or `{"profile", "error", "workspaces"}` for a profile file that does not load.

### `profile show`

A profile plus `missing` (its skills the library lacks) and `enabled_in` (workspace paths).

### `profile create`

`{"profile", "path", "description", "skills"}`.

### `profile add`, `profile remove`

`{"profile", "added" | "removed", "already_there" | "not_there", "enabled_in"}`.

### `profile delete`

`{"profile", "deleted", "disabled_in"}`.

### `init`, `library init`

`{"home", "config", "config_created", "registry", "registry_created", "library", "library_state", "switched_from", "skills", "profiles"}`.

### `doctor`

`{"sections": [{"title", "checks": [{"level", "message", "hints", "error"}]}], "errors", "warnings"}`; `level` is `ok`, `note`, `warning` or `error`.

### `help`, `--version`

`{"text"}` and `{"version"}`.
