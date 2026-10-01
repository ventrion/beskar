# Elvish

Inspect the installed version's completion API. Current Elvish uses
`edit:completion:arg-completer[beskar]`; older releases may use a different name.
Register a function receiving the command name and argument words, including a
trailing empty word when starting a new argument.

Return candidate strings through value output with `put`; leave prefix matching
to the editor. Use `edit:complex-candidate` for descriptions and native filename
completion for path arguments. `edit:complete-dirname` requires Elvish 0.21 or
newer. Keep other entries in the completer map intact.

Place a `beskar.elv` module in the user's module search path and load it once
from their existing `rc.elv`. Resolve these locations using the installed
Elvish configuration instead of assuming a Unix home layout.

Use the installed version's parse-only mode, then test the registered function
with representative argument lists. The editor API requires an interactive
context; use a terminal session when a batch process cannot load it. Verify
actual Tab behavior as well as returned candidate values.

Source: [Elvish editor completion API](https://elv.sh/ref/edit.html#argument-completer).
