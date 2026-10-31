# Documentation Index

Durable project decisions live in these documents. Start with the one that owns
the topic you are changing, and update it in the same change as the behavior.

| Document | Owns |
| --- | --- |
| [`SPEC.md`](SPEC.md) | Product specification, MVP scope, behavior contracts, verification status, and the manual checklist |
| [`implementation-guidelines.md`](implementation-guidelines.md) | Module boundaries, domain-primitive and parsing rules, error handling, and the review checklist |
| [`ui-design-decisions.md`](ui-design-decisions.md) | Selected UI direction: layouts, visualizer, themes, splash, Signal View, and the Agent Planets presentation (plus its superseded lineage) |
| [`audio-spike.md`](audio-spike.md) | Native audio pipeline findings, playback caveats, control-thread and cleanup behavior |
| [`herdr-plugin.md`](herdr-plugin.md) | Herdr plugin operations: install, verify, open, develop, update, uninstall, troubleshoot |

Related files outside this directory:

- [`../README.md`](../README.md) — user-facing overview, install, and controls.
- [`../AGENTS.md`](../AGENTS.md) — agent workflow entry point and scope guard.
- `index.html` and `images/` — the GitHub Pages site and its screenshots.

Per-iteration design and plan documents are not kept here: durable decisions are
consolidated into the documents above, and the originals are preserved in git
history.
