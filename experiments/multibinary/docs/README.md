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
  [M6-T3](../../../multibinary-multicore-plan.md#m6--multi-sourcetarget-readyepoch-complete-docs).
- **stub** — placeholder with a `TODO(M6-T4)` marker.

## Status snapshot (M5.5)

- `cargo xbin sync` validates `rticx.toml`, collects `<app>.xbin.json`
  per application, merges/validates the project, allocates the per-task FIFOs,
  emits the sealed `target/rticx-xbin/system.json` and generates/updates the
  `ipc-types/` crate, rewriting only changed files.
- Phase-2 codegen (M3) and `cargo xbin build` (M4-T1) generate and link the
  producer `cross_spawn` stubs, the receiver doorbell dispatchers and the init
  hooks for the in-tree `fixtures/e2e` mock distribution.
- M4-T2 runs the generated sender and receiver in one host process over the
  mock runtime (spawn → dispatcher → input verification, FIFO backpressure),
  and M4-T3 asserts the documented errors for a missing sync, a stale view or
  source, priority conflicts, unknown types and region overflow.
- M5 makes the local -> global `core_ids` mapping native to `rticx-core`: the
  core/software passes emit runtime core checks against the global ids and the
  extension pass only *reads* the mapping (leaving the key to the core pass)
  while `external_cores` stays pass-owned.
- M5.5 moves cross-binary declarations to the native `#[sw_task]` +
  `impl RticSwTask` syntax: receivers are the only declaration of a task, the
  producer applications declare nothing and get their `Task::cross_spawn`
  stubs generated from the system view, and the pass requires the
  distribution's `swtasks` feature. Async cross-binary tasks are out of scope.
- M6 (multi-source/target, ready/epoch, complete docs) is still pending.
