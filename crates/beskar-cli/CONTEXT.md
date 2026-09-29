# Beskar CLI

The `beskar` command. It parses the command line, asks Beskar core to do the work, and shows the result to a person or a coding agent.

## Language

**Dry run**:
Working out and showing what a command would do while changing nothing. It exits with the status the real run would have.
_Avoid_: preview, simulate

**Shortcut**:
The top-level `status` and `update` commands, which stand for `repo status` and `repo update`, or `registry status` and `registry update` with `--all`.
_Avoid_: alias

**Usage error**:
A mistake in the command line itself: an unknown command or option, a missing or malformed argument, an empty path. It ends the command with status 2 and shows the usage line. A well-formed request that cannot be done, such as a skill that does not exist, is a failure with status 1.
_Avoid_: syntax error, bad request

**Non-interactive run**:
An invocation with no terminal to answer questions on. It never prompts. A decision that would need an answer stops the command with a message that names the flag to pass.
_Avoid_: batch mode, headless

**Assume yes**:
The `--yes` flag: confirmation given in advance. It does not authorize discarding library changes; `--force` does.
_Avoid_: auto-confirm, force

**JSON output**:
The `--json` form of a command's result, stable enough for a script or agent to read.
_Avoid_: machine output

**Home flag**:
`--home`, which points a single invocation at a different Beskar folder.
_Avoid_: config flag
