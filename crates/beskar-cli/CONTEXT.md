# Beskar CLI

The command-line presentation of Beskar operations.

## Language

**Dry run**:
A validated preview that changes no managed content or registry records.
_Avoid_: partial update

**Conflict policy**:
An explicit decision about unresolved local drift: abort, keep, or replace. Asking obtains those decisions interactively.
_Avoid_: automatic merge

**Report**:
The operation's result presented as text or a JSON document.
_Avoid_: registry
