# RTICX Multi-Binary / Heterogeneous Multi-Core Extension — Implementation Plan

**Status:** in progress (M0–M5 complete; M5.5 next)
**Date:** 2026-09-27
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
5. The IDL models **data types only**. Task/priority/capacity topology lives in the
   receiver's native `#[sw_task]` declaration and is extracted by the new pass
   (producer applications declare nothing since M5.5); the driver is the single
   merger.
6. v1 uses an internal interrupt-free section inside the generated spawn. The
   transport is kept independent of the lock mechanism so a later `Producer`
   shared-resource API (SRP locking) can replace it in one place.

---

## 2. Locked decisions

| Topic | Decision |
|---|---|
| v1 scope | Native `#[sw_task]` control-plane spawn only; no async |
| Async | Explicitly out of scope; no extension of `rticx-async-pass`. The xbin pass requires the distribution's `swtasks` feature (sync `#[sw_task]` only) until a later milestone adds async |
| Cross-binary channels / shared resources | Out of scope |
| Task syntax | Cross-binary receivers are native `#[sw_task]` items with `impl RticSwTask`; no xbin-specific attributes or traits. In applications declaring `external_cores`, `spawn_by` names a **global** core (`∈ core_ids` → in-app, mapped back to a local index for sw-pass; `∈ external_cores` → cross receiver; else error); `core` stays a local index. `SpawnInput: rticx_xbin_rt::CrossCoreMessage` is enforced by a generated const assertion (M5.5) |
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
| Core changes | None required for M0–M4; M5 adds the native `core_ids` mapping to `rticx-core`. The generated multi-binary code keeps using only the frozen public surface (external `task_trait` paths, etc.) |
| Boot sequencing | Distribution-owned; pass emits `init_shared` / `mark_ready` / ready-check calls through the backend trait. Each core zeroes the FIFOs it produces (its outbound regions) before its `post_init` (M6-T1) |
| Producer declaration | The receiver's `spawn_by` is the only task-topology declaration. It is required for cross receivers, takes exactly one global producer core id (never an array; multi-producer tasks are rejected) and is resolved through `core_ids`/`external_cores`. Sender stubs are generated by the pass from `system.json` (M5.5) |
| Priority planning | No system-level priority shifting: on one target core each priority level belongs to exactly one origin core. Producer-vs-producer collisions are hard `sync` errors (merge); collisions with the target application's own sw/async tasks are hard `build` errors raised by the pass (the single-binary `check_disjoint_priorities` / `check_uniform_spawn_by` rule extended project-wide) |
| Future API | Remove `cross_spawn` in favor of a `Producer` type passed as a shared resource in `init`, allowing proper RTIC SRP locking for both single-binary and cross-binary spawns |

---

## 3. Scope

**In scope (v1):**

- One binary per homogeneous core group; each binary may use the existing
  single-binary multicore support internally.
- Cross-binary spawn of software tasks with input values, from any task on a source
  core to a task on a target core in another binary.
- Receiver declarations are the single task-topology source and use the native
  `#[sw_task]`/`RticSwTask` syntax; the pass generates the producer-side
  `cross_spawn` stubs, and a task has exactly one producer core (M5.5).
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
  #[sw_task] receiver (spawn_by)
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
- App syntax is the only source of tasks/priorities/capacities/visibility, and the
  receiver's native `#[sw_task]` declaration is the only task-topology declaration
  (producer applications declare nothing since M5.5).
- The driver is the only merger and address allocator.
- SPSC discipline: per task FIFO, single producer core (the receiver's `spawn_by`,
  one global core id, never an array; multi-producer tasks are rejected), single
  consumer dispatcher.
- Priority lines on a target core are disjoint between local and each remote source
  core (extends the existing `check_disjoint_priorities` / `check_uniform_spawn_by`
  rules to the project-wide view). One priority level belongs to exactly one origin
  core: producer-vs-producer collisions are hard `sync` errors, collisions with the
  target application's own sw/async tasks are hard `build` errors raised by the
  pass, and the driver never re-plans priorities.
- Producer applications do not declare their stubs: phase 2 generates
  `Task::cross_spawn` for every view task whose producer core belongs to the
  application (M5.5).

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
| fixtures | `fixtures/` | Fixture projects for in-tree end-to-end tests (`metadata`, `e2e`, `three-app`) |
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

