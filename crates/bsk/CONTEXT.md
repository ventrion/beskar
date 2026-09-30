# bsk

The editable record language used by Beskar files.

## Language

**Record**:
One named fact with ordered values on one line.
_Avoid_: object, table

**Value**:
Literal text within a record. Quoting changes its representation, not its meaning.
_Avoid_: inferred type, expression

**Version record**:
The declaration of the document's grammar version.
_Avoid_: application version
