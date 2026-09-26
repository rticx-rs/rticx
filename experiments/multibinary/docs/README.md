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

## Status snapshot (M0)

- `rticx.toml` parsing/validation and the `cargo xbin` CLI skeleton work:
  `cargo xbin sync` validates the project manifest and creates
  `target/rticx-xbin/`.
- `ipc-types.toml` parsing, the canonical layout engine and the `ipc-types`
  crate generator work in `rticx-xbin-proto`.
- Metadata collection, `system.json`, generated code and the fixture project
  are still pending (M1+), so parts of the guide are marked *planned*.