- `core_ids = [g0, g1, ...]` — maps local core index `i` to the global core id
  `core_ids[i]`; length must equal `cores` (or `len == 1` when `cores` defaults to
  1). Defaults to the identity mapping `0..cores`, so `cores = N` alone keeps the
  historical behavior. Parsed and validated natively by `rticx-core` since M5.
- `external_cores = [g...]` — global ids of cores in other binaries visible to this
  application. Consumed (stripped) by the multi-binary pass; `rticx-core` does not
  know this key, so other distributions only see an unknown-key warning for it.

Task `core` syntax stays local (`0..cores`). Natively `spawn_by` is a local index
too; in an application that declares `external_cores`, the xbin pass resolves
`spawn_by` in the global namespace instead (see §6.5). The generated runtime core
checks and the multi-binary metadata translate through `core_ids` (M5).

### 6.5 Task syntax (native `#[sw_task]` since M5.5)

Receiver binary (the task runs here; the **only** declaration of the task):

```rust
#[sw_task(priority = 3, capacity = 2, core = 0, spawn_by = 1, init = generated)]
struct EncryptTask;

impl RticSwTask for EncryptTask {
    type SpawnInput = ipc_types::EncryptReq;
    fn exec(&mut self, input: Self::SpawnInput) { /* handler */ }
}
```

Producer binary (no declaration; phase 2 generates the stub from `system.json`):

```rust
// generated inside the `#[app]` module:
//   pub struct EncryptTask;
//   impl EncryptTask {
//       pub fn cross_spawn(input: ipc_types::EncryptReq) -> Result<(), Option<ipc_types::EncryptReq>>;
//   }
EncryptTask::cross_spawn(ipc_types::EncryptReq { addr: 0, len: 0, key: 0 });
```

- Cross-binary receivers are **ordinary native software tasks**: a `#[sw_task]`
  struct plus `impl RticSwTask` with `SpawnInput`. There are no xbin-specific
  attributes or traits; `#[cross_bin_task]`, `#[cross_bin_spawn]`, `CrossBinTask`
  and `CrossBinSpawn` are deleted outright (M5.5) with no compatibility shim,
  warning or migration error (the extension has no released users).
- `spawn_by` is the producer core. Natively it is a local index; in an application
  that declares `external_cores`, the pass resolves it in the **global** namespace:
  - value ∈ `core_ids` → the task is spawned by one of the application's own cores
    (in-app cross-core task); the pass maps it back to the local index for sw-pass.
  - value ∈ `external_cores` (validated disjoint from `core_ids`) → the task is a
    cross-binary receiver; that global core is its single producer.
  - anything else → error naming the unknown core.
  With the identity `core_ids` default, global ids equal local indexes, so existing
  syntax is unchanged.
- `core` stays a **local** index (`0..cores`, default `0`, matching RTIC's task
  `core`); a task always executes in this application.
- v1 keeps one SPSC FIFO and one priority line per task, so a task has exactly one
  producer core; multi-producer tasks are rejected. Phase 1 infers each producer
  application from `spawn_by`.
- Defaults: `priority = 1` (cross receivers require `priority >= 1`; native
  `#[sw_task]` defaults to 0), `capacity = 1` (minimum 1), `init = generated`.
  Unknown, duplicate and multi-segment keys, malformed values, an out-of-range
  receiver `core`, and `external_cores` overlapping the application's own
  `core_ids` are hard errors. `shared = [...]` is passed through to the core pass,
  which computes the SRP ceilings.
- The receiver's `SpawnInput` must implement `rticx_xbin_rt::CrossCoreMessage`; the
  pass emits a const assertion for it instead of relying on a trait bound in the
  generated `RticSwTask`.
- Phase 2 generates `pub struct <Task>;` plus
  `Task::cross_spawn(input) -> Result<(), Option<Input>>` in the producer
  application's `#[app]` module for every view task whose `spawner_core` belongs to
  that application, matching the existing `cross_spawn` error semantics. The input
  type is the receiver's IDL type, reached through the distribution's
  `ipc_types_path`, so producer applications declare nothing and only add
  `ipc-types` as a dependency.
