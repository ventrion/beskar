# Zsh

Create an autoloadable `_beskar` file beginning with `#compdef beskar`.
Use `_arguments`, `_describe`, `_files`, and `_directories` for native completion.
Keep parsing within the words at or before `$CURRENT`; account for global
options when selecting a subcommand. Use native escaping for descriptions and
candidate values, especially punctuation with meaning inside `_arguments` specs.

Install `_beskar` in a user-writable directory on `fpath`. Any new `fpath` entry
must precede the existing `compinit` call in the effective `.zshrc`, respecting
`ZDOTDIR`. Integrate with the user's framework if it already initializes
completion. If no initializer exists, add `autoload -Uz compinit` and `compinit`
once. Keep completion security checks enabled.

Run `zsh -n` and test loading with a fresh completion initialization.
For candidate tests, use an interactive terminal or Zsh's completion test
facilities. Calling `_arguments` from an ordinary script is not a valid test:
it needs a completion context. Verify Tab on nested commands, value options,
and a path with spaces. Report syntax-only checks accurately if an interactive
test is unavailable.

Source: [Zsh completion system](https://zsh.sourceforge.io/Doc/Release/Completion-System.html).
