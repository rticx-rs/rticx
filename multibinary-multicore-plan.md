# RTICX Multi-Binary / Heterogeneous Multi-Core Extension — Implementation Plan

**Status:** approved, not started
**Date:** 2026-09-26
**Target:** experimental, in-tree development, designed for later extraction into its own repository.

This document is the single reference for implementing the extension. Work proceeds
one task at a time using the task IDs in [Milestones](#14-milestones-and-task-checklist)
(e.g. "implement M0-T2"). Each task lists its acceptance criteria.

---

## 1. Purpose

Extend RTICX from "single binary, possibly multi-core" to "multi-binary, multi-core"
projects where:

- each group of homogeneous cores shares a single binary and continues to use the
  existing single-binary multicore framework;
- heterogeneous core groups run separate binaries;
- cross-core task spawns between binaries are carried over shared-memory FIFOs with a
  hardware doorbell, reusing the RTICX task/dispatcher programming model.

End goal and acceptance platform: STM32H7 (Cortex-M7 + Cortex-M4) under an existing
Renode simulation, delivered by an out-of-tree distribution.

### Verdict of the design review

The model is viable and worth building, with these corrections that are now part of
the design:

1. Cross-core message representation is guaranteed by a generated
   `#[repr(C)]` type set with per-target `size_of`/`align_of`/`offset_of` const
   assertions — **not** by `#[repr(C)] + #[repr(packed(8))]` (which does not do what
   it sounds like: `packed(N)` caps alignment, it cannot raise it, and it does not
   make offsets ABI-independent).
2. Atomics provide ordering, **not** cache flushing. The shared region must be
   Normal, Non-cacheable, Shareable (MPU), or explicit cache maintenance must be
   provided. Device/Strongly-ordered memory is invalid for `ldrex`/`strex`.
3. One FIFO per `(source → target)` direction is required (SPSC); with N cores there
   are up to N(N−1) FIFOs. The distro declares one region per direction; the driver
   allocates per-task FIFOs inside it.
4. Per-task shared FIFO with direct dispatcher drain is the chosen transport:
   one copy, exact capacity semantics, industry-standard (OpenAMP/rpmsg vring-like).
5. The IDL models **data types only**. Task/priority/capacity topology lives in app
   syntax and is extracted by the new pass; the driver is the single merger.
6. v1 uses an internal interrupt-free section inside the generated spawn. The
   transport is kept independent of the lock mechanism so a later `Producer`
   shared-resource API (SRP locking) can replace it in one place.

---

## 2. Locked decisions

| Topic | Decision |
|---|---|
| v1 scope | `#[sw_task]`-equivalent control-plane spawn only; no async |
| Async | Explicitly out of scope; no extension of `rticx-async-pass` |
| Cross-binary channels / shared resources | Out of scope |
| Receiver transport | Per-task shared FIFO, direct dispatcher drain, no forwarder ISR |
| Sender-side lock | v1: `core_local_interrupt_free` inside generated spawn. Future: `Producer` as a shared resource with SRP lock |
| Type discovery | TOML IDL at workspace root, types only; driver generates a shared `ipc-types` crate |
| Type subset v1 | 32-bit-safe only: `u8..u32`, `i8..i32`, `f32`, `[T; N]`, nested messages, optional `repr(u32)` enums. No `u64/i64/f64`, `usize`, `bool`, pointers, references |
| Layout guarantee | Generated `#[repr(C)]` types + canonical layout consts + per-target compile-time assertions |
| FIFO ordering | New atomic SPSC queue (`AtomicUsize` head/tail via `portable-atomic`, Release/Acquire) — not `rticx-spsc` |
| Cache policy | Distribution configures the shared region Normal, Non-cacheable, Shareable; optional clean/invalidate fallback |
| Phase-2 metadata | Driver emits one JSON system view; each binary's pass parses the whole view and filters by its cores |
| Driver UX | `cargo xbin sync` and `cargo xbin build` (= sync + build); granular per-app cargo/clippy workflows remain possible after an explicit `sync` |
| Pass internals | The external pass reimplements needed internals for now, marked `TODO(extract): ...`; may later be promoted/shared with `rticx-sw-pass` |
| Development home | In-tree experimental workspace under `experiments/multibinary/`; extract to its own repo later |
| Core changes | None required; the generated code only uses the frozen public surface (external `task_trait` paths, etc.) |
| Boot sequencing | Distribution-owned; pass emits `init_shared` / `mark_ready` / ready-check calls through the backend trait |
| Future API | Remove `cross_spawn` in favor of a `Producer` type passed as a shared resource in `init`, allowing proper RTIC SRP locking for both single-binary and cross-binary spawns |

---

## 3. Scope

**In scope (v1):**

- One binary per homogeneous core group; each binary may use the existing
  single-binary multicore support internally.
- Cross-binary spawn of software tasks with input values, from any task on a source
  core to a task on a target core in another binary.
- Type-only IDL → generated types crate; driver-generated JSON system view.
- Driver commands `cargo xbin sync` and `cargo xbin build`.
- Distro bindings: IPC regions, doorbells, cache/MPU policy, ready/epoch.
- Documentation with small PlantUML diagrams.

**Out of scope (v1):**

- Async tasks; `rticx-async-pass` changes.
- Cross-binary `rticx-async::Channel`, cross-binary `#[shared]` resources/locks.
- Zero-copy large-buffer transfer (control-plane messages only).
- Dynamic memory.
- More than one binary per homogeneous core group.
- Integration into the root workspace/release process (until extraction).

---

## 4. Architecture overview

```
Application sources (Rust)          IDL (types only)         Distro config
  #[app(core_ids, external_cores)]     ipc-types.toml          rticx.toml [ipc.regions]
  #[cross_bin_task] receiver
  #[cross_bin_spawn] sender
          |                                |                       |
          v                                v                       v
   Phase 1: cargo xbin sync  ────────────────────────────────────────────────
     per-app metadata (pass in metadata mode)  ->  <app>.xbin.json
     driver merge + validate + allocate addresses
     -> target/rticx-xbin/system.json
     -> generated ipc-types/ crate (repr(C), consts, asserts)
          |
          v
   Phase 2: cargo xbin build (normal cargo build per app)
     pass reads system.json, filters by its cores
     -> sender stubs / receiver doorbell ISRs + dispatchers / FIFO views
     -> #[task(...)] items the core pass already understands
```

Key invariants:

- The IDL is the only source of shared data types.
- App syntax is the only source of tasks/priorities/capacities/visibility.
- The driver is the only merger and address allocator.
- SPSC discipline: per task FIFO, single producer core, single consumer dispatcher.
- Priority lines on a target core are disjoint between local and each remote source
  core (extends the existing `check_disjoint_priorities` / `check_uniform_spawn_by`
  rules to the project-wide view).

---

## 5. Repository layout

Standalone Cargo workspace at `experiments/multibinary/` (its own `[workspace]`,
deliberately outside the root workspace to keep release-plz, generation checks and
core CI untouched). It path-depends on the root crates.

| Crate | Path | Purpose |
|---|---|---|
| `rticx-xbin-proto` | `crates/rticx-xbin-proto/` | IDL parser, canonical layout engine, JSON schema + validation |
| `rticx-xbin-pass` | `crates/rticx-xbin-pass/` | `RticPass` implementation, metadata mode + codegen mode |
| `rticx-xbin-rt` | `crates/rticx-xbin-rt/` | Atomic cross-core SPSC queue, marker trait, FIFO views, ready/epoch helpers |
| `rticx-xbin-driver` | `crates/rticx-xbin-driver/` | `cargo-xbin` subcommand (`sync`, `build`, later `visualize`) |
| `rticx-xbin-mock` | `crates/rticx-xbin-mock/` | Mock distro/backend for host tests |
| fixtures | `fixtures/` | Two-app fixture project for in-tree end-to-end tests |
| docs | `docs/` | User guide, architecture doc, PlantUML diagram sources (rendered manually) |

Conventions:

- Duplicated internals from `rticx-sw-pass` are marked `TODO(extract): <what>`.
- No public API is added to the root workspace in v1.

---

## 6. Configuration and syntax

### 6.1 `rticx.toml` (project root)

Single source of project topology.

```toml
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]                  # local core index i -> global core id core_ids[i]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[ipc.regions]                   # distro-documented addresses
# Example shape, one entry per ordered pair:
# "0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
# "1->0" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
```

Notes:

- `core_ids` maps local core indices to globally unique ids used in task attributes
  and the JSON view.
- IPC regions are per `(source → target)` direction, with per-core base-address
  views (aliases allowed). The distro documents the values; the user or distro
  template fills them in.
- `target` may include an optional target triple; otherwise it comes from
  `.cargo/config.toml`.

### 6.2 `ipc-types.toml` (workspace root)

Types only. Example:

```toml
schema = 1

[message.EncryptReq]
fields = { addr = "u32", len = "u32", key = "u32" }

[message.SensorBatch]
fields = { tag = "u8", kind = "u8", xs = "[i16; 8]" }
```

Allowed field types v1:

- integers `u8 u16 u32 i8 i16 i32`
- `f32`
- fixed arrays `[T; N]`
- references to other messages by name
- optional `repr(u32)` enums declared in the IDL

Rejected: `u64/i64/f64`, `usize/isize`, `bool`, pointers, references, tuples,
default-`repr` enums, ZSTs, unions, bitfields.

### 6.3 Generated `ipc-types` crate

Generated at project root as `ipc-types/` (kept in VCS; `sync` reports changes).
Contents per message:

```rust
#[repr(C)]
pub struct EncryptReq { pub addr: u32, pub len: u32, pub key: u32 }

pub const SIZE_ENCRYPT_REQ: usize = 12;
pub const ALIGN_ENCRYPT_REQ: usize = 4;
pub const OFF_ENCRYPT_REQ_ADDR: usize = 0;
// ... one const per field
pub const LAYOUT_HASH: u64 = 0x...;

unsafe impl rticx_xbin_rt::CrossCoreMessage for EncryptReq {}

const _: () = {
    assert!(core::mem::size_of::<EncryptReq>() == SIZE_ENCRYPT_REQ);
    assert!(core::mem::align_of::<EncryptReq>() == ALIGN_ENCRYPT_REQ);
    assert!(core::mem::offset_of!(EncryptReq, addr) == OFF_ENCRYPT_REQ_ADDR);
    // ...
    assert!(cfg!(target_endian = "little"));
    assert!(cfg!(target_pointer_width = "32"));
};
```

### 6.4 `#[app(...)]` additions

- `core_ids = [g0, g1, ...]` — maps local core indices to global ids; length must
  equal `cores` (or `len == 1` when `cores` defaults to 1).
- `external_cores = [g...]` — global ids of cores in other binaries visible to this
  application.

Both keys are consumed (stripped) by the new pass before the core pass parses the
module, so other distributions only see unknown-key warnings.

### 6.5 Task syntax (confirmed in M1-T3)

Receiver binary (task actually runs here):

```rust
#[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
struct EncryptTask;

impl CrossBinTask for EncryptTask {
    type Input = ipc_types::EncryptReq;
    fn exec(&mut self, input: Self::Input) {}
}
```

Sender binary (lightweight stub; name must match the receiver task):

```rust
#[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
struct EncryptTask;
```

- Sender `core` and receiver `spawned_by` use **global** core ids. Receiver `core` is a
  **local** core index (`0..cores`, default `0`, matching RTIC's task `core`).
- Defaults: `priority = 1`, `capacity = 1` (minimum 1). Unknown, duplicate and
  multi-segment keys, malformed values, an out-of-range receiver `core` and
  `external_cores` overlapping the application's own `core_ids` are hard errors.
- Both sides declare the task so phase 1 knows who spawns what and can validate the
  match. A later relaxation may auto-generate sender stubs from receiver
  declarations.
- The pass generates `EncryptTask::cross_spawn(input) -> Result<(), Option<Input>>`
  on the sender, matching existing `cross_spawn` error semantics.

---

## 7. Phase 1 — `cargo xbin sync` (metadata)

### 7.1 Driver steps

1. Locate and parse `rticx.toml` at the workspace root.
2. For each `[[application]]`:
   - run `cargo clean -p <package>` (metadata env changes are invisible to cargo
     fingerprints), then `cargo check` with `RTICX_XBIN_META_OUT=<dir>`;
   - the extension helper (detected at the macro level) writes `<app>.xbin.json`:
     package/target, source hash, local cores + core_ids, external cores,
     `#[cross_bin_spawn]` stubs, `#[cross_bin_task]` receivers, type paths,
     priorities, capacities.
3. Parse `ipc-types.toml`; compute canonical layout; validate the type subset.
4. Merge manifests and validate:
   - sender stub ↔ receiver task match: name, input type, capacity, target priority;
   - per-target priority lines disjoint across all remote sources and local tasks;
   - every referenced type exists in the IDL and is cross-core safe;
   - total per-task FIFO bytes fit each `(source → target)` region;
   - global core ids consistent across applications and `rticx.toml`.
5. Allocate per-task FIFO offsets deterministically:
   - sort by `(source_global, target_global, task_name)`;
   - align each FIFO to 8 bytes (indices cache-line padded in the runtime type);
   - depth = `capacity + 1` (ring buffer wastes one slot; effective capacity is exact
     because the dispatcher drains the FIFO directly);
   - hard error if the region overflows.
6. Write `target/rticx-xbin/system.json`.
7. Generate/update `ipc-types/` (types, marker impls, consts, asserts, `LAYOUT_HASH`);
   report if changed.

### 7.2 `system.json` contents

```jsonc
{
  "schema_version": 1,
  "rticx_generation": "0.x",
  "topology_hash": "...",
  "cores":  [ { "global_id": 0, "app": "app-m7", "local_index": 0 } ],
  "types":  [ { "name": "EncryptReq", "size": 12, "align": 4,
                "fields": [ { "name": "addr", "ty": "u32", "offset": 0 } ] } ],
  "tasks":  [ { "id": 1, "name": "EncryptTask",
                "receiver_core": 1, "spawner_cores": [0],
                "priority": 3, "capacity": 2,
                "input_type": "EncryptReq",
                "fifo": { "source": 0, "target": 1, "offset": 0,
                          "elem_size": 12, "depth": 3 } } ],
  "regions": [ { "source": 0, "target": 1,
                 "base_from_source": "0x30040000",
                 "base_from_target": "0x30040000", "size": 4096 } ],
  "doorbells": [ { "source": 0, "target": 1, "priority": 3, "line": 0 } ]
}
```

Determinism: identical inputs must produce byte-identical JSON. The JSON view is
also the future input for `cargo xbin visualize`.

### 7.3 Freshness / staleness

- The pass in phase 2 emits `include_str!` of `system.json` so rustc records it in
  dep-info and rebuilds when it changes.
- Generated code embeds a `const TOPOLOGY_HASH: u64 = ...;`; a mismatch between the
  app's own recomputed hash and the JSON hash is a hard compile error instructing the
  user to run `cargo xbin sync`.
- `cargo xbin build` always runs `sync` first. Plain `cargo build` is supported only
  after an explicit `sync`; an app whose source changed will fail until resynced.

---

## 8. Phase 2 — `cargo xbin build` / normal `cargo build`

The pass reads `system.json` (env var `RTICX_XBIN_SYSTEM` or the default project
path), filters to its own cores, and generates:

- **FIFO views** for every cross task at `base[core_view] + offset` via
  `rticx-xbin-rt`, with const addresses from the generated metadata. No local input
  queue, no forwarder, no task id/union.
- **Receiver side:**
  - the receiver struct is rewritten into
    `#[task(priority = <line>, core = N, task_trait = CrossBinTask, init = generated)]`,
    so the core pass generates its static and enforces the user's
    `impl CrossBinTask` (M3-T2);
  - one generated dispatcher per `(source → target, priority)` doorbell line:
    `#[task(binds = <doorbell IRQ>, priority = <line>, core = N, init = generated)]`
    implementing `RticTask`, whose `exec` drains each of its FIFOs directly
    until empty and calls the receiver task's `exec(input)`. (A later split may
    run a shared ISR at the highest priority and pend per-priority dispatchers;
    v1 binds the dispatcher to the doorbell IRQ directly.)
- **Sender side:** `Task::cross_spawn(input)`:
  - runtime global-core guard (`if current_global_core_id() != expected { return Err(Some(input)) }`);
  - enqueue into the task FIFO inside `__rticx_interrupt_free` (v1);
  - ring the doorbell for the target priority line;
  - semantics: `Ok(())`, `Err(None)` (enqueued but doorbell failed), `Err(Some(input))`
    (not enqueued), matching existing `cross_spawn`.
- **Init hooks** (through the backend):
  - `init_shared` on the owner core (zero indices, set magic/epoch);
  - `mark_ready(core)` at the end of each core's `post_init`;
  - arm the doorbell before setting ready;
  - optional `configure_shared_memory` (MPU).
- **Assertions:** layout consts from `ipc-types`, `TOPOLOGY_HASH`, FIFO address
  bounds and alignment.

Generated `#[task]` items are exactly what the core pass already consumes via
external `task_trait` paths, so no core changes are required.

---

## 9. Runtime crate (`rticx-xbin-rt`)

- Atomic SPSC ring for shared memory:
  - `AtomicUsize` head/tail (`portable-atomic`), publish with `Release`, consume with
    `Acquire`;
  - payload stored in place; element type must be `Copy`;
  - cache-line-padded indices to avoid false sharing;
  - `unsafe fn view_at(addr: usize) -> *mut Self` for fixed-address placement.
- `unsafe trait CrossCoreMessage: Copy + 'static {}` — implemented only by generated
  types.
- Ready/epoch helpers: atomic ready bitmap, epoch word for peer-reset detection.
- Deliberately separate from `rticx-spsc` so existing non-atomic single-binary
  semantics stay frozen.

---

## 10. Distribution/backend contract (`CrossBinBackend`)

Owned by the extension; implemented by the out-of-tree H7 distribution and by the
mock backend.

- `ipc_regions()` — per `(source, target)`, per-core base views and size.
- `configure_shared_memory()` — configure Normal, Non-cacheable, Shareable (MPU).
  Device/Strongly-ordered is forbidden (`ldrex`/`strex` are invalid there).
- optional `clean_range` / `invalidate_range` for cacheable-region fallback.
- `doorbell_setup(target, line, irq)`, `doorbell_ring(target, line)`,
  dispatcher IRQ type paths.
- `current_global_core_id()`.
- `init_shared()`, `mark_ready(core)`, `is_ready(core)`, `epoch()`.
- Boot sequencing is distribution-owned (H7: CM7 initializes the region and then
  releases CM4, e.g. via `RCC_GCR.BOOT_C2`); the ready bitmap makes boot order and
  peer reset safe. `cross_spawn` returns `Err` when the target is not ready.

---

## 11. Testing strategy

- **Unit:** IDL parse/layout golden tests; allocator determinism; every validation
  error path; codegen snapshots (sender stub, receiver dispatcher, FIFO views,
  topology-hash mismatch); JSON schema round-trip.
- **Host mock:** two threads over `rticx-xbin-rt` with a mock doorbell; scenarios:
  spawn → drain, FIFO full/backpressure, ready/epoch, simulated peer reset.
- **Cross-compile layout checks:** `thumbv7em-none-eabihf`, `thumbv6m-none-eabi`,
  optionally `riscv32imc`.
- **In-tree end-to-end:** two fixture apps under `fixtures/` built via
  `cargo xbin build`.
- **Acceptance (out-of-tree):** STM32H7 M7+M4 distribution under Renode with a
  real doorbell (HSEM/EXTI or IPCC where available) and non-cacheable shared SRAM.
- **CI:** separate advisory workflow for `experiments/multibinary` (fmt, clippy,
  tests, mock e2e). Root CI untouched.

---

## 12. Documentation and PlantUML

- **User guide (step by step):** install `cargo-xbin`; write `rticx.toml` and
  `ipc-types.toml`; declare receiver/sender tasks; `cargo xbin build`; run; common
  errors and fixes.
- **Architecture doc:** phases, JSON schema, memory/priority rules, locking, boot and
  reset, failure modes, cache/MPU notes.
- **PlantUML diagrams** (each small, `.puml` sources in `docs/diagrams/`; rendering
  is manual, done by the reviewer):
  1. Project topology — two binaries, global core ids, IPC regions.
  2. `sync` → merge/validate/allocate → `build` pipeline.
  3. Spawn flow timeline A→B — producer, FIFO, doorbell, ISR, dispatcher, exec.
  4. Memory/priority map — per-task FIFOs, priority lines, doorbells.
  5. Type pipeline — IDL → generated crate → layout asserts.

---

## 13. Success criteria

- Two fixture apps compile via `cargo xbin build` with **no root-workspace changes**.
- `system.json` is deterministic for identical inputs.
- Layout assertions pass on all supported targets.
- Host mock end-to-end test passes, including backpressure and staleness errors.
- A new user can reproduce the fixture from the docs and diagrams alone.
- STM32H7/Renode acceptance demo (M6, out-of-tree) runs cross-binary spawns.

---

## 14. Milestones and task checklist

Work proceeds one task at a time. Each task should be committed separately with tests.

### M0 — Skeleton, IDL, layout, generated crate

- [x] **M0-T1** Create `experiments/multibinary/` standalone workspace with empty
      crates (`proto`, `pass`, `rt`, `driver`, `mock`) and path deps on root crates.
      *Acceptance:* `cargo build` and `cargo test` succeed in the new workspace; root
      workspace untouched.
- [x] **M0-T2** Implement `ipc-types.toml` parser and 32-bit-safe type subset
      validation in `rticx-xbin-proto`.
      *Acceptance:* golden tests for accepted/rejected schemas with clear errors.
- [x] **M0-T3** Implement canonical layout engine (size/align/field offsets, LE,
      align cap 4) in `rticx-xbin-proto`.
      *Acceptance:* unit tests for nested structs, arrays, enums; host `size_of`
      agrees for the supported subset.
- [x] **M0-T4** Implement the `ipc-types` crate generator (types, marker impls,
      consts, asserts, `LAYOUT_HASH`).
      *Acceptance:* generated crate compiles on host and at least two embedded
      targets; assertions pass.
- [x] **M0-T5** Parse `rticx.toml` (`[[application]]`, `[ipc.regions]`) in
      `rticx-xbin-proto`; deterministic error messages.
      *Acceptance:* round-trip and error tests.
- [x] **M0-T6** Driver CLI skeleton: `cargo-xbin sync` and `cargo-xbin build`
      dispatch; create `target/rticx-xbin/` layout.
      *Acceptance:* `cargo xbin --help` works; no-op sync/build succeed.
- [x] **M0-T7** Docs skeleton: user guide outline, architecture outline, diagram
      stubs; first PlantUML source (project topology).
      *Acceptance:* `docs/` present with user guide/architecture outlines and the
      project-topology `.puml` source (rendering is manual, see §12).

### M1 — Metadata phase, merge, validation, allocation

- [x] **M1-T1** Define the `system.json` schema in `rticx-xbin-proto` (serde types,
      `schema_version`, generation, hashes).
      *Acceptance:* serialization round-trip; determinism test.
- [x] **M1-T2** Implement the pass metadata mode: `RTICX_XBIN_META_OUT` detection at
      the macro level; parse `#[app]` additions and cross-binary task declarations;
      write `<app>.xbin.json`.
      *Acceptance:* fixture app produces the expected manifest.
- [x] **M1-T3** Confirm and implement the task attribute syntax (`#[cross_bin_task]`,
      `#[cross_bin_spawn]`); strip args before core parse.
      *Acceptance:* attributes parsed; unknown/malformed keys produce precise errors;
      other distros only warn.
- [x] **M1-T4** Driver application enumeration + `cargo check` metadata invocation
      (including `cargo clean -p` for freshness).
      *Acceptance:* sync produces manifests for all fixture apps.
- [x] **M1-T5** Merge and validation: sender/receiver match, global priority
      disjointness per target, type existence, region fit, core-id consistency.
      *Acceptance:* one test per validation rule, positive and negative.
- [x] **M1-T6** Deterministic per-task FIFO address allocation and `system.json` emit.
      *Acceptance:* stable byte-identical output across runs; overflow errors name the
      task and region.
- [x] **M1-T7** Wire `ipc-types` generation into `sync`; change detection/report.
      *Acceptance:* editing the IDL updates the generated crate; unchanged IDL is a
      no-op.

### M2 — Runtime atomic FIFO and mock backend

- [x] **M2-T1** Implement `rticx-xbin-rt`: atomic SPSC ring, `CrossCoreMessage`,
      `view_at`, cache-line-padded indices.
      *Acceptance:* host tests for wrap, full/empty, `Copy` payloads; Release/Acquire
      correctness under threads.
- [x] **M2-T2** Ready/epoch helpers and tests, including peer-reset simulation.
      *Acceptance:* stale-epoch detection test.
- [x] **M2-T3** Mock backend: IPC regions over an `mmap`/array, mock doorbell,
      `current_global_core_id`.
      *Acceptance:* two threads exercise spawn→drain via raw FIFO.
- [x] **M2-T4** Cache/MPU documentation note in the backend trait and fallback hooks
      (no-op on host).
      *Acceptance:* rustdoc documents the Normal-Non-cacheable-Shareable rule and the
      Device-memory restriction.

### M3 — Codegen for one pair, one priority line

- [x] **M3-T1** Generate FIFO views and sender `cross_spawn` (interrupt-free,
      global-core guard, doorbell ring, error semantics).
      *Acceptance:* snapshot test of generated sender code.
- [x] **M3-T2** Generate the receiver doorbell ISR and dispatcher draining a single
      FIFO directly; `#[task(..., task_trait = CrossBinTask)]` shape accepted by the
      core pass.
      *Acceptance:* snapshot test; a minimal fixture expands and compiles.
- [x] **M3-T3** Generate init hooks (`init_shared`, `mark_ready`, arm doorbell) via
      `CrossBinBackend`.
      *Acceptance:* snapshot test; mock app reaches ready state in tests.
- [x] **M3-T4** Topology hash const + `include_str!` freshness + hard error on
      mismatch.
      *Acceptance:* editing `system.json` triggers rebuild; stale hash fails with the
      documented message.

### M4 — In-tree end-to-end fixture

- [ ] **M4-T1** Two-app fixture project under `fixtures/` (sender + receiver) using
      an IDL type and one cross-binary task.
      *Acceptance:* `cargo xbin build` succeeds.
- [ ] **M4-T2** Mock-runtime end-to-end test: spawn from app A, execute on app B's
      dispatcher, verify input value; full FIFO returns `Err(Some(input))`.
      *Acceptance:* test passes deterministically.
- [ ] **M4-T3** Negative tests: missing sync, hash mismatch, priority conflict,
      unknown type, region overflow.
      *Acceptance:* each fails with its documented error message.

### M5 — Multi-source/target, ready/epoch, complete docs

- [ ] **M5-T1** Support multiple source cores per target with disjoint priority
      lines; multiple doorbell lines.
      *Acceptance:* three-app fixture; priority planner tests.
- [ ] **M5-T2** Ready/epoch integration in generated spawn (target-not-ready error);
      peer reset recovery path documented and tested in the mock.
      *Acceptance:* tests for not-ready and post-reset spawns.
- [ ] **M5-T3** Complete user guide and architecture doc.
      *Acceptance:* a fresh reader can build the fixture following only the docs.
- [ ] **M5-T4** Complete PlantUML set (diagrams 1–5) as `.puml` sources.
      *Acceptance:* diagrams reviewed for small size/clarity (rendering is manual).
- [ ] **M5-T5** Separate advisory CI workflow for the experimental workspace.
      *Acceptance:* fmt, clippy, tests, mock e2e green.

### M6 — STM32H7 acceptance and extraction (separate effort)

- [ ] **M6-T1** Out-of-tree STM32H7 (M7+M4) distribution implementing
      `CrossBinBackend` (non-cacheable shared region, doorbell, boot release).
- [ ] **M6-T2** Renode acceptance demo: cross-binary spawn M7→M4 and M4→M7.
- [ ] **M6-T3** Extract `experiments/multibinary` into its own repository; resolve
      and remove `TODO(extract)` markers; add CI there.
- [ ] **M6-T4** Optional: re-evaluate promoting reimplemented internals into a shared
      RTICX crate, and the `Producer`-as-shared-resource API with SRP locking.

---

## 15. Risks and open items

| Risk / open item | Mitigation / note |
|---|---|
| `ipc-types` generation location policy | Recommended: root `ipc-types/`, kept in VCS; `sync` reports changes |
| Doorbell/IRQ availability per distro | Fallback: one doorbell per pair whose ISR pends multiple dispatchers |
| Cacheable-region fallback correctness | v1 recommends non-cacheable only; fallback hooks exist but are not the supported path |
| Staleness under plain `cargo build` | Only the changed app detects its own staleness; `cargo xbin build` is the safe path |
| Reimplemented internals drift from `rticx-sw-pass` | `TODO(extract)` markers; periodic manual comparison; possible later shared crate |
| Priority-line exhaustion on a target | Bounded by available priorities; validation errors are explicit; document limits |
| Future `Producer`/SRP API | Transport and lock are separated in codegen so migration is localized |
| Sender stub syntax (mirrored vs auto-generated) | Mirrored stub recommended for explicit phase-1 validation; can be relaxed later |

---

## 16. Glossary

- **IDL** — the type-only interface description (`ipc-types.toml`).
- **System view** — the driver-generated `system.json` describing the whole project.
- **Region** — distro-declared shared-memory block for one `(source → target)`
  direction, containing all per-task FIFOs of that pair.
- **FIFO** — per-task atomic SPSC ring inside a region, one element per spawn input.
- **Doorbell** — hardware signal (mailbox/IPI/HSEM) that pends the target's dispatcher.
- **Priority line** — a target-core priority reserved for tasks arriving from one
  specific remote source core.
- **Epoch** — shared counter used to detect peer reset and re-synchronize FIFOs.