- The pass is bound **before** `rticx-sw-pass` and requires the distribution's
  `swtasks` feature: sw-pass emits the generated `RticSwTask` trait and the
  receiver is compiled through the core pass's `task_trait` mechanism. Async
  cross-binary tasks are out of scope until a later milestone.

---

## 7. Phase 1 — `cargo xbin sync` (metadata)

### 7.1 Driver steps

1. Locate and parse `rticx.toml` at the workspace root.
2. For each `[[application]]`:
   - run `cargo clean -p <package>` (metadata env changes are invisible to cargo
     fingerprints), then `cargo check` with `RTICX_XBIN_META_OUT=<dir>`;
   - the extension helper (detected at the macro level) writes `<app>.xbin.json`:
     package/target, source hash, local cores + core_ids, external cores, cross
     receivers (native `#[sw_task]` items), their `SpawnInput` type paths,
     priorities, capacities. There are no sender declarations (M5.5): the driver
     infers each task's single producer application from the receivers' `spawn_by`.
3. Parse `ipc-types.toml`; compute canonical layout; validate the type subset.
4. Merge manifests and validate:
   - every receiver's `spawn_by` names a single existing producer in another
     application, is listed in the receiver's `external_cores`, and the producer
     application lists the receiver's core in its `external_cores`;
   - per-target priority lines are disjoint across producer cores; a collision is a
     hard error (the single-binary rule extended project-wide — the driver plans
     and validates the lines, it never shifts priorities). Manifests carry only
     cross-binary receivers, so collisions with the target application's own
     sw/async tasks are not visible here and are checked by the pass at `build`
     (see §8);
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
                "receiver_core": 1, "spawner_core": 0,
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

v1 keeps one producer core per task, so every `tasks[]` entry has a single
`spawner_core` and one `fifo` (one FIFO per `(task, source → target)` direction),
while a target core may carry lines from several producer cores (one doorbell
entry per `(source, target, priority)`). `core_ids` and `external_cores` stay
arrays; only `spawn_by`/`spawner_core` are singular.

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
path), filters to its own cores, and generates for every application listed in the
view — including applications that declare no cross tasks, whose endpoints still
need the generated sender stubs and init hooks, so the phase-2 view-load gate no
longer requires cross-task declarations (M5.5):

- **FIFO views** for every cross task at `base[core_view] + offset` via
  `rticx-xbin-rt`, with const addresses from the generated metadata. No local input
  queue, no forwarder, no task id/union.
- **Receiver side:**
  - the receiver's native `#[sw_task]` is rewritten into
    `#[task(priority = <line>, core = N, task_trait = RticSwTask, init = generated)]`
    (preserving `shared`), so the core pass generates its static and enforces the
    user's `impl RticSwTask` (M5.5); the native `spawn` queue is not generated for
    cross receivers — their inputs arrive through the FIFO;
  - one generated dispatcher per `(source → target, priority)` doorbell line:
    `#[task(binds = <doorbell IRQ>, priority = <line>, core = N, init = generated)]`
    implementing `RticTask`, whose `exec` drains each of its FIFOs directly
    until empty and calls the receiver task's `exec(input)`. (A later split may
    run a shared ISR at the highest priority and pend per-priority dispatchers;
    v1 binds the dispatcher to the doorbell IRQ directly.)
- **Priority-line validation (M6-T1):** the pass runs before the sw/async passes,
  so it analyzes the application's raw `#[sw_task]`/`#[async_task]` declarations
  (`priority`, `core`, `spawn_by` mapped through `core_ids`) together with the
  cross-bin receivers from the whole view. On each target core one priority level
  belongs to exactly one origin core: reject a cross-bin receiver that collides
  with a core-local sw/async task, with an in-app `spawn_by` task, or with a
  cross-bin receiver from a different producer core. Receivers from the same
  producer may share a line. Violations are hard compile errors on the receiver
  declaration naming the conflicting task; `sync` cannot see local tasks, so this
  check runs at `build` only.
- **Message-type assertion (M5.5):** the pass emits a const assertion that each
  receiver's `<Task as RticSwTask>::SpawnInput` implements
  `rticx_xbin_rt::CrossCoreMessage`, so the guarantee lives in generated code
  instead of the trait definition.
