# Install Rohrpost locally

Use this playbook only after the Rohrpost skill reports that
`scripts/rohrpost` is missing or its installation is incomplete.

On Windows — PowerShell, cmd, or Git Bash — follow
[`playbooks/windows.md`](windows.md) instead of the steps below: it places
`rp.exe`, materialises the `.ps1`, `.cmd`, and Git Bash wrappers, and
validates them with the Windows equivalents of the checks here.

## Permission

Ask the user whether to install Rohrpost locally. Explain that this will
download (or build) the single `rp` binary into an install home and create the
local `scripts/rohrpost` wrapper. Nothing else is installed: `rp` is a static
executable with no runtime dependencies. Stop if the user declines.

## Location

Use `ROHRPOST_HOME` when it is already set. Otherwise use:

```bash
export ROHRPOST_HOME="${XDG_CACHE_HOME:-$HOME/.cache}/rohrpost"
```

Keep this variable set for the current shell. The generated wrapper also embeds
the resolved path, so later calls remain usable when the environment variable
is not restored. The binary lives at `$ROHRPOST_HOME/bin/rp`.

## Provision

Preferred: a prebuilt release. Releases are published at
`https://github.com/code-factorio/rohrpost/releases`, one archive per target
(`rp-<tag>-<target>.tar.gz`). Pick the target for the machine:
`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`,
`aarch64-apple-darwin` or `x86_64-apple-darwin`. Require `curl` and `tar`.

```bash
mkdir -p "$ROHRPOST_HOME/bin"
tag="$(curl -fsSL https://api.github.com/repos/code-factorio/rohrpost/releases/latest | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')"
curl -fsSL "https://github.com/code-factorio/rohrpost/releases/download/$tag/rp-$tag-<target>.tar.gz" \
  | tar -xz -C "$ROHRPOST_HOME/bin" --strip-components=1 "rp-$tag-<target>/rp"
chmod +x "$ROHRPOST_HOME/bin/rp"
```

Record the tag that was installed. Verify the checksum against the release's
`SHA256SUMS` when the environment allows it.

Fallback when no release matches the machine or the network blocks GitHub
release downloads but a Rust toolchain (1.89+) is available:

```bash
cargo install --git https://github.com/code-factorio/rohrpost --locked --root "$ROHRPOST_HOME"
```

`cargo install --root` places the binary at `$ROHRPOST_HOME/bin/rp`. If neither
route is available, stop and report the missing prerequisite. Do not substitute
a different checkout or a system-wide `rp`.

## Materialize the wrapper

From the repository containing this skill, replace the placeholder in
`scripts/rohrpost.template` with the shell-quoted value of `$ROHRPOST_HOME`,
write the result to `scripts/rohrpost`, and make it executable. Do not
hand-edit the generated wrapper.

Validate the generated file before using it:

```bash
bash -n scripts/rohrpost
test -x scripts/rohrpost
```

Finally, verify the installation from the caller's repository:

```bash
scripts/rohrpost doctor --json
```

If any prerequisite, download, build, executable, or validation step fails,
stop and report the exact failure. The wrapper is intentionally a validator and
launcher; it never repairs an installation itself.
