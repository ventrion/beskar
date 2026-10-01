# Bash

Use a function registered with `complete -F`. Read `COMP_WORDS` through
`COMP_CWORD`, return candidates in `COMPREPLY`, and reset it on each call.
Use `COMP_LINE` and `COMP_POINT` when word boundaries need clarification.
Keep command detection scoped to `COMP_WORDS`: the full line may contain earlier
commands or pipelines. Do not evaluate the command line. Account for `=` in
`COMP_WORDBREAKS` when handling inline option values; leave the user's global
word-break settings intact.

Use `compgen` for paths and fixed choices. Preserve each path as one array
element instead of splitting command substitution on whitespace. Apply filename
quoting and directory continuation in path contexts. Target the installed Bash:
macOS users may still have Bash 3.2, without associative arrays or `mapfile`.
Keep external bash-completion helpers optional.

For installation, store a script such as `beskar.bash` in a persistent user
directory and source it from the interactive startup file using one marked block.
Use a bash-completion autoload directory only if that loader is already configured.
Check whether login shells source the user's interactive configuration.

Run `bash -n`. In a clean Bash process, source the file, set completion variables
for each test case, call the registered function, and assert `COMPREPLY`.
Also test a path insertion through Readline in a terminal when available.

Sources: [completion API](https://www.gnu.org/software/bash/manual/html_node/Programmable-Completion.html),
[completion builtins](https://www.gnu.org/software/bash/manual/html_node/Programmable-Completion-Builtins.html).
