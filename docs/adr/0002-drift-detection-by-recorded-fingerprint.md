# Drift is detected with one recorded fingerprint per installed skill

The registry stores, for each installed skill, the fingerprint of the content
Beskar copied. It stores no copy of that content. That one value is enough to
tell apart all three situations that matter: library changed (L ≠ R), edited
locally (W ≠ R), or both. Only "both", or removing an edited copy, needs a
decision. The brief's `source_fingerprint` and `installed_fingerprint`
collapse into this one value, because an install is an exact copy.

## Consequences

`skill diff` compares a workspace copy with the *current* library, not with
the version originally installed. After a conflict is resolved with "keep",
the recorded fingerprint stays unchanged, so the conflict is reported again
until it is replaced or promoted. That is deliberate: edited copies stay
visible.