- **Sender stubs (M5.5):** for every view task whose `spawner_core` belongs to
  this application, the pass generates `pub struct <Task>;` and the `cross_spawn`
  impl below inside the `#[app]` module; the producer source never mentions the
  task. A name collision with a user item is a dedicated compile error.
- **Sender API:** `Task::cross_spawn(input)`:
  - runtime global-core guard (`if current_global_core_id() != expected { return Err(Some(input)) }`);
  - enqueue into the task FIFO inside `__rticx_interrupt_free` (v1);
  - ring the doorbell for the target priority line;
  - semantics: `Ok(())`, `Err(None)` (enqueued but doorbell failed), `Err(Some(input))`
    (not enqueued), matching existing `cross_spawn`.
- **Init hooks** (through the backend):
  - `init_shared` on the owner core (set magic/epoch);
  - every core zeroes the ring indices of the FIFOs it **produces** (the outbound
    half of each region it is the source of) at `BeforePostInit`, before its
    `post_init` can spawn; topologies whose owner core is not an endpoint of a
    region initialize correctly (M6-T1);
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
  error path; `spawn_by` resolution tests (identity-default back-compat, in-app
  global→local mapping, external classification, unknown-core error); native-syntax
  extraction (`#[sw_task]` + `impl RticSwTask` → manifest, `SpawnInput`); the
  `SpawnInput: CrossCoreMessage` const assertion (a non-conforming input must fail
  to compile) and `shared` pass-through; priority-line validation tests (cross-bin
  vs core-local sw, vs `#[async_task]`, vs in-app `spawn_by`, cross-producer
  collision error, same-producer sharing, deterministic doorbell numbering);
  codegen snapshots (auto-generated sender stub, receiver dispatcher, FIFO views,
  topology-hash mismatch); JSON schema round-trip.
- **Host mock:** two threads over `rticx-xbin-rt` with a mock doorbell; scenarios:
  spawn → drain, FIFO full/backpressure, ready/epoch, simulated peer reset.
- **Cross-compile layout checks:** `thumbv7em-none-eabihf`, `thumbv6m-none-eabi`,
  optionally `riscv32imc`.
- **In-tree end-to-end:** the two- and three-application fixture projects
  (`fixtures/e2e`, `fixtures/three-app`) built via `cargo xbin build`. The M4-T2
  runtime harness (`crates/rticx-xbin-pass/tests/e2e_runtime.rs`) is extended to
  the three-app fixture: three applications expanded in one process over a shared
  `MockSystem`, two producer cores each spawning onto the receiver core on two
  doorbell lines, asserting auto-stub generation for an application with zero
  declarations, per-producer `BeforePostInit` FIFO zeroing of a region whose owner
  is not an endpoint, ordered draining of both FIFOs and per-source backpressure.
- **Acceptance (out-of-tree):** STM32H7 M7+M4 distribution under Renode with a
  real doorbell (HSEM/EXTI or IPCC where available) and non-cacheable shared SRAM.
- **CI:** separate advisory workflow for `experiments/multibinary` (fmt, clippy,
  tests, mock e2e). Root CI untouched.

---

## 12. Documentation and PlantUML

- **User guide (step by step):** install `cargo-xbin`; enable the distribution's
  `swtasks` feature; write `rticx.toml` and `ipc-types.toml`; declare receivers as
  native `#[sw_task]` items (`impl RticSwTask`, `SpawnInput`) whose `spawn_by`
  names the producer core (producer stubs are generated by the pass; producer
  sources only call `Task::cross_spawn`); `cargo xbin build`; run; common errors
  and fixes. Async cross-binary tasks are documented as not yet supported.
- **Architecture doc:** phases, JSON schema, memory/priority rules, locking, boot and
  reset, failure modes, cache/MPU notes, and the native `#[sw_task]` declaration
  model (global `spawn_by` resolution for apps declaring `external_cores`).
- **PlantUML diagrams** (each small, `.puml` sources in `docs/diagrams/`; rendering
  is manual, done by the reviewer):
  1. Project topology — two binaries, global core ids, IPC regions.
  2. `sync` → merge/validate/allocate → `build` pipeline.
  3. Spawn flow timeline A→B — producer, FIFO, doorbell, ISR, dispatcher, exec.
  4. Memory/priority map — per-task FIFOs, priority lines, doorbells.
  5. Type pipeline — IDL → generated crate → layout asserts.

