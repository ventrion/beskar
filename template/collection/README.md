# Beskar skill collection

This directory is a starter for a Beskar collection: a Git repository that
publishes skills to Beskar libraries. Copy it, add your skills, commit, and
push. Machines subscribe with:

```sh
beskar remote add your-name URL --yes
```

Layout:

- `skills/` — one directory per skill, each with a `SKILL.md`. This is the
  only required directory.
- `profiles/` — optional named setups (`beskar profile` format) that import
  on every sync. Keep names distinct from personal profiles on receiving
  machines, or the sync will ask which side wins.
- `metadata/` — optional `NAME.bsk` requirement records. A collection is
  self-contained: requirements must name skills in this repository.

Remove this README or keep it; Beskar ignores files outside the three
directories. See `docs/COLLECTIONS.md` in the Beskar repository for the full
sync semantics.
