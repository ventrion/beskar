---
name: writing-agent-skills
description: Use when creating, editing, or reviewing a SKILL.md or a skill folder. Covers frontmatter, description triggers, body layout, and which content to push into reference files.
---

# Writing agent skills

A skill is a folder with a `SKILL.md` at its root. The agent sees the frontmatter `description` on every turn, and loads the body only when the skill fires. So: write the description as a trigger, the body as a procedure, and everything else as reference reached through a link.

**Before writing any content, load the `writing-for-agents` skill** and apply it to every file you produce here - the description, the body, and every reference file. It holds the writing rules (context pointers, information hierarchy, completion criteria, leading words, pruning); this skill holds only the skill-specific mechanics and the order of work.

## 1. Decide who fires the skill

- **Model-invoked** (default) - the agent fires it on its own from the description, and other skills can call it. Omit `disable-model-invocation`.
- **User-invoked** - only a human typing `/skill-name` or `$skill-name` fires it. Set `disable-model-invocation: true`. Choose this when the skill only ever runs by hand; it then costs no context on turns where it is idle.

Done when: you can say in one sentence who fires the skill, and the frontmatter matches.

## 2. Write the frontmatter

```yaml
---
name: skill-name              # kebab-case, same as the folder name
description: Use when ...     # see below
disable-model-invocation: true   # user-invoked skills only
---
```

`name` and `description` are required.

### Description

For a **model-invoked** skill the description is the only text the agent reads before deciding to load the skill, so it does all the triggering work:

- Open with the trigger: "Use when ..." followed by the distinct situations that should fire the skill.
- One trigger per situation. Two phrasings of the same situation are one trigger written twice - keep one.
- Use the words the user and your other docs already use for this task; the agent matches on shared vocabulary.
- Leave out what the body already says about itself.

For a **user-invoked** skill the description is read by humans only: one line that says what the skill does, no trigger list.

Done when: given a realistic task, the skill fires on the intended cases and stays quiet on the rest.

## 3. Write the body

Write the body with `writing-for-agents`: steps in order, each ending on a checkable completion criterion; reference inline only when every run needs it, otherwise behind a link; every line pruned by its rules.

Length: a simple skill's `SKILL.md` holds everything. A complex skill's `SKILL.md` is a short map of steps with links to detail.

Done when: every line either changes the agent's behaviour or routes to a file that does.

## 4. Lay out the folder

Create a directory only when a file needs to live in it.

```
skill-name/
  SKILL.md        required - the entry point
  references/     reference docs reached by links from SKILL.md
  examples/       worked examples the body points to
  scripts/        scripts the body tells the agent to run, plus any package they need
  tests/          cases that verify the skill behaves as written
  README.md       optional - human-facing notes; the agent does not read it
```

Every file other than `SKILL.md` must be reachable by a link from `SKILL.md` (or from a file `SKILL.md` links to). A file nothing links to is dead weight - link it or delete it.

## 5. Review before you ship

- Frontmatter has `name` and `description`; `disable-model-invocation` matches who fires the skill.
- Description triggers on the intended cases only.
- Every step ends on a checkable completion criterion.
- Every linked file exists; every file is linked.
- No meaning appears in two places.