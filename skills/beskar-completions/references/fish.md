# Fish

Write `beskar.fish` using `complete -c beskar` and conditions for each command
context. Use `commandline` token APIs in helper functions so option values do
not look like subcommands. Set whether options require values and whether file
completion applies. Include descriptions from the discovered help.

Install into the user's existing completions directory on `$fish_complete_path`,
usually `$__fish_config_dir/completions`. Fish autoloads a file named after the
command, so an extra startup source line is normally unnecessary. Check for
existing Beskar definitions before replacing anything.

Run `fish -n`. Source the staged script in a clean Fish process and inspect
`complete -C 'beskar repo '` and equivalent cases with flags and paths. Results
may include tab-separated descriptions; assert candidate values separately.
Repeat loading and check for duplicate candidates. Verify autoloading in a fresh
session using the actual destination before reporting installation complete.

Sources: [writing completions](https://fishshell.com/docs/current/completions.html),
[`complete`](https://fishshell.com/docs/current/cmds/complete.html).
