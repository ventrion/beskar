# Install Rohrpost on Windows

Use this playbook on Windows — in PowerShell, cmd, or Git Bash — when the
Rohrpost skill reports that a wrapper is missing or its installation is
incomplete. It is the Windows branch of `install-local.md`: ask the user for
permission before installing anything and stop if the user declines. The
wrappers are validators and launchers; they never repair an installation.

`rp.exe` is a single static executable: no Python, no `uv`, no virtual
environment, nothing to activate.

## Location

Use `ROHRPOST_HOME` when it is already set. Otherwise set nothing: the `.ps1`
and `.cmd` wrappers default the install home to `%LOCALAPPDATA%\rohrpost` at
runtime. If you choose a custom location, keep `ROHRPOST_HOME` set for the
current shell. The Git Bash wrapper embeds the resolved path when it is
materialised, so later calls stay usable when the variable is not restored.
The binary lives at `<home>\bin\rp.exe`.

## Provision

Preferred: the prebuilt release for `x86_64-pc-windows-msvc`, published at
`https://github.com/code-factorio/rohrpost/releases` as
`rp-<tag>-x86_64-pc-windows-msvc.zip`.

```powershell
$rohrpostHome = if ($env:ROHRPOST_HOME) { $env:ROHRPOST_HOME } else { Join-Path $env:LOCALAPPDATA 'rohrpost' }
New-Item -ItemType Directory -Force -Path (Join-Path $rohrpostHome 'bin') | Out-Null
$tag = (Invoke-RestMethod https://api.github.com/repos/code-factorio/rohrpost/releases/latest).tag_name
$zip = Join-Path $env:TEMP "rp-$tag.zip"
Invoke-WebRequest "https://github.com/code-factorio/rohrpost/releases/download/$tag/rp-$tag-x86_64-pc-windows-msvc.zip" -OutFile $zip
Expand-Archive $zip -DestinationPath $env:TEMP -Force
Copy-Item (Join-Path $env:TEMP "rp-$tag-x86_64-pc-windows-msvc\rp.exe") (Join-Path $rohrpostHome 'bin\rp.exe') -Force
```

Record the tag that was installed. Fallback when a Rust toolchain (1.89+) is
available and the release cannot be downloaded:

```powershell
cargo install --git https://github.com/code-factorio/rohrpost --locked --root $rohrpostHome
```

`cargo install --root` places the binary at `<home>\bin\rp.exe`. If neither
route is available, stop and report the missing prerequisite.

## Materialise the wrappers

From the repository containing this skill:

- `scripts/rohrpost.ps1.template` → copy to `scripts/rohrpost.ps1` (PowerShell)
- `scripts/rohrpost.cmd.template` → copy to `scripts/rohrpost.cmd` (cmd)
- `scripts/rohrpost.template` → `scripts/rohrpost` for Git Bash, with the
  `__ROHRPOST_HOME__` placeholder replaced by the shell-quoted POSIX form of
  the install home (e.g. `/c/Users/<you>/AppData/Local/rohrpost`); do this
  substitution from Git Bash exactly as `install-local.md` describes

```powershell
Copy-Item scripts/rohrpost.ps1.template scripts/rohrpost.ps1
Copy-Item scripts/rohrpost.cmd.template scripts/rohrpost.cmd
```

The `.ps1` and `.cmd` templates carry no placeholder — they resolve
`%LOCALAPPDATA%\rohrpost` at runtime and honour `ROHRPOST_HOME`. Do not
hand-edit any generated wrapper.

## Validate

Windows decides executability by file extension, not a mode bit, so the
`test -x` equivalent is a plain existence check, and the `bash -n` equivalent
is a parse run:

```powershell
Test-Path scripts/rohrpost.ps1 -PathType Leaf
Test-Path scripts/rohrpost.cmd -PathType Leaf

$errors = $null
[System.Management.Automation.Language.Parser]::ParseFile(
    (Resolve-Path scripts/rohrpost.ps1), [ref]$null, [ref]$errors) | Out-Null
if ($errors) { $errors; exit 1 }
```

cmd has no offline syntax check; the end-to-end call below is its validation.

Finally, verify the installation from the caller's repository — each shell
must reach the same install through its own wrapper:

```powershell
scripts\rohrpost.ps1 doctor --json
```

```bat
scripts\rohrpost.cmd doctor --json
```

```bash
scripts/rohrpost doctor --json
```

If any prerequisite, download, build, executable, or validation step fails,
stop and report the exact failure.

## If PowerShell blocks the wrapper

If PowerShell reports that running scripts is disabled on the system, widen
the policy for the current user once:
`Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`.
