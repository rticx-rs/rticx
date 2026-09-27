# Diagrams (PlantUML sources)

Text sources for the five diagrams in
[plan §12](../../../../multibinary-multicore-plan.md#12-documentation-and-plantuml).
Rendering is **manual**: no SVG is generated or committed by the build; the
`.puml` files are the source of truth.

```bash
plantuml -tsvg docs/diagrams/*.puml   # writes SVG next to each source
```

| # | Source | Subject | Status |
|---|---|---|---|
| 1 | [01-project-topology.puml](01-project-topology.puml) | two binaries, global core ids, IPC regions (`rticx.toml`) | authored |
| 2 | [02-sync-build-pipeline.puml](02-sync-build-pipeline.puml) | `sync` → merge/validate/allocate → `build` | stub |
| 3 | [03-spawn-timeline.puml](03-spawn-timeline.puml) | spawn flow A→B: producer, FIFO, ring function, router, ready queue, line dispatcher, exec | authored |
| 4 | [04-memory-priority-map.puml](04-memory-priority-map.puml) | per-task FIFOs, `ipc_dispatchers` pool, per-pair routers | authored |
| 5 | [05-type-pipeline.puml](05-type-pipeline.puml) | IDL → generated crate → layout asserts | stub |

Conventions:

- one diagram per file, `@startuml <slug>` / `@enduml`;
- `monochrome` and `defaultFontName monospace` for small, readable output;
- keep each diagram under one screen; split instead of growing;
- update the source in the same change as the behaviour it documents.
