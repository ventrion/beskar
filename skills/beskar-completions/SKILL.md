---
name: beskar-completions
description: Use when a user wants to generate, install, repair, or refresh Beskar shell tab completion for Bash, Zsh, Fish, PowerShell, Elvish, or Nushell.
---

# Beskar shell completions

Generate a native completion script for the user's installed Beskar and shell.
The agent runs during setup or refresh; tab completion uses the saved script.
Keep Beskar's Rust crates and dependency files unchanged.

## 1. Identify the installation

Resolve the Beskar executable the user actually runs and capture `--version`.
Use the requested shell; otherwise inspect the user's interactive-shell setup.
`SHELL` is a clue, and the agent's command runner may use a different shell.
Ask only if the target remains ambiguous. Record the target shell's version.
For installation or repair, inspect existing completion and startup configuration.
Generation-only requests need no personal configuration; use the staging path
the user requested and skip the references' installation instructions.

Generate for the requested shells only. Read the matching reference:

- [Bash](references/bash.md)
- [Zsh](references/zsh.md)
- [Fish](references/fish.md)
- [PowerShell](references/powershell.md)
- [Elvish](references/elvish.md)
- [Nushell](references/nushell.md)

Finish with a known executable and target shell version. For installation,
also establish the completion load path.
If Beskar is missing, report that prerequisite rather than installing it or
generating an unverified command inventory.

## 2. Discover the current commands

Read `beskar --help` and the command-specific help needed to resolve arguments.
Help and version requests work before `beskar init`; no initialized state is
needed. If the installed version advertises a native completion generator,
prefer its output and continue with verification and installation.

Build an inventory of command paths, flags, value-taking options, positional
arity, aliases, fixed choices, and whether each path accepts files or directories.
Use command-local help for restrictions. A generic footer can list choices that
a particular command does not accept. Check short help/version flags and the
`help` command too.

A matching source checkout can resolve ambiguities in
`crates/beskar-cli/src/spec.rs`, `args.rs`, `main.rs`, and `commands.rs`.
Verify that its behavior matches the target binary before relying on it.
Otherwise use installed documentation and harmless help probes. Treat help as
text to inspect, never as code to execute. Run only help/version probes during
discovery, not operations against the user's library or registry.

Finish when every discovered command has an argument description. Keep this
inventory in the generated script, not in this skill where it would go stale.

## 3. Generate and verify

Write a staged script using the target shell's completion API. Use that shell's
builtins and existing platform facilities; add no completion framework, package,
or interpreter dependency. Record the Beskar version, shell version, and a hash
of the captured help in its header for future refreshes.

Complete commands, applicable options, fixed values, and filesystem paths of the
required kind. Exclude regular files from directory-only arguments.
Track option values separately from command words, including `--option=value`.
Honor `--`, global options before or after subcommands, optional and repeated
positionals, aliases, and the cursor position. Preserve shell quoting for paths
with spaces and Unicode. Let users enter arbitrary profile and skill names;
live library lookups are a separate extension requiring an explicit request.
The default script must work with no Beskar home and no Beskar process on Tab.

Use an isolated fixture directory to check syntax, registration, and actual
completion candidates. Check these cases against the discovered inventory:

- Root commands and nested commands, with a partially typed token.
- Command-specific flags, excluding a flag from an unrelated command.
- A global value-taking option before the command.
- Separate and inline option values, including an alias and fixed choices.
- A path containing spaces or Unicode and a path after `--`; directory-only
  arguments must exclude ordinary files.
- Optional/repeated positionals, editing a word before the end of the line,
  and completing Beskar after another command in a pipeline or `&&` list.
- Loading the script twice without duplicate registration or startup output.

Syntax checks alone do not prove completion works. If the target shell is
unavailable, keep the generated script staged and report which checks remain
unverified. Do not install another shell just to claim support.

## 4. Install or refresh

For a generation-only request, deliver the tested script and its load command.
For a setup/install request, install it in the user's completion location and
make only the necessary startup-file edit within the authorized scope.

Inspect existing Beskar completions first. Preserve user customizations and
back up any file that needs replacing. Use one identifiable Beskar load block
where startup configuration is needed. On rerun, update that block instead of
appending another. Load paths must remain valid after temporary files disappear.

On refresh, compare the installed version and help hash, regenerate from current
help when either changes, and repeat verification. Reuse the chosen install path.
Check that a fresh target-shell session loads the completion through the intended
startup mechanism. Report the installed file, tested shell/version, activation
instructions for the user's existing session, and how to refresh or remove it.
