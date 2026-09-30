# Track ownership explicitly and fingerprint complete skills

Several candidate PRs adopt an existing copy when its bytes match the library or ignore generated content during comparison. Beskar keeps ownership explicit and fingerprints all supported content, so matching bytes never authorize a later deletion and ignored files cannot disappear unnoticed. Copies support regular files and directories; symlinks are rejected, and local edits or deletion require a conflict policy before replacement.
