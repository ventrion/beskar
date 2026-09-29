# Plate

A line-oriented, typeless configuration format. Its specification is in
[SPEC.md](./SPEC.md).

## Language

**Document**:
A Plate file: a top level, followed by any number of sections.

**Section**:
The lines following a `[kind label]` header, up to the next header.
_Avoid_: table, block, group

**Kind**:
The first word of a section header, naming the type of record.

**Label**:
The rest of a section header after the kind. Together with the kind, it identifies the section.

**Entry**:
A `key = value` (scalar) or a `key:` line with its items (list).
_Avoid_: property, field, pair

**Item**:
One `- value` line of a list.