---

## 13. Success criteria

- The two- and three-application fixture projects compile via `cargo xbin build`;
  the extension itself adds no root-workspace changes beyond the M5 native
  `core_ids` mapping.
- The three-app runtime test passes: two producer cores spawn onto one target core
  over two doorbell lines, with per-producer FIFO initialization and the
  auto-generated sender stubs for an application declaring no cross tasks.
- Cross-binary receivers use the native `#[sw_task]`/`RticSwTask` syntax with
  `spawn_by` resolved through `core_ids`/`external_cores`; no xbin-specific task
  attribute or trait remains.
- `system.json` is deterministic for identical inputs.
- Layout assertions pass on all supported targets.
- Host mock end-to-end test passes, including backpressure and staleness errors.
- A binary with a non-identity `core_ids` mapping enforces its runtime core checks
  against the global ids (M5).
- A new user can reproduce the fixture from the docs and diagrams alone.
- STM32H7/Renode acceptance demo (M7, out-of-tree) runs cross-binary spawns.

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

- [x] **M4-T1** Two-app fixture project under `fixtures/` (sender + receiver) using
      an IDL type and one cross-binary task.
      *Acceptance:* `cargo xbin build` succeeds.
- [x] **M4-T2** Mock-runtime end-to-end test: spawn from app A, execute on app B's
      dispatcher, verify input value; full FIFO returns `Err(Some(input))`.
      *Acceptance:* test passes deterministically.
- [x] **M4-T3** Negative tests: missing sync, hash mismatch, priority conflict,
      unknown type, region overflow.
      *Acceptance:* each fails with its documented error message.

### M5 — Native global core ids in `rticx-core` (`core_ids`)

Today `rticx-core` and the software/async passes assume a binary occupies the
contiguous core ids `0..cores`: `SwPassBackend::current_core_id()` is compared
against the raw task `core`/`spawn_by` literals, and `pacs`/entry points are
indexed by the local core index. That is only correct while a binary's runtime
(hardware) core ids are exactly its local indices.

This milestone makes the local → global mapping native to the core pass:

- `#[app(device = …, cores = N)]` keeps today's behavior: local index `i` maps to
  global id `i` (the identity `0..N`).
- `#[app(device = …, cores = N, core_ids = [g0, g1, …])]` maps local index `i` to
  global id `core_ids[i]`; the length must equal `cores` and the ids must be
  unique.

Task `core`/`spawn_by` syntax stays local (matching RTIC); "global id" is what
the runtime core checks (`current_core_id()`), `external_cores` and the
multi-binary system view use. This is the only planned root-workspace change of
the extension, so follow `COMPATIBILITY.md`: the key is additive and optional,
and any field/signature addition that `cargo-semver-checks` classifies as
breaking triggers the coordinated generation bump.

- [x] **M5-T1** Parse and validate `core_ids` in `rticx-core`: add
      `RticAttr::take_u32_array` (promote the xbin-pass helper marked
      `TODO(extract)`), add `AppArgs.core_ids` defaulting to `(0..cores)` plus a
      `global_core(local)` accessor, validate length/`cores` equality and
      uniqueness, and add `core_ids` to the supported app args so it no longer
      warns.
      *Acceptance:* parser tests for the identity default, an explicit mapping, a
      length mismatch and duplicate ids; `cd rticx-core && cargo test`.
- [x] **M5-T2** Translate local → global in the generated runtime core checks:
      read `core_ids` in `AppParameters::parse` (shared by the sw/async passes)
      and emit `core_ids[core]` / `core_ids[spawn_by]` in the `spawn` /
      `cross_spawn` guards instead of the local literals; expose the mapping on
      `SubApp`/`App` for backends that need it.
      *Acceptance:* a pass test with a mock backend whose `current_core_id()`
      returns non-identity ids (e.g. `core_ids = [1, 2]`) compiles and passes;
      existing multicore codegen tests are unchanged.
- [x] **M5-T3** Integrate the multi-binary pass: stop consuming/stripping
      `core_ids` (the core pass owns it now), keep `external_cores` pass-owned,
      and read the mapping from the parsed `App` in codegen mode.
      *Acceptance:* `cargo xbin sync`/`build` stays green on the fixtures and a
      `cores = 2, core_ids = [1, 2]` app compiles without an unknown-arg warning,
      with the runtime core checks using the global ids.
