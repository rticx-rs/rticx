# Diagrams (PlantUML sources)

Text sources for the five diagrams in
[plan §12](../../../../multibinary-multicore-plan.md#12-documentation-and-plantuml).
Rendering is **manual**: no SVG is generated or committed by the build; the
`.puml` files are the source of truth. All five are authored and reviewed
(M6-T4); each renders to well under one 1920×1080 screen.

```bash
plantuml -checkonly docs/diagrams/*.puml   # syntax check
plantuml -tsvg docs/diagrams/*.puml        # writes SVG next to each source
```

| # | Source | Subject |
|---|---|---|
| 1 | [01-project-topology.puml](01-project-topology.puml) | two binaries, global core ids, IPC regions (`rticx.toml`) |
| 2 | [02-sync-build-pipeline.puml](02-sync-build-pipeline.puml) | `sync` → merge/validate/allocate → `build` |
| 3 | [03-spawn-timeline.puml](03-spawn-timeline.puml) | spawn flow A→B: producer, ready gate, FIFO, ring function, router, ready queue, line dispatcher, exec |
| 4 | [04-memory-priority-map.puml](04-memory-priority-map.puml) | per-task FIFOs, `ipc_dispatchers` pool, per-pair routers (two sources onto one target) |
| 5 | [05-type-pipeline.puml](05-type-pipeline.puml) | IDL → generated crate → layout asserts |

Conventions:

- one diagram per file, `@startuml <slug>` / `@enduml`;
- `monochrome` and `defaultFontName monospace` for small, readable output;
- keep each diagram under one screen; split instead of growing;
- escape a leading `#` as `~#` (PlantUML would otherwise read `#[app]` as
  list/colour markup);
- update the source in the same change as the behaviour it documents.
