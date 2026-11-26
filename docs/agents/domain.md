# Domain Docs

How the engineering skills should consume this repo's domain documentation when exploring the codebase.

## Before exploring, read these

- **`GLOSSARY.md`** at the repo root: project-specific terms and `_Avoid_` vocabulary.
- **`docs/context/architecture.md`**: current ownership, module boundaries, security boundaries, and cross-process/cross-language contracts.
- **`docs/adr/README.md`** and relevant **`docs/adr/NNNN-*.md`**: read ADRs that touch the area you're about to work in, checking status and revisions in the index first.

## File structure

Single-context repo:

```
/
├── GLOSSARY.md
├── docs/
│   ├── context/
│   │   └── architecture.md
│   └── adr/
│       ├── README.md
│       ├── 0001-execution-runtime-boundary.md
│       └── ...
├── src/
└── apps/tray/
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis, a test name), use the term as defined in `GLOSSARY.md`. Don't drift to synonyms the glossary explicitly avoids.

If the concept you need isn't in the glossary yet, that's a signal: either you're inventing language the project doesn't use (reconsider) or there's a real gap (note it for `/domain-modeling`).

## Flag ADR conflicts

If your output contradicts an existing ADR or `docs/context/architecture.md`, surface it explicitly rather than silently overriding:

> _Contradicts ADR-0007 (wait-any-ready-subset), but worth reopening because…_
