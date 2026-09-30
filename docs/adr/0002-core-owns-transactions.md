# Keep deployment state and files in one core transaction

A failure between changing copies and saving the registry can lose the baseline needed to protect local work. Beskar uses PR #10's persisted journal for the whole selected batch, with the registry included, and places planning, validation, locks, and recovery behind the core session interface. CLI and future interfaces inspect immutable plans and supply conflict choices without handling staging paths or rewriting records themselves.
