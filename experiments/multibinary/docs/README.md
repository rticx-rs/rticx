# RTICX multi-binary extension — documentation

Documentation skeleton for the experimental multi-binary / heterogeneous
multi-core extension of RTICX. The authoritative design and task checklist is
[`multibinary-multicore-plan.md`](../../../multibinary-multicore-plan.md)
(§12 lists the documentation deliverables); this directory only holds the
user-facing and architecture documents.

| Document | Status | Content |
|---|---|---|
| [User guide](user-guide.md) | outline | installation, `rticx.toml`, `ipc-types.toml`, task syntax, build/run, troubleshooting |
| [Architecture](architecture.md) | outline | phases, manifests, memory/priority rules, locking, boot/reset, failure modes |
| [Diagrams](diagrams/README.md) | 1 authored, 4 stubs | PlantUML sources for the five plan diagrams |

Status legend:

- **outline** — section structure plus the parts that already exist (M0);
  remaining content is filled in by
  [M5-T3](../../../multibinary-multicore-plan.md#m5--multi-sourcetarget-readyepoch-complete-docs).
- **stub** — placeholder with a `TODO(M5-T4)` marker.

## Status snapshot (M1, through M1-T7)

- `cargo xbin sync` validates `rticx.toml`, collects `<app>.xbin.json`
  per application, merges/validates the project, allocates the per-task FIFOs,
  emits the sealed `target/rticx-xbin/system.json` and generates/updates the
  `ipc-types/` crate, rewriting only changed files.
- Phase-2 codegen (M3) and building the applications (M4) are still pending,
  so parts of the guide are marked *planned*.
