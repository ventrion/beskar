# Use a versioned record format with quoted paths

The nine implementations use incompatible small formats. Beskar keeps PR #10's `beskar 1` records and explicit quoting because filesystem paths must round-trip spaces, quotes, backslashes, hashes, and line breaks. Parsing and text editing live in `bsk`; the core owns schemas and identity validation, so format code does not depend on deployment concepts.
