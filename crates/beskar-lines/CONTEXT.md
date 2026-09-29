# Lines

The file format behind every Beskar file. It exists so that people and coding agents can read and edit configuration without a parser library, and without a way to write something that quietly means something else.

## Language

**Document**:
The contents of one Lines file, kept line by line so that writing it back changes only what was edited.
_Avoid_: config, file contents

**Entry**:
A line with a key and, optionally, a value. Everything in a document that is not blank or a comment is an entry.
_Avoid_: setting, property, field

**Key**:
The first word of an entry, in lowercase letters, digits and dashes.
_Avoid_: name, property name

**Value**:
The rest of an entry's line, taken literally. It has no type, no quoting and no escapes.
_Avoid_: string, argument

**Child**:
An entry indented under an unindented entry, its parent. Nesting goes one level deep and the amount of indentation means nothing.
_Avoid_: sub-entry, nested key

**Comment**:
A line whose first non-blank character is `#`. A `#` anywhere else is part of a value.
_Avoid_: annotation, note

**Lossless edit**:
Adding, changing or removing an entry while every other line, including comments and alignment, comes back byte for byte.
_Avoid_: round trip, pretty print
