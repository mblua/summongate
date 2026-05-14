# Agent Skills Tutorial

Agent skills are reusable instructions stored in an agent's canonical
`skills/` folder. Use them for workflows that are too specific for a general
role prompt but useful enough to keep around, such as release checklists,
framework conventions, debugging recipes, or tool-specific operating notes.

In Agents Commander, the `skills/` folder is filesystem support. Agent Matrix
creation creates the folder, and workgroup replica context grants the replica
write access to the origin Agent Matrix `skills/` directory. Agents Commander
does not currently scan skills, list them in the prompt, or inject `SKILL.md`
bodies automatically. A coding agent must be asked or instructed to read the
relevant skill file during the task.

## Where Skills Live

For an Agent Matrix agent, skills live beside the agent's canonical role,
memory, and plans:

```text
<project>/
+-- .ac-new/
    +-- _agent_dev-rust/
        +-- Role.md
        +-- memory/
        +-- plans/
        +-- skills/
```

When that agent runs inside a workgroup replica, the generated
AgentsCommander context allows writes to the origin Agent Matrix `skills/`
folder. That keeps skills canonical across replicas instead of copying them
into one temporary workgroup session.

Standalone agent folders can also contain a local `skills/` folder, but today
Agents Commander does not create or manage that folder for plain
`create-agent` agents.

## Minimal Skill Layout

Use one directory per skill. The required entry point is `SKILL.md`.

```text
skills/
+-- rust-test-triage/
    +-- SKILL.md
    +-- references/
        +-- cargo-flags.md
```

Minimal `SKILL.md`:

```markdown
# rust-test-triage

Use this skill when a Rust test, `cargo check`, or `cargo clippy` failure needs
triage.

## Workflow

1. Read the failing command output.
2. Identify whether the failure is compile, lint, test behavior, or environment.
3. Inspect the smallest relevant module first.
4. Fix the cause without broad refactors.
5. Re-run the failing command, then any nearby lightweight checks.

## References

- `references/cargo-flags.md` for common command variants.
```

A skill can contain extra files such as `references/`, `templates/`, or
`scripts/`. Keep `SKILL.md` small enough to read at task start, and let it point
to larger supporting files only when they are needed.

## Creating a Skill

1. Open the canonical Agent Matrix directory for the agent, for example
   `.ac-new/_agent_dev-rust/`.
2. Create `skills/<skill-name>/`.
3. Add `skills/<skill-name>/SKILL.md`.
4. Describe when to use the skill and the exact workflow to follow.
5. Add small reference files only when they reduce repeated instructions.

Skill names should be short, lowercase, and filesystem-friendly:
`rust-test-triage`, `release-notes`, `ui-accessibility-check`.

## Using a Skill

Because Agents Commander does not inject skills automatically, reference them
explicitly in normal workflow prompts or role instructions:

```text
Use the rust-test-triage skill for this cargo clippy failure.
```

or:

```text
Before publishing release notes, read skills/release-notes/SKILL.md and follow
that workflow.
```

An agent should then:

1. Locate the relevant `skills/<skill-name>/SKILL.md`.
2. Read `SKILL.md` before making changes.
3. Open only the referenced supporting files that matter for the current task.
4. Apply the workflow while still obeying the session's write restrictions.
5. Mention in the final report which skill was used when that helps review.

If the user names a skill that does not exist, the agent should say so and
continue with the best available fallback instead of inventing hidden behavior.

## Current Runtime Behavior

Agents Commander currently supports skills as files:

- Agent Matrix creation creates a `skills/` folder.
- Workgroup replica context permits writes to the origin Agent Matrix
  `skills/` folder.
- The generated session context names `skills/` as an allowed canonical state
  directory when an origin Agent Matrix is available.

Agents Commander currently does not:

- Discover available skills at session start.
- Add skill names to `AGENTS.md`, `CLAUDE.md`, or `GEMINI.md` automatically.
- Inject `SKILL.md` contents into the model prompt.
- Decide when a skill applies to a task.
- Validate skill directory structure.

That means a skill is inert until the user, role prompt, or agent workflow
explicitly tells the coding agent to read it.