- [x] **M5-T4** Update the frozen-surface documentation and consumers: wiki
      `#[app]` syntax page, `COMPATIBILITY.md` checklist decision (additive vs
      generation bump), and adoption notes for the out-of-tree distributions
      (rp2040 ids are the identity `0/1`; riscv is single-core).
      *Acceptance:* `make fmt all` green; the default and explicit mappings are
      documented side by side; the semver outcome is recorded.

### M5.5 — Native `#[sw_task]` cross-binary declarations

- [ ] **M5.5-T1** Native receiver syntax: parse cross receivers from `#[sw_task]`
      plus `impl RticSwTask` (`SpawnInput`) instead of `#[cross_bin_task]`. In
      applications declaring `external_cores`, resolve `spawn_by` in the global
      namespace (`∈ core_ids` → in-app task, mapped back to a local index for
      sw-pass; `∈ external_cores` → cross receiver; else error), keeping `core` a
      local index. Delete `#[cross_bin_task]`, `#[cross_bin_spawn]`,
      `CrossBinTask` and `CrossBinSpawn`; make the manifest field `spawn_by: u32`
      and bump the manifest/view schema versions.
      *Acceptance:* parse tests for identity-default back-compat, in-app
      global→local mapping, external classification and unknown-core errors; the
      `cross_bin_task`/`cross_bin_spawn`/`CrossBinTask`/`CrossBinSpawn` symbols no
      longer exist (`CrossBinBackend` stays — it is the backend contract).
- [ ] **M5.5-T2** Codegen and stubs: before the native parser runs, metadata mode
      rewrites in-app global `spawn_by` values to local indexes and strips
      `spawn_by` on cross receivers, so sw-pass never sees an external id; codegen
      mode injects `#[task(..., task_trait = RticSwTask, init = generated)]`,
      preserves `shared` (the core pass computes ceilings) and emits the const
      assertion `SpawnInput: rticx_xbin_rt::CrossCoreMessage`. Bind the pass
      before `rticx-sw-pass`, delete the sender machinery and generate the
      producer stubs, and remove the view-load gate.
      *Acceptance:* codegen snapshots; a receiver with `shared` compiles; a
      producer application with zero declarations gets its stubs; the documented
      `swtasks` requirement (missing the feature leaves `RticSwTask` undefined
      and fails to compile).
- [ ] **M5.5-T3** Migrate the fixtures (`e2e`, `metadata`), driver/negative tests,
      user guide, architecture doc and the wiki note to the native syntax;
      document the `swtasks` requirement and defer async explicitly.
      *Acceptance:* all fixture builds and workspace tests green; no
      xbin-specific task attribute remains in code or docs.

### M6 — Multi-source/target, ready/epoch, complete docs

- [ ] **M6-T1** Support multiple source cores per target with disjoint priority
      lines and multiple doorbell lines, building on the native `#[sw_task]`
      syntax (M5.5).
      - Tasks from different producer cores on one target must declare disjoint
        priorities; producer-vs-producer collisions are hard `sync` errors and the
        driver never shifts priorities (the single-binary rule extended
        project-wide).
      - The pass validates priority lines at `build`: it runs pre-core and
        analyzes the app's raw `#[sw_task]`/`#[async_task]` declarations
        (`priority`, `core`, `spawn_by` mapped through `core_ids`) together with
        the cross-bin receivers from the view. One priority level per target core
        belongs to exactly one origin core; a collision with a core-local sw/async
        task, an in-app `spawn_by` task or a receiver from another producer is a
        hard compile error. `sync` catches producer-vs-producer collisions only.
      - FIFO index initialization moves from the owner core to each FIFO's
        producer core (`BeforePostInit`), so topologies whose owner core is not
        an endpoint of a region work.
      *Acceptance:* new three-app fixture (two producer binaries, one receiver
      binary, two doorbell lines); the M4-T2 runtime harness is extended to it —
      three applications in one process, spawns from both producer cores drain
      through their own doorbell lines, and the non-owner producer initializes
      its region; priority-line validation tests.
