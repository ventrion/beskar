# BSK notation

The line format of Beskar's files, specified in [docs/FORMAT.md](../../docs/FORMAT.md). This context parses and edits documents and reports mistakes; it knows nothing about what any file means.

## Language

**Document**:
A parsed BSK file that keeps every line, so it can be written back unchanged.
_Avoid_: config object, tree

**Entry**:
A `key: value` line.
_Avoid_: property, field, pair, assignment

**Key**:
The name before an entry's colon: lowercase letters, digits and `-`.
_Avoid_: field name, property name

**Value**:
The text after an entry's colon, trimmed and otherwise verbatim. It has no type until a schema gives it one.
_Avoid_: string

**Section**:
A `[name label]` header line and the entries below it, up to the next header.
_Avoid_: table, group

**Label**:
The free text after a section's name, such as a workspace path.
_Avoid_: argument, section id

**Root block**:
The entries before the first section header.
_Avoid_: top level, global section

**Block**:
The root block or one section; the unit in which keys are looked up.

**List**:
The values of one key repeated within a block, in file order.
_Avoid_: array, sequence

**Schema**:
The rules one kind of file imposes: which keys exist, which may repeat, and what their values mean. A schema belongs to the code that reads the file, not to BSK.
