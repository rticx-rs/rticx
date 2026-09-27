# RTICX multi-binary extension — documentation

Documentation for the experimental multi-binary / heterogeneous multi-core
extension of RTICX. The authoritative design and task checklist is
[`multibinary-multicore-plan.md`](../../../multibinary-multicore-plan.md)
(§12 lists the documentation deliverables); this directory holds the
user-facing and architecture documents.

| Document | Status | Content |
|---|---|---|
| [User guide](user-guide.md) | complete (M6-T3) | installation, `rticx.toml`, `ipc-types.toml`, task syntax, worked example, build/run, troubleshooting |
| [Architecture](architecture.md) | complete (M6-T3) | phases, manifests, memory/priority rules, locking, boot/reset, failure modes, layout and cache-maintenance worked examples |
| [Diagrams](diagrams/README.md) | complete (M6-T4) | five reviewed PlantUML sources for the plan diagrams |

Status legend:

- **complete** — reviewed content describing the implemented behaviour.

## Status snapshot (M6-T4)

- `cargo xbin sync` validates `rticx.toml`, collects `<app>.xbin.json`
  per application, merges/validates the project, allocates the per-task FIFOs,
  emits the sealed `target/rticx-xbin/system.json` and generates/updates the
  `ipc-types/` crate, rewriting only changed files.
- Phase-2 codegen (M3) and `cargo xbin build` (M4-T1) generate and link the
  producer `cross_spawn` stubs, the receiver line dispatchers and per-pair
  doorbell routers, and the init hooks for the in-tree `fixtures/e2e` mock
  distribution.
- M4-T2 runs the generated sender and receiver in one host process over the
  mock runtime (spawn → ring function → router → pended line dispatcher →
  input verification, FIFO backpressure), and M4-T3 asserts the documented
  errors for a missing sync, a stale view or source, priority conflicts,
  unknown types and region overflow.
- M5 makes the local → global `core_ids` mapping native to `rticx-core`: the
  core/software passes emit runtime core checks against the global ids and the
  extension pass only *reads* the mapping (leaving the key to the core pass)
  while `external_cores` stays pass-owned.
- M5.5 moves cross-binary declarations to the native `#[sw_task]` +
  `impl RticSwTask` syntax: receivers are the only declaration of a task, the
  producer applications declare nothing and get their `Task::cross_spawn`
  stubs generated from the system view, and the pass requires the
  distribution's `swtasks` feature. Async cross-binary tasks are out of scope.
- M6.5 replaces the v1 per-line doorbell IRQs with the dispatch chain:
  `cross_spawn` → generated per-pair ring function → per-pair **doorbell
  router** → line **ready queue** → line **dispatcher** bound to an
  `ipc_dispatchers` pool entry → `exec`. The `CrossBinBackend` runtime trait
  carries no doorbell methods; the router IRQ is configured by the core pass's
  used-IRQ machinery.
- M6-T1 adds multi-source support (disjoint priority lines per producer core,
  producer-owned FIFO initialization, `fixtures/three-app` and the build-phase
  priority-line validation), M6-T2 the ready/epoch gate (`ReadyCache`) with
  not-ready and peer-reset tests, M6-T3 the complete documentation set and
  M6-T4 the five PlantUML diagram sources. M8-T1 adds the advisory CI
  workflow.