- [ ] **M6-T2** Ready/epoch integration in generated spawn (target-not-ready error);
      peer reset recovery path documented and tested in the mock.
      *Acceptance:* tests for not-ready and post-reset spawns.
- [ ] **M6-T3** Complete user guide and architecture doc, including the native
      `#[sw_task]` declaration model and the pass-generated sender stubs of
      M5.5.
      *Acceptance:* a fresh reader can build the fixture following only the docs.
- [ ] **M6-T4** Complete PlantUML set (diagrams 1–5) as `.puml` sources.
      *Acceptance:* diagrams reviewed for small size/clarity (rendering is manual).
- [ ] **M6-T5** Separate advisory CI workflow for the experimental workspace.
      *Acceptance:* fmt, clippy, tests, mock e2e green.

### M7 — STM32H7 acceptance and extraction (separate effort)

- [ ] **M7-T1** Out-of-tree STM32H7 (M7+M4) distribution implementing
      `CrossBinBackend` (non-cacheable shared region, doorbell, boot release).
- [ ] **M7-T2** Renode acceptance demo: cross-binary spawn M7→M4 and M4→M7.
- [ ] **M7-T3** Extract `experiments/multibinary` into its own repository; resolve
      and remove `TODO(extract)` markers; add CI there.
- [ ] **M7-T4** Optional: re-evaluate promoting reimplemented internals into a shared
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
| Priority-line exhaustion on a target | No automatic shifting (M6-T1): producer cores must declare disjoint priorities; collisions are hard errors naming both tasks (`sync` for producer-vs-producer, the pass at `build` for collisions with the target app's own sw/async tasks); the limits are documented |
| Future `Producer`/SRP API | Transport and lock are separated in codegen so migration is localized |
| Auto-generated sender stub name collision | The generated `pub struct <Task>` can collide with a user item in the `#[app]` module; the pass emits a dedicated compile error naming the task (M5.5) |
| xbin-pass depends on the `swtasks` feature | `RticSwTask` is generated by sw-pass and the receiver compiles through the core pass's `task_trait`; without the feature the generated code fails on the unresolved trait, and the requirement is documented (M5.5) |
| Global `spawn_by` namespace in apps declaring `external_cores` | The native local-index reading still applies where `external_cores` is absent; the identity `core_ids` default makes both readings identical, and the rule is documented per application (M5.5) |
| Local vs global core ids in out-of-tree distros | The identity default keeps rp2040/riscv correct; they adopt `core_ids` in follow-up PRs per `COMPATIBILITY.md` after the M5 release |

---

## 16. Glossary

- **IDL** — the type-only interface description (`ipc-types.toml`).
- **System view** — the driver-generated `system.json` describing the whole project.
- **Region** — distro-declared shared-memory block for one `(source → target)`
  direction, containing all per-task FIFOs of that pair.
- **FIFO** — per-task atomic SPSC ring inside a region, one element per spawn input.
- **Doorbell** — hardware signal (mailbox/IPI/HSEM) that pends the target's dispatcher.
- **Priority line** — a target-core priority reserved for tasks arriving from one
  specific remote source core; producer cores on one target must hold disjoint
  lines (validated at `sync` for producer-vs-producer, by the pass at `build` for
  collisions with the target application's own sw/async tasks; never re-planned).
- **Cross-binary receiver** — a native `#[sw_task]` in the consuming application
  whose `spawn_by` names an external (global) core; it is the only declaration of
  the cross-binary task (M5.5).
- **`SpawnInput`** — the `RticSwTask` associated type carrying a software task's
  input; for cross-binary receivers it must implement `CrossCoreMessage`, enforced
  by a generated const assertion (M5.5).
- **Sender stub** — the `pub struct <Task>;` plus `Task::cross_spawn(input)` API
  the pass generates inside a producer application's `#[app]` module for every
  system-view task whose `spawner_core` is one of its cores (M5.5); producer
  sources do not declare it.
- **Local core index** — a core's position in an application's `0..cores`, as used
  by task `core` syntax and `pacs` (and by `spawn_by` in applications that do not
  declare `external_cores`).
- **Global core id** — the project-wide id used by runtime core checks,
  `external_cores` and the system view: local index `i` maps to `core_ids[i]`
  (identity by default, M5).
- **Epoch** — shared counter used to detect peer reset and re-synchronize FIFOs.
