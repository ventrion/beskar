# Nushell

Generate a module with `export extern` declarations for Beskar and its command
paths. Include supported global flags on the relevant declarations, named
options, optional positionals, and rest arguments. Attach fixed-choice
completers to argument shapes, and use path shapes where appropriate.
Keep external command execution and argument forwarding intact.

Save a persistent `beskar.nu` and add one `use /path/to/beskar.nu *` statement
to the user's actual config file, found through `$nu.config-path`.
Use a quoted literal module path that the parser can resolve. Preserve any
existing external completer; declarations for Beskar do not require replacing
the user's global completion closure.

Check APIs against the installed version because Nushell's completion interfaces
change. Load the module in a clean Nu process. Where supported, use
`'beskar repo ' | commandline complete` to inspect candidates; otherwise test
in a terminal. Test fixed values and path quoting, and check that extern
declarations still allow harmless commands such as `beskar --version`.

Source: [Nushell custom completions](https://www.nushell.sh/book/custom_completions.html).
