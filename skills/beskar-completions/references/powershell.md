# PowerShell

Use `Register-ArgumentCompleter -Native -CommandName beskar,beskar.exe` on a
PowerShell version that supports it. Its script block receives
`$wordToComplete`, `$commandAst`, and `$cursorPosition`.
Use AST elements before the cursor instead of splitting the input string on
spaces. Return `System.Management.Automation.CompletionResult` objects with
properly quoted insertion text. Keep literal paths separate from wildcard
patterns, and handle trailing directory separators.

Save a persistent `beskar.ps1` and dot-source it once from the user's chosen
PowerShell profile. Inspect `$PROFILE` in that host; Windows PowerShell and
PowerShell 7 can use different locations. Preserve the existing execution policy.
Check the installed version's API before choosing syntax or profile paths.

Parse the file with `System.Management.Automation.Language.Parser.ParseFile`
and check for errors. In a profile-free process, load the script and call
`System.Management.Automation.CommandCompletion.CompleteInput` on representative
lines and cursor offsets. Inspect `CompletionMatches`. Then verify the real
profile loads the script and preserves other command completers.

Source: [`Register-ArgumentCompleter`](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/register-argumentcompleter).
