# Beskar

Beskar copies selected agent skills from a global library into local workspaces. Profiles name sets of skills. A machine-local registry records which profiles each workspace uses and the fingerprint of every copy Beskar installed.

Beskar is written in Rust and has no third-party dependencies. It works offline and does not need Git.

## Install

```sh
cargo install --path .
beskar init
```

`beskar init` creates `~/.beskar/config.bsk`, `~/.beskar/registry.bsk`, and `~/.beskar/library/{skills,profiles}`. Set `BESKAR_HOME` to use another state directory. You can also set the library and registry paths when initializing:

```sh
beskar init --library /path/to/my-library --registry /path/to/local-registry.bsk
```

Keep the registry outside the library. The library can be version controlled or copied between machines; the registry contains paths specific to one machine.

## Use

```sh
beskar library add ~/Downloads/skills/code-review
beskar library scan ~/Downloads/skills --yes

beskar profile create coding
beskar profile add coding code-review

cd ~/projects/my-app
beskar repo add .
beskar repo enable coding
beskar repo update --dry-run
beskar repo update
```

`repo enable` and `repo disable` change the registry's desired profiles. `repo update` is the step that copies or removes files in `.agents/skills/`. If two profiles select the same skill, Beskar installs one copy.

To propagate a library change to all registered workspaces:

```sh
beskar registry update --all --dry-run
beskar registry update --all
```

`beskar repo status`, `beskar registry status`, `beskar registry stats`, `beskar registry where profile coding`, and `beskar registry where skill code-review` show what is selected and where it is installed. `beskar doctor` checks the configuration, library, profile references, repository paths, and pending or conflicting changes.

## The `.bsk` format

Beskar uses a small line-based format so it can parse configuration with only Rust's standard library. Each line contains one `key = value`. Blank lines and lines beginning with `#` are ignored. Values can be bare words or quoted strings; paths with spaces or `#` should be quoted. Quoted strings allow `\\`, `\"`, and `\n`. Repeated keys are allowed only where the format says so. Unknown keys and duplicate single-value keys are errors.

Configuration has three keys:

```text
library = "/home/alex/my skills library"
registry = "/home/alex/.beskar/registry.bsk"
skills_dir = ".agents/skills"
```

The library has `skills/<name>/` directories and `profiles/<name>.bsk` files. A profile is one skill per line:

```text
# library/profiles/coding.bsk
skill = "code-review"
skill = "testing"
```

The registry uses sections, one per absolute workspace path:

```text
[repo "/home/alex/projects/my app"]
profile = "coding"
installed = "code-review" e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
```

Beskar writes the full 64-digit fingerprint. The value above illustrates the field's syntax. You normally edit the config or profile files and let Beskar maintain the registry.

## Local changes

Beskar fingerprints each installed tree, including file contents, paths, empty directories, and executable bits on Unix. An update stops before writing anything if it would overwrite or remove a locally edited skill. It also stops if an unmanaged directory already occupies a selected skill name.

You can resolve a conflict explicitly:

```sh
beskar repo update --on-conflict keep
beskar repo update --on-conflict replace
```

`keep` leaves conflicted workspace copies in place and updates other skills. The conflict remains visible on the next status or update. `replace` discards the conflicting workspace copy and installs the library copy, or removes it if no active profile selects it. Review the dry run before using `replace`. The same policies work with `registry update --all`.

Beskar rejects symlinks and special files inside skill trees. This keeps copies self-contained and prevents a skill from reaching outside its directory during import or update. It does not change directories in `.agents/skills/` that are neither selected nor tracked in the registry.
