# bsk

The line-oriented file format Beskar uses for its own files: every line stands alone, every value is a string, nothing is quoted. This context is the format itself and knows nothing about skills.

## Language

**Document**:
A parsed bsk file that prints back byte for byte, and that edits in place.
_Avoid_: config, file object

**Entry**:
One `key value` line.
_Avoid_: property, setting, field, pair

**Key**:
The word that starts an entry.
_Avoid_: name, property name

**Value**:
The rest of an entry's line after the key, with surrounding whitespace removed. Always a string.
_Avoid_: field value, argument

**Section**:
A group of entries under a `[kind name]` header, which runs until the next header.
_Avoid_: table, block, group

**Scope**:
The entries of one section, or the top-level entries before the first header.
_Avoid_: namespace, context

**Comment**:
A whole line whose first non-blank character is `#`. A `#` anywhere else is part of a value.
_Avoid_: trailing comment, annotation

**Attached comment**:
The comment lines directly above another line, with no blank line between them and that line. They belong to that line.
_Avoid_: header comment, doc comment

**List**:
The values of a key that appears on several lines, in file order.
_Avoid_: array, sequence

**Schema**:
The keys and sections a program accepts, and how often each key may appear.
_Avoid_: grammar (the grammar is the fixed syntax; a schema is per program)

**Diagnostic**:
A problem in a file with its line and column and, where possible, a hint for fixing it.
_Avoid_: error message, warning

**Lossless edit**:
A change that touches only the lines it means to, so comments, blank lines, indentation, alignment and line endings survive.
_Avoid_: rewrite, regenerate
