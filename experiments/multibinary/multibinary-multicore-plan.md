# RTICX Multi-Binary / Heterogeneous Multi-Core Extension — Architecture Reference

**Status:** M0–M6.5 complete (skeleton, IDL, layout, runtime, codegen, fixtures,
native `core_ids`, native `#[sw_task]` receivers, dispatcher pool + doorbell
routing, multi-source, ready/epoch, docs), **M6.9-T1..T9** (distro capability
binding, manifest schema 2, `[ipc.regions]` removal, pool graph + shared
budget, `system.json` schema 2, pool-aware codegen/runtime, linked-ELF
verification, pool-panel visualization, docs and fixtures) complete. Remaining
work: **M7** (STM32H7
acceptance + extraction) and **M8-T1** (advisory CI), see
[§13](#13-remaining-milestones).
**Last updated:** 2026-10-04 (M6.9-T9; docs and fixtures).
**Target:** experimental, in-tree development, designed for later extraction into
its own repository (M7-T3).

This is the reference document for the extension. It describes the architecture
**as built**, where each part lives under `experiments/multibinary/`, what the
fixtures test, and the tasks that remain. Completed milestones are not repeated
here — their tasks and acceptance evidence are in git history. New work is
executed by loading this document and implementing one task of
[§13](#13-remaining-milestones).

---

## 1. Purpose

Extend RTICX from "single binary, possibly multi-core" to "multi-binary,
multi-core" projects where:

- each group of homogeneous cores shares a single binary and continues to use the
  existing single-binary multicore framework;
- heterogeneous core groups run separate binaries;
- cross-core task spawns between binaries are carried over shared-memory FIFOs with
  a hardware doorbell, reusing the RTICX task/dispatcher programming model.

End goal and acceptance platform: STM32H7 (Cortex-M7 + Cortex-M4) under an existing
Renode simulation ([§11.1](#111-renode-acceptance-harness-stm32h7-m7m4)), delivered
by an out-of-tree distribution.

---

## 2. Locked decisions

| Topic | Decision |
|---|---|
| v1 scope | Native `#[sw_task]` control-plane spawn only; no async |
| Async | Explicitly out of scope; no extension of `rticx-async-pass`. The xbin pass requires the distribution's `swtasks` feature (sync `#[sw_task]` only) until a later milestone adds async |
| Cross-binary channels / shared resources | Out of scope |
| Task syntax | Cross-binary receivers are native `#[sw_task]` items with `impl RticSwTask`; no xbin-specific attributes or traits. In applications declaring `external_cores`, `spawn_by` names a **global** core (`∈ core_ids` → in-app, mapped back to a local index for sw-pass; `∈ external_cores` → cross receiver; else error); `core` stays a local index. `SpawnInput: rticx_xbin_rt::CrossCoreMessage` is enforced by a generated const assertion |
| Receiver transport | Per-task shared FIFO, direct dispatcher drain; the per-pair router only routes task ids and pends dispatchers — it never forwards payloads (no forwarder ISR) |
| Sender-side lock | v1: `core_local_interrupt_free` inside generated spawn. Future: `Producer` as a shared resource with SRP lock |
| Type discovery | TOML IDL at workspace root, types only; driver generates a shared `ipc-types` crate |
| Type subset v1 | 32-bit-safe only: `u8..u32`, `i8..i32`, `f32`, `[T; N]`, nested messages, optional `repr(u32)` enums. No `u64/i64/f64`, `usize`, `bool`, pointers, references |
| Layout guarantee | Generated `#[repr(C)]` types + canonical layout consts + per-target compile-time assertions |
| FIFO ordering | New atomic SPSC queue (`AtomicUsize` head/tail via `portable-atomic`, Release/Acquire) — not `rticx-spsc` |
| Cache policy | Distribution configures the shared region Normal, Non-cacheable, Shareable; optional clean/invalidate fallback |
| Phase-2 metadata | Driver emits one JSON system view; each binary's pass parses the whole view and filters by its cores |
| Driver UX | `cargo xbin sync`, `cargo xbin build` (= sync + build, `--verify-elf` to check the linked ELFs) and `cargo xbin verify` (check the plain-`cargo build` output); granular per-app cargo/clippy workflows remain possible after an explicit `sync` |
| Pass internals | The external pass reimplements needed internals for now, marked `TODO(extract): ...`; may later be promoted/shared with `rticx-sw-pass` |
| Development home | In-tree experimental workspace under `experiments/multibinary/`; extract to its own repo later |
| Core changes | M5 added the native `core_ids` mapping to `rticx-core`. The generated multi-binary code keeps using only the frozen public surface (external `task_trait` paths, etc.) |
| Boot sequencing | Distribution-owned; pass emits `init_shared` / `mark_ready` / ready-check calls through the backend trait. Each core zeroes the FIFOs it produces (its outbound regions) before its `post_init` |
| IPC memory ownership | Distro-owned (M6.9): the project declares **no** IPC addresses or regions. The distro exposes a capability binding — which core pairs can reach each other, through which pool, with per-core views and a shared per-dual budget — and ships the linker reservations. One shared pool per unordered core pair; **no** project-level override |
| Link-time verification | `cargo xbin build --verify-elf` / `cargo xbin verify` parse the linked ELF with `object`: no `SHF_ALLOC` section or stack bound symbol may intersect a pool view of the application's cores, and distro pool bound symbols must match `system.json` (M6.9-T7) |
| Producer declaration | The receiver's `spawn_by` is the only task-topology declaration. It is required for cross receivers, takes exactly one global producer core id (never an array; multi-producer tasks are rejected) and is resolved through `core_ids`/`external_cores`. Sender stubs are generated by the pass from `system.json` |
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
  `cross_spawn` stubs, and a task has exactly one producer core.
- Type-only IDL → generated types crate; driver-generated JSON system view.
- Driver commands `cargo xbin sync`, `cargo xbin build` and `cargo xbin verify`.
- Distro bindings: IPC pools, doorbells, cache/MPU policy, ready/epoch.

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
Application sources (Rust)          IDL (types only)         Project + distro
  #[app(core_ids, external_cores)]     ipc-types.toml          rticx.toml (apps, core_ids)
  #[sw_task] receiver (spawn_by)                               distro capability binding (pools, M6.9)
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
     -> sender stubs / per-pair doorbell routers + line dispatchers / FIFO views
     -> #[task(...)] items the core pass already understands
```

### Design invariants

- The IDL is the only source of shared data types.
- App syntax is the only source of tasks/priorities/capacities/visibility, and the
  receiver's native `#[sw_task]` declaration is the only task-topology declaration
  (producer applications declare nothing).
- The driver is the only merger and address allocator.
- SPSC discipline: per task FIFO, single producer core (the receiver's `spawn_by`,
  one global core id, never an array; multi-producer tasks are rejected), single
  consumer dispatcher.
- Priority lines on a target core are disjoint between local and each remote source
  core. One priority level belongs to exactly one origin core: producer-vs-producer
  collisions are hard `sync` errors, collisions with the target application's own
  sw/async tasks are hard `build` errors raised by the pass, and the driver never
  re-plans priorities.
- Producer applications do not declare their stubs: phase 2 generates
  `Task::cross_spawn` for every view task whose producer core belongs to the
  application.

### Dispatch chain (as built)

```
Task::cross_spawn(input)
  → runtime global-core guard
  → ReadyCache target-ready gate
  → enqueue element in the per-task FIFO (inside __rticx_interrupt_free)
  → __rticx_xbin_ring_{source}_{target}(task_id)      (per-pair doorbell)
      → target's doorbell router IRQ                   (one per (source → target) pair)
          → push task id into its priority line's ready queue
          → __rticx_local_irq_pend[_core{N}](line IRQ)  (sw-pass pend fn)
              → line dispatcher IRQ bound to an ipc_dispatchers entry
                  → drain the ready queue, per task drain its FIFO until empty
                  → receiver task's exec(input)
```

`Ok(())` means enqueued and notified; `Err(None)` means enqueued but the
notification failed (recovered by the next spawn's notification, since the dispatcher
drains to empty); `Err(Some(input))` means not enqueued (FIFO full, wrong core, or
target not ready).

---

## 5. Implementation map (`experiments/multibinary/`)

### 5.1 Workspace

Standalone Cargo workspace at `experiments/multibinary/` (its own `[workspace]`,
deliberately outside the root workspace to keep release-plz, generation checks and
core CI untouched). It path-depends on the root crates with caret versions tracking
the root generation.

| Crate | Path | Purpose |
|---|---|---|
| `rticx-xbin-proto` | `crates/rticx-xbin-proto/` | IDL parser, canonical layout engine, merge + validation, JSON schemas, canonical FIFO image |
| `rticx-xbin-pass` | `crates/rticx-xbin-pass/` | `RticPass` implementation, metadata mode + codegen mode |
| `rticx-xbin-rt` | `crates/rticx-xbin-rt/` | Atomic cross-core SPSC queue, marker trait, ready/epoch helpers, backend contract |
| `rticx-xbin-driver` | `crates/rticx-xbin-driver/` | `cargo-xbin` subcommand (`sync`, `build`, `verify`) |
| `rticx-xbin-mock` | `crates/rticx-xbin-mock/` | `MockSystem` runtime for host tests |
| `mock-pac` | `crates/mock/mock-pac/` | Minimal PAC for the mock `#[app]` |
| `xbin-mock-capability` | `crates/mock/xbin-mock-capability/` | Fixture IPC capability table shared by the mock distributions |
| `xbin-mock-distro` | `crates/mock/xbin-mock-distro/` | Mock `#[app]` distribution (core pass + xbin pass + sw pass) |
| `xbin-mock-runtime` | `crates/mock/xbin-mock-runtime/` | Runtime wrapper the generated mock code calls (`xbin_rt`, backend handle) |

### 5.2 Key modules

| Crate | Module | Responsibility |
|---|---|---|
| `rticx-xbin-proto` | `idl.rs` | Parse `ipc-types.toml`; validate the 32-bit-safe type subset |
| | `layout.rs` | Canonical little-endian layout (natural alignment capped at 4) |
| | `codegen.rs` | Generate the `ipc-types` crate (types, marker impls, consts, asserts, `LAYOUT_HASH`) with change-detecting writes |
| | `project.rs` | Parse `rticx.toml` (`[[application]]`, `core_ids`; rejects the removed `[ipc.regions]`) |
| | `manifest.rs` | `<app>.xbin.json` schema (written by the pass in metadata mode: receivers + per-core distro capability binding) |
| | `merge.rs` | Merge manifests, validate topology/priority/type/core-id rules, match the distro pool graph, reject impossible links, allocate FIFOs in the shared pool budgets |
| | `alloc.rs` | Emit the sealed system view (FIFO offsets, doorbell lines, hashes) |
| | `system.rs` | `system.json` schema (`seal`, `canonical_topology_text`, `verify_topology_hash`) |
| | `fifo.rs` | Canonical in-region FIFO image and sizing (shared with the runtime) |
| | `hash.rs` | Deterministic FNV-1a 64-bit hash (source/layout/topology) |
| `rticx-xbin-pass` | `lib.rs` | `XbinPass`: mode detection (`RTICX_XBIN_META_OUT` / `RTICX_XBIN_SYSTEM`), view load, pass orchestration |
| | `parse.rs` | `#[app]` args (`external_cores`, `ipc_dispatchers`; reads `core_ids`), native `#[sw_task]` receiver extraction + `spawn_by` resolution, receiver injection/strip |
| | `codegen.rs` | Sender stubs, receiver FIFO views, line dispatchers, doorbell routers, ring/read functions, init hooks, freshness anchors, `XbinPassBackend` |
| | `priority.rs` | Build-phase priority-line validation over raw sw/async declarations + view receivers |
| `rticx-xbin-rt` | `fifo.rs` | Atomic SPSC ring (`Fifo`), producer/consumer split, `view_at` placement |
| | `state.rs` | `SharedState` (magic/ready bitmap/epoch) and spawn-side `ReadyCache` |
| | `backend.rs` | `CrossBinBackend` + `IpcRegion` contract (pools, core id, cache/MPU) |
| | `lib.rs` | `CrossCoreMessage` marker trait, `Queue` re-export for ready queues |
| `rticx-xbin-driver` | `cli.rs` | `cargo xbin` CLI surface (`sync`, `build [--verify-elf]`, `verify [--release]`) |
| | `commands.rs` | `sync` (collect, merge, allocate, emit, generate), `build` (= sync + per-app build, optional ELF verification) and `verify` (check a plain `cargo build` output) |
| | `elf.rs` | Linked-ELF verification with the `object` crate: pool-view overlap, stack bound and distro pool bound symbols (M6.9-T7) |
| | `project.rs` | Project discovery (`rticx.toml` upward from the manifest dir) |
| `rticx-xbin-mock` | `lib.rs` | `MockSystem`: in-process aligned IPC pools, per-pair doorbell message word, ready/epoch over one `SharedState`, per-handle `current_global_core_id` |

Generated code reaches the runtime through the distribution's `rt_path` and a
distribution-injected backend helper; no public API of the root workspace changes.

### 5.3 Fixtures and what they test

The fixtures are standalone workspaces under `fixtures/`, each path-depending on the
workspace crates and (for `e2e`/`three-app`) on the shared mock distribution.

| Fixture | Topology | Exercises |
|---|---|---|
| `fixtures/metadata/` | Two apps (`app-m7` producer core 0, `app-m4` receiver core 1) + a minimal `metadata-macro` `#[app]` stand-in that runs only the xbin metadata pass and binds the mock capability table (M6.9-T9) | `cargo xbin sync` end to end: metadata collection, merge/validation, `system.json` (schema 2, `pools` + physical ids) emission and `ipc-types/` generation. Driven by the driver's `tests/fixture.rs`; the generated `ipc-types/` crate is checked in like a real project |
| `fixtures/e2e/` | Same two apps, with the mock **distribution** (`xbin-mock-distro` + `xbin-mock-runtime`) running the full core pass on `MockCoreBackend` and the xbin pass against an in-process backend | `cargo xbin build` over a real phase-1/phase-2 cycle (driver `tests/e2e.rs`); phase 2 generates the sender `cross_spawn`, the receiver dispatcher and the init hooks and links both host binaries. The pass crate's `tests/e2e_runtime.rs` expands both apps into one host binary and **runs** the generated chain: `cross_spawn` → ring function → router ISR → pended line dispatcher → `exec`, plus FIFO backpressure and coalesced/duplicate notifications |
| `fixtures/three-app/` | Three apps: producers `app-m7` (global 0) and `app-m5` (global 2) spawn onto receiver `app-m4` (global 1) through two shared pools, two priority lines and two per-pair routers | Multi-source support: driver `tests/three_app.rs` builds it with `cargo xbin build`; `tests/e2e_runtime.rs` runs all three apps in one process (per-pair routers/dispatcher lines, per-source backpressure, and the non-owner producer initializing its own pool); pass `tests/priority_lines.rs` covers build-phase priority-line validation |

Supporting negative/edge coverage lives next to the crates rather than in a fixture:
driver `tests/negative.rs` (missing sync, stale view/source, priority conflicts,
unknown types, region overflow), driver `tests/verify.rs` (linked-ELF
verification over `fixtures/e2e`: clean pass, injected pool overlap, standalone
`verify`), driver `tests/driver.rs`/`tests/cli.rs`, pass
`tests/{codegen,syntax,metadata,core_ids,ipc_dispatchers,init_hooks,receiver_compile}.rs`,
proto `tests/{idl,layout,generate,generate_compile,alloc,merge,project,system,hash}.rs`,
rt `tests/{fifo,state}.rs`, mock `tests/backend.rs`.

### 5.4 Renode acceptance harness (`renode/`)

The harness the H7 distribution is validated with, in-tree until the M7-T3
extraction:

| Item | Path | Notes |
|---|---|---|
| Platform | `renode/platforms/cpus/stm32h7_dualcore.repl` | Self-contained: RCC (`RCC_GCR.BOOT_C2`), PWR, HSEM and EXTI models embedded between `// >>> pydev:` markers; no C#/Renode build required |
| Startup script | `renode/scripts/single-node/stm32h7_dualcore.resc` | Loads the M7 image on `cpu0`, the M4 image on `cpu1`, runs `machine Reset` |
| Launcher | `renode/run.sh <cm7.elf> <cm4.elf> [seconds]` | Headless (`--console --disable-gui`), routes USART1 (M7) and USART2 (M4) to the log, runs, quits |
| Notes | `renode/README.md` | Provenance, memory map, doorbell paths, limitations |

Provenance: copied from the standalone simulation repository
`/home/zakaria/stm32-renode` (commit `c7caf8c`), which keeps the canonical
Peripheral-Script sources, the `sync_pydev_into_repl.py` regenerator, the known-bug
log and the Python/Robot suites. Installed Renode is v1.16.1.


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
```

> **Since M6.9-T3:** the manifest declares **no** IPC memory. The distribution
> owns the IPC pools and exposes a capability binding (reachable core pairs,
> pool id, per-core base views, shared per-dual budget) that each application
> records in its metadata manifest, so `rticx.toml` carries only
> `[[application]]`/`core_ids`. A leftover `[ipc.regions]` table is a hard
> `sync` error naming the removal.

Notes:

- `core_ids` maps local core indices to globally unique ids used in task attributes
  and the JSON view.
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
  historical behavior. Parsed and validated natively by `rticx-core` (M5).
- `external_cores = [g...]` — global ids of cores in other binaries visible to this
  application. Consumed (stripped) by the multi-binary pass; `rticx-core` does not
  know this key, so other distributions only see an unknown-key warning for it.
- `ipc_dispatchers = [IRQ...]` / `[[IRQ...], ...]` — pass-owned per-core list of
  interrupt lines used by the generated cross-binary **line dispatchers**; same
  shape as `dispatchers` (a single array when `cores = 1`, one inner array per core
  otherwise), one entry per `(source, priority)` line on that core in the
  deterministic `(source, priority)` order. Consumed (stripped) by the multi-binary
  pass before the core/software/async passes run, so no other pass sees those
  entries; the list must be disjoint from `dispatchers`.

Task `core` syntax stays local (`0..cores`). Natively `spawn_by` is a local index
too; in an application that declares `external_cores`, the xbin pass resolves
`spawn_by` in the global namespace instead (see §6.5). The generated runtime core
checks and the multi-binary metadata translate through `core_ids`.

### 6.5 Task syntax (native `#[sw_task]`)

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
  and `CrossBinSpawn` were deleted outright with no compatibility shim.
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
  `swtasks` feature: sw-pass emits the generated `RticSwTask` trait, the receiver
  is compiled through the core pass's `task_trait` mechanism, and the pass reuses
  sw-pass's `__rticx_local_irq_pend[_core{N}]` to pend the line dispatchers. Async
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
     priorities, capacities, and the per-local-core distro capability binding
     (physical core id + IPC pools; M6.9-T2). There are no sender declarations:
     the driver infers each task's single producer application from the
     receivers' `spawn_by`.
3. Parse `ipc-types.toml`; compute canonical layout; validate the type subset.
4. Merge manifests and validate:
   - every receiver's `spawn_by` names a single existing producer in another
     application, is listed in the receiver's `external_cores`, and the producer
     application lists the receiver's core in its `external_cores`;
   - per-target priority lines are disjoint across producer cores; a collision is a
     hard error (the driver plans and validates the lines, it never shifts
     priorities). Manifests carry only cross-binary receivers, so collisions with
     the target application's own sw/async tasks are not visible here and are
     checked by the pass at `build` (see §8);
   - every referenced type exists in the IDL and is cross-core safe;
   - total per-task FIFO bytes fit each dual's shared pool budget (both
     directions together, M6.9-T4);
   - global core ids consistent across applications and `rticx.toml`.
5. Allocate per-task FIFO offsets deterministically:
   - pools keyed by `(core_a, core_b)`; within a pool, tasks in global task order;
   - align each FIFO to 8 bytes (indices cache-line padded in the runtime type);
   - depth = `capacity + 1` (ring buffer wastes one slot; effective capacity is
     exact because the dispatcher drains the FIFO directly);
   - hard error if the pool overflows.
6. Write `target/rticx-xbin/system.json`.
7. Generate/update `ipc-types/` (types, marker impls, consts, asserts, `LAYOUT_HASH`);
   report if changed.

### 7.2 `system.json` contents

```jsonc
{
  "schema_version": 2,
  "rticx_generation": "0.x",
  "topology_hash": "...",
  "cores":  [ { "global_id": 0, "physical_core": 0, "app": "app-m7", "local_index": 0 } ],
  "types":  [ { "name": "EncryptReq", "size": 12, "align": 4,
                "fields": [ { "name": "addr", "ty": "u32", "offset": 0 } ] } ],
  "tasks":  [ { "id": 1, "name": "EncryptTask",
                "receiver_core": 1, "spawner_core": 0,
                "priority": 3, "capacity": 2,
                "input_type": "EncryptReq",
                "fifo": { "source": 0, "target": 1, "pool": "p01", "offset": 0,
                          "elem_size": 12, "depth": 3 } } ],
  "pools": [ { "id": "p01", "core_a": 0, "core_b": 1,
               "base_from_a": "0x30040000",
               "base_from_b": "0x30040000", "budget": 4096, "used": 100 } ],
  "doorbells": [ { "source": 0, "target": 1, "priority": 3, "line": 0 } ]
}
```

> **Since M6.9-T5 (schema 2):** `cores[]` records each core's `physical_core`,
> `pools[]` replaces `regions[]` (one entry per unordered core pair, with each
> core's view, the shared `budget` and the bytes `used`), and `fifo` carries the
> `pool` id with a pool-relative `offset`. `fifo.pool` is omitted when the
> project bound no distro capability table (a distribution that has not adopted
> the M6.9 binding).

v1 keeps one producer core per task, so every `tasks[]` entry has a single
`spawner_core` and one `fifo` (one FIFO per `(task, source → target)` direction),
while a target core may carry lines from several producer cores (one doorbell entry
per `(source, target, priority)`). `core_ids` and `external_cores` stay arrays; only
`spawn_by`/`spawner_core` are singular.

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
longer requires cross-task declarations:

- **FIFO views** for every cross task at `base[core_view] + offset` via
  `rticx-xbin-rt`, with const addresses from the generated metadata. No local input
  queue, no forwarder, no task id/union.
- **Receiver side:**
  - the receiver's native `#[sw_task]` is rewritten into
    `#[task(priority = <line>, core = N, task_trait = RticSwTask, init = generated)]`
    (preserving `shared`), so the core pass generates its static and enforces the
    user's `impl RticSwTask`; the native `spawn` queue is not generated for
    cross receivers — their inputs arrive through the FIFO;
  - one generated **line dispatcher** per `(source → target, priority)` line,
    bound to the line's `ipc_dispatchers` entry:
    `#[task(binds = <ipc_dispatchers entry>, priority = <line>, core = N,
    init = generated)]` implementing `RticTask`, whose `exec` drains the line's
    ready queue and, per popped task, drains its FIFO until empty (a duplicate
    notification finds an empty FIFO and is a no-op) and calls the receiver
    task's `exec(input)`;
  - one generated **doorbell router** per `(source → target)` pair:
    `#[task(binds = <pair doorbell IRQ>, priority = <highest line priority of the
    pair>, core = N, init = generated)]` whose `exec` loops reading task ids from
    the pair's doorbell message and, per id, enqueues the task in its line's
    ready queue and pends that line's dispatcher through sw-pass's
    `__rticx_local_irq_pend[_core{N}]`; unknown ids are ignored. The ready queue
    is sized to the sum of the line's task capacities, so a spawn notification
    (one FIFO element plus one ready entry) can never overflow it.
- **Priority-line validation:** the pass runs before the sw/async passes, so it
  analyzes the application's raw `#[sw_task]`/`#[async_task]` declarations
  (`priority`, `core`, `spawn_by` mapped through `core_ids`) together with the
  cross-bin receivers from the whole view. On each target core one priority level
  belongs to exactly one origin core: reject a cross-bin receiver that collides
  with a core-local sw/async task, with an in-app `spawn_by` task, or with a
  cross-bin receiver from a different producer core. Receivers from the same
  producer may share a line. Violations are hard compile errors on the receiver
  declaration naming the conflicting task; `sync` cannot see local tasks, so this
  check runs at `build` only.
- **Message-type assertion:** the pass emits a const assertion that each
  receiver's `<Task as RticSwTask>::SpawnInput` implements
  `rticx_xbin_rt::CrossCoreMessage`, so the guarantee lives in generated code
  instead of the trait definition.
- **Sender stubs:** for every view task whose `spawner_core` belongs to this
  application, the pass generates `pub struct <Task>;` and the `cross_spawn` impl
  below inside the `#[app]` module; the producer source never mentions the task. A
  name collision with a user item is a dedicated compile error.
- **Sender API:** `Task::cross_spawn(input)`:
  - runtime global-core guard (`if current_global_core_id() != expected { return Err(Some(input)) }`);
  - target-ready gate through the spawner-local cached epoch
    (`rticx_xbin_rt::ReadyCache`): `Err(Some(input))` while the target has not
    marked itself ready, refreshing the epoch once after a peer reset or
    reinitialization;
  - enqueue into the task FIFO inside `__rticx_interrupt_free` (v1);
  - call the generated per-pair ring function
    `__rticx_xbin_ring_{source}_{target}(task_id)`, which delivers the task id to
    the target's router and triggers its doorbell IRQ;
  - semantics: `Ok(())`, `Err(None)` (enqueued but the notification failed),
    `Err(Some(input))` (not enqueued), matching existing `cross_spawn`.
- **Init hooks** (through the backend):
  - `init_shared` on the owner core (set magic/epoch);
  - every core zeroes the ring indices of the FIFOs it **produces** (its
    outbound half of each dual's shared pool) at `BeforePostInit`, before its
    `post_init` can spawn; topologies whose owner core is not an endpoint of a
    pool initialize correctly;
  - `mark_ready(core)` at the end of each core's `post_init`; the router IRQs are
    already enabled and prioritized by the core pass's used-IRQ machinery, so no
    per-line doorbell arming step remains;
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
- Ready/epoch helpers: atomic ready bitmap, epoch word for peer-reset detection,
  and the spawn-side `ReadyCache` (cached epoch, refresh-on-reset).
- `Queue` — the core-local SPSC queue backing generated line ready queues (re-export
  of the software pass's queue type).
- Deliberately separate from `rticx-spsc` so existing non-atomic single-binary
  semantics stay frozen.

---

## 10. Distribution/backend contract

Owned by the extension; implemented by the out-of-tree H7 distribution and by the
mock backend.

**Runtime half — `rticx_xbin_rt::backend::CrossBinBackend`:**

- `ipc_region()` — the dual's shared pool as seen through `(source, target)`:
  each endpoint's base view and the shared budget. Both directions of a dual
  return the same pool (M6.9-T6).
- `configure_shared_memory()` — configure Normal, Non-cacheable, Shareable (MPU).
  Device/Strongly-ordered is forbidden (`ldrex`/`strex` are invalid there).
- optional `clean_range` / `invalidate_range` for cacheable-region fallback.
- `current_global_core_id()`.
- `shared_state()` with default `init_shared()`, `mark_ready(core)`, `is_ready(core)`,
  `epoch()`.
- Carries **no doorbell methods**: the transport is the generated per-pair
  ring/read bodies.

Boot sequencing is distribution-owned (H7: CM7 initializes the region and then
releases CM4 via `RCC_GCR.BOOT_C2`; the available M7/M4 doorbells are HSEM IRQ
125/126 and per-core-masked EXTI lines, §11.1); the ready bitmap makes boot order and
peer reset safe. `cross_spawn` returns `Err` when the target is not ready.

**Code-generation half — `rticx_xbin_pass::XbinPassBackend`** (proc-macro side): the
distribution makes the per-pair doorbell concrete by filling templates —

| Binding | Emits / names |
|---|---|
| `ring_doorbell_fn(source, target, template)` | `__rticx_xbin_ring_{source}_{target}(task_id) -> Result<(), ()>` on the producer side |
| `doorbell_interrupt(target, source) -> Ident` | the router's `binds` |
| `read_doorbell_msg_fn(target, source, template)` | `__rticx_xbin_read_{source}_{target}() -> Option<u32>` on the target side |
| `custom_interrupt_path(core)` | the interrupt type the router pends through (mirrors the software pass's backend) |

A portable implementation writes the task id to a per-pair shared atomic word and
triggers one IRQ; hardware with a payload-capable doorbell can implement the same
contract directly. There is no IPCC on the H7 line, so the portable shape applies.
The router IRQ is enabled/prioritized by the core pass's used-IRQ machinery (no
`doorbell_setup` call remains).

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
  `ipc_dispatchers` pool tests (both shapes, line count, duplicates, overlap
  with `dispatchers`); router tests (id→line match arm per view task, unknown
  ids ignored, coalesced/duplicate ids lose no spawns, ready-queue sizing);
  codegen snapshots (auto-generated sender stub, receiver dispatcher, ring and
  read-doorbell functions, router, FIFO views, topology-hash mismatch); JSON
  schema round-trip.
- **Host mock:** two threads over `rticx-xbin-rt` with a mock doorbell; scenarios:
  spawn → drain, FIFO full/backpressure, ready/epoch, simulated peer reset.
- **Cross-compile layout checks:** `thumbv7em-none-eabihf`, `thumbv6m-none-eabi`,
  optionally `riscv32imc`.
- **In-tree end-to-end:** the two- and three-application fixture projects
  (`fixtures/e2e`, `fixtures/three-app`) built via `cargo xbin build`. The runtime
  harness (`crates/rticx-xbin-pass/tests/e2e_runtime.rs`) expands all applications
  into one process over a shared `MockSystem`: two producer cores each spawning onto
  the receiver core through their own router and dispatcher line, auto-stub
  generation for an application with zero declarations, per-producer `BeforePostInit`
  FIFO zeroing of a pool whose owner is not an endpoint, ordered draining of both
  FIFOs and per-source backpressure. The single-pair harness additionally drives the
  full path `cross_spawn` → ring function → router ISR → pended line dispatcher →
  `exec`, including coalesced notifications.
- **Acceptance (out-of-tree):** STM32H7 M7+M4 distribution under the in-tree
  Renode harness (§11.1) with a real doorbell (HSEM/EXTI) and non-cacheable
  shared SRAM.
- **CI:** separate advisory workflow for `experiments/multibinary` (fmt, clippy,
  tests, mock e2e), M8-T1. Root CI untouched.

### 11.1 Renode acceptance harness (STM32H7 M7+M4)

See [§5.4](#54-renode-acceptance-harness-renode) for the file map. Platform facts
the H7 distribution must be written against:

- **Boot.** CPU0 (M7) boots from flash bank 1 (`0x0800_0000`); CPU1 (M4)
  starts halted with `VTOR = 0x0810_0000` and runs only after the M7 writes
  `RCC_GCR.BOOT_C2`. That release is the distribution's `init_shared` → boot step.
- **Execution.** `Machine SetSerialExecution True`, so both cores are
  deterministic and the IPC paths are race-free in simulation.
- **Shared memory.** AXI SRAM (`0x2400_0000`), SRAM1–3 (`0x3000_0000`,
  `0x3002_0000`, `0x3004_0000`, aliased at `0x1000_0000`+ for the D2/M4 view)
  and SRAM4 (`0x3800_0000`) are shared and coherent; DTCM/ITCM are M7-private.
  IPC regions go in one of the shared blocks (SRAM4 carries the upstream demo's
  mailbox).
- **Doorbells.** HSEM (`0x5802_6400`): releasing semaphore `n` raises IRQ 125
  (`HSEM1`) on `nvic0`/M7 and IRQ 126 (`HSEM2`) on `nvic1`/M4, and sets the
  status bit on both cores. EXTI (`0x5800_0000`): a shared `SWIERx` write sets
  the pending bit in both `C1PRx` and `C2PRx`; each core masks through its own
  `CxIMR` and clears through its own `CxPRx`, which makes EXTI lines usable as
  one-directional doorbells. There is no IPCC on this line, so the portable
  per-pair doorbell word + router IRQ shape applies.
- **Console.** USART1 is the M7 console and USART2 the M4 console;
  `showAnalyzer` is required even headless.
- **No cache model.** Renode does not emulate the M7 D-cache, so the
  non-cacheable MPU configuration is invisible in simulation: a green Renode
  run validates boot sequencing, transport and interrupt routing, never cache
  policy.

---

## 12. Documentation

- **User guide (`docs/user-guide.md`):** install `cargo-xbin`; enable the
  distribution's `swtasks` feature; write `rticx.toml` and `ipc-types.toml`;
  declare receivers as native `#[sw_task]` items (`impl RticSwTask`, `SpawnInput`)
  whose `spawn_by` names the producer core and list one `ipc_dispatchers` entry per
  cross line (producer stubs are generated by the pass; producer sources only call
  `Task::cross_spawn`); `cargo xbin build`; run; common errors and fixes. Async
  cross-binary tasks are documented as not yet supported.
- **Architecture doc (`docs/architecture.md`):** phases, JSON schema, memory/priority
  rules, locking, boot and reset, failure modes, cache/MPU notes, the native
  `#[sw_task]` declaration model (global `spawn_by` resolution for apps declaring
  `external_cores`), and the dispatch model (per-pair doorbell routers,
  `ipc_dispatchers` line dispatchers, task-id routing).

---

## 13. Remaining milestones

Work proceeds one task at a time; each task is committed separately with tests and
its own acceptance criteria. Milestones M0–M6.5 are complete (task lists removed;
see git history). What remains:

### M6.9 — Distro-owned IPC pools and link-time verification

The project stops knowing anything about IPC memory addresses. The distribution
becomes the single owner: it exposes a **capability binding** (which core pairs
can reach each other, through which pool, with which per-core views and budget)
and ships the linker reservations that keep the pools out of every binary's
`.data`/`.bss`. The driver allocates both directions of a core pair inside one
**shared per-dual budget**, and `cargo xbin build --verify-elf` (or the
standalone `cargo xbin verify`) independently verifies the linked images against
the pool ranges.

**Decisions locked by this milestone**

- `rticx.toml` has **no** region/address syntax at all (no project-level
  override); a leftover `[ipc.regions]` is a hard `sync` error.
- One **shared pool per unordered core pair** (a *dual*): both `A→B` and `B→A`
  FIFOs allocate inside the same physical block under a single budget. This
  reduces fragmentation versus one region per direction.
- The distro owns the pool addresses/aliases and the linker reservations; the
  driver independently verifies the result (it never trusts the script).
- Pool identity is a distro vocabulary, not a project one. The pass reports each
  local core's **physical core id** and its pools so the driver can match the two
  endpoints of every dual.

**Binding (host/proc-macro side), proposed shape**

```rust
/// Stable, distro-defined physical-core identifier (e.g. "cm7", "cm4").
pub struct PhysicalCore(pub u32); // or a small interned string id

/// One shared IPC pool of an unordered core pair, as seen from this core.
pub struct IpcPool {
    pub id: PoolId,          // symbolic, distro vocabulary ("sram3", "axi", ...)
    pub peer: PhysicalCore,  // the other endpoint of the dual
    pub base_local: u32,     // this core's view
    pub base_peer: u32,      // the peer's view (aliases allowed)
    pub budget: u32,         // shared by both directions of the dual
    pub policy: CachePolicy, // Normal/Non-cacheable/Shareable + MPU hints
}

trait XbinPassBackend {
    // ... existing doorbell / rt / interrupt bindings ...
    /// Physical core this application's local core `local_core` runs on.
    fn physical_core(&self, local_core: u32) -> PhysicalCore;
    /// Every IPC pool this physical core can reach (the adjacency, from one side).
    fn ipc_pools(&self, local_core: u32) -> Vec<IpcPool>;
}
```

The adjacency is exactly the set of pools: an edge `{A, B}` exists iff `A`
reports a pool whose peer is `B` **and** `B` reports the matching pool. A
direction with no pool is an impossible link; `sync` rejects a receiver whose
`spawn_by` has no pool path with a precise error.

- [x] **M6.9-T1 — Distro capability binding.** Add `PhysicalCore`, `PoolId`,
  `IpcPool`, `CachePolicy` and the `physical_core` / `ipc_pools` methods to the
  distro code-generation contract; implement them in the mock distro with the
  fixture topology (pools for the `{0,1}`, `{1,2}`, `{0,2}` duals). Unit tests
  pin the binding shape and that the returned pools are symmetric across the two
  endpoints' vocabularies.
- [x] **M6.9-T2 — Manifest schema 2.** In metadata mode the pass writes, per
  local core, its physical id and its pool entries (id, peer physical id, both
  views, budget, policy) into `<app>.xbin.json`; `MANIFEST_SCHEMA_VERSION` bumps
  to 2. Extraction tests cover the identity default and multi-core applications.
- [x] **M6.9-T3 — Remove `[ipc.regions]` from `rticx.toml`.** `project.rs` drops
  `Region`, `RegionKey`, `parse_ipc`/`parse_regions`, `validate_region_views`
  and the `ipc` top-level key; `ProjectConfig::to_toml_string` renders only
  applications; `SyncOutcome::region_count` goes away. A leftover
  `[ipc.regions]` is a hard error naming the removal. Project/golden/negative
  tests and every fixture `rticx.toml` updated. To keep the driver green,
  `merge_project` now derives each used direction's region view from the source
  core's capability pool (`base_local`/`base_peer`/`budget`); a project whose
  manifests carry no capability table allocates unbounded and emits no region.
  T4 replaces this per-direction view with the matched pool model and shared
  budget.
- [x] **M6.9-T4 — Merge: capability graph, adjacency and shared budget.**
  `merge_project` builds pools by matching the two endpoints' capability entries
  (same `PoolId`, opposite physical ids; a one-sided or inconsistent entry is
  `IpcPoolMismatch`), resolves the global adjacency, and rejects a used direction
  with no pool (`NoIpcPath`). FIFO allocation places **both directions of a dual
  inside one budget**, deterministically (pool, then task order), 8-byte aligned,
  depth `capacity + 1`; overflow is `PoolBudgetExceeded` naming the dual, pool,
  needed and budget. The per-core view-disjointness check moves from
  `rticx.toml` to the distro-reported pool views (`PoolViewOverlap`).
  `MergedProject.regions` becomes `MergedProject.pools`. Unit tests: two
  directions fitting together but not separately (and vice versa), budget
  overflow, asymmetric/mismatched capabilities, impossible link. `PoolEntry`
  (`{ id, core_a, core_b, base_from_a, base_from_b, budget, used }`) is defined
  now; until T5 the emitted `system.json` still expands each pool into its two
  schema-1 `regions[]` entries (and `alloc.rs` sorts them by `(source, target)`),
  so the field only changes in the merge model here. A project whose manifests
  carry no capability table at all (the minimal `metadata-macro` fixture, until
  T9) has an empty pool graph and allocates sequentially per direction, exactly
  as in T3.
- [x] **M6.9-T5 — `system.json` schema 2.** `cores[]` gains `physical_core`
  (the capability binding's id; the identity global id when the application
  bound no backend); `regions[]` is replaced by `pools[]`
  (`{ id, core_a, core_b, base_from_a, base_from_b, budget, used }`); `FifoEntry`
  gains `pool` and its `offset` is pool-relative. `canonical_topology_text` /
  `seal` cover pools and physical ids, so a distro change that moves a pool
  changes the topology hash and forces a rebuild; `SYSTEM_SCHEMA_VERSION` bumps
  to 2. `alloc.rs` copies the matched pools and the per-task pool ids directly.
  `FifoEntry::pool` is `Option<String>` (serialized only when present): a
  project whose manifests carry no capability table at all has no pool to name
  and its FIFOs are
  unbounded, exactly as in T3/T4. `visualize.rs` keeps emitting the existing
  per-direction region panels by expanding each pool into its two directions
  until T8 renders real pool panels. Golden/host fixtures and `alloc.rs` updated
  (`proto/tests/system.rs`, `proto/tests/alloc.rs`, the pass codegen/init-hooks/
  priority-line fixtures, the driver fixture/three-app assertions).
- [x] **M6.9-T6 — Codegen and runtime.** `CrossBinBackend::ipc_region(source,
  target)` keeps its signature but now returns the dual's shared pool:
  `base_from_source`/`base_from_target` are each endpoint's view and both
  directions return the same pool with the same shared budget (documented on
  `IpcRegion`). Generated FIFO views and init hooks keep reading the
  pool-relative offsets and additionally pin the task's distro pool: they emit
  `__RTICX_XBIN_POOL_ID` / `__RTICX_XBIN_POOL_BUDGET` and const-assert the FIFO
  still fits the budget, and a task naming a pool absent from `pools[]` is a
  hard `sync`-naming error. Stale-view panic messages now say "IPC pool". The
  mock runtime exposes pools (`MockSystem::add_pool(core_a, core_b, size)`,
  unordered duals; both directions resolve to the one backing) and the fixture
  runtime declares the three `{0,1}`/`{1,2}`/`{0,2}` pools; the
  codegen/init-hooks/e2e harnesses declare pools instead of per-direction
  regions and pass with the shared budgets. Tests: mock pool-table/dual-sharing
  unit tests, the pinned-const sender snapshot, and the unknown-pool rejection.
- [x] **M6.9-T7 — ELF verification.** `cargo xbin build --verify-elf` (default
  **off**) runs `sync`, links every application and then verifies each linked
  binary; a standalone `cargo xbin verify` (`--release` for the release
  profile) checks the plain-`cargo build` workflow without re-running `sync`.
  Both parse the linked ELF with the `object` crate and assert that no
  `SHF_ALLOC` section (`[address, address + size)`) intersects a pool view of
  the application's own cores, that the stack bound symbol
  (`_stack_start`/`__rticx_xbin_stack_start`, when defined) is outside every
  pool view, and, where the distro exports `__rticx_xbin_pool_<id>_start/_end`,
  that they match the `system.json` base and shared budget. Failures name the
  application, section/symbol, range and pool
  (`VerifyError::{Overlap,StackOverlap,PoolBounds}`). The build discovers each
  executable through Cargo's `--message-format json-render-diagnostics`
  `compiler-artifact` messages (robust across target dir, triple, profile and
  `.cargo/config.toml`); `verify` reads `target/rticx-xbin/system.json`,
  rejects a stale (`topology_hash` mismatch) view and locates the already-built
  binary with a cached `cargo build` so nothing relinks. The check is
  detection, not prevention: it sees static `SHF_ALLOC` sections only, not
  runtime stack/heap growth, and its symbol checks need the **unstripped** link
  output (a stripped binary loses `.symtab`; the section check still works).
  Tests: `driver/src/elf.rs` unit tests (pool-view selection, section overlap,
  half-open pool containment, pool-bound symbol matching/sanitization) and
  `driver/tests/verify.rs` over the built `fixtures/e2e` (clean pass, an
  injected overlap naming the section and pool, the standalone `verify`, and
  `build` without `--verify-elf` reporting no verifications).

- [x] **M6.9-T8 — Pool-panel visualization.** `visualize.rs` renders one panel
  per distro pool (the *dual*) instead of expanding each pool into its two
  per-direction region panels. A panel carries both directions' FIFOs at their
  pool-relative offsets, colored by producer core, and shows the pool id, both
  cores' base views, the shared `budget` and the `used` bytes. The bar is scaled
  to the occupied extent (`used`, at least the end of the last FIFO) rather than
  the whole budget, so the FIFOs stay readable instead of collapsing into a
  sliver (the pre-T8 panels rendered only ~2–7 % of the bar). A FIFO naming no
  (or an unknown) pool — a project whose manifests carry no capability table —
  falls back to a synthetic per-direction panel so it stays visible. Sender and
  receiver arrows are wired through each FIFO's own `source`/`target`
  (`app.js`, `template.html`, `style.css`). Tests: the `visualize` module unit
  tests (one pool panel per dual, both directions in one panel, occupied-extent
  scaling, empty pool, unpooled fallback, JSON embedding/determinism/escaping).

- [x] **M6.9-T9 — Docs and fixtures.** The metadata-only fixture now binds the
  mock capability table (shared through the new `xbin-mock-capability` crate
  with `xbin-mock-distro`), so `cargo xbin sync` emits `system.json` schema 2
  with `pools[]` and physical core ids for every fixture; the e2e/three-app
  `rticx.toml` comments drop the last per-direction language. The user guide
  loses the region/linker-carving sections: `rticx.toml` documents only
  applications/`core_ids`, and §7.5 becomes distro-author guidance (the
  capability binding, pool linker reservations and `--verify-elf`/`verify`).
  The architecture doc is updated throughout (§2/§3/§4/§6/§7/§9/§10/§11/§12)
  for pools, physical ids, the shared budget and link-time verification.
  Fixture test: the metadata `sync` asserts the matched `mock-0-1` pool and the
  pool-relative FIFO; CLI test: `sync --html` over `fixtures/e2e` renders the
  `mock-0-1` pool panel with its shared budget.

*Acceptance:* `cargo xbin sync` validates the fixtures with no region syntax and
emits `system.json` with `pools` and physical ids; `cargo xbin build` links both
mock binaries and the ELF check passes, while an injected overlapping section is
rejected with the pool named; a test where two directions fit a shared budget but
not separate ones (and one that overflows the shared budget) pins the per-dual
budget rule; `sync --html` renders pool panels; all existing mock e2e harnesses
pass.

*Explicitly out of scope:* a project-level pool override (decided against), a
runtime arena handed over at boot, and hardware-IPC-RAM-specific defaults (a
distro may use them internally, but no new framework mechanism).

### M7 — STM32H7 acceptance and extraction (separate effort, out-of-tree)

- [ ] **M7-T1** Out-of-tree STM32H7 (M7+M4) distribution implementing
      `CrossBinBackend` (non-cacheable shared region, boot release) and the
      `XbinPassBackend` doorbell bindings (ring/read/router IRQ).
      *Acceptance:* the M7 and M4 example binaries boot under
      `experiments/multibinary/renode/run.sh` (§11.1): the M7 initializes the
      shared region, releases the M4 through `RCC_GCR.BOOT_C2`, and both cores
      mark themselves ready.
- [ ] **M7-T2** Renode acceptance demo: cross-binary spawn M7→M4 and M4→M7.
      *Acceptance:* `renode/run.sh <m7.elf> <m4.elf>` shows both cross-binary
      directions executing (console output and/or a shared result word) through
      the generated dispatch path.
- [ ] **M7-T3** Extract `experiments/multibinary` into its own repository; resolve
      and remove `TODO(extract)` markers; add CI there.
- [ ] **M7-T4** Optional: re-evaluate promoting reimplemented internals into a shared
      RTICX crate, and the `Producer`-as-shared-resource API with SRP locking.

### M8 — CI

- [ ] **M8-T1** Separate advisory CI workflow for the experimental workspace.
      *Acceptance:* fmt, clippy, tests, mock e2e green.

---

## 14. Risks and open items

| Risk / open item | Mitigation / note |
|---|---|
| `ipc-types` generation location policy | Recommended: root `ipc-types/`, kept in VCS; `sync` reports changes |
| Doorbell/IRQ availability per distro | One doorbell IRQ per producer pair plus one dispatcher IRQ per line, supplied by the user's `ipc_dispatchers` list; the router coalesces all lines of a pair onto that one IRQ |
| Cacheable-region fallback correctness | v1 recommends non-cacheable only; fallback hooks exist but are not the supported path |
| Staleness under plain `cargo build` | Only the changed app detects its own staleness; `cargo xbin build` is the safe path |
| Reimplemented internals drift from `rticx-sw-pass` | `TODO(extract)` markers; periodic manual comparison; possible later shared crate |
| Priority-line exhaustion on a target | No automatic shifting: producer cores must declare disjoint priorities; collisions are hard errors naming both tasks (`sync` for producer-vs-producer, the pass at `build` for collisions with the target app's own sw/async tasks); the limits are documented |
| Future `Producer`/SRP API | Transport and lock are separated in codegen so migration is localized |
| Auto-generated sender stub name collision | The generated `pub struct <Task>` can collide with a user item in the `#[app]` module; the pass emits a dedicated compile error naming the task |
| xbin-pass depends on the `swtasks` feature | `RticSwTask` is generated by sw-pass and the receiver compiles through the core pass's `task_trait`; without the feature the generated code fails on the unresolved trait, and the requirement is documented |
| Global `spawn_by` namespace in apps declaring `external_cores` | The native local-index reading still applies where `external_cores` is absent; the identity `core_ids` default makes both readings identical, and the rule is documented per application |
| Doorbell notification loss/coalescing | The producer sets the task id in the pair's doorbell word and then triggers the IRQ; the router drains until no id remains, and duplicate/coalesced ids are idempotent because the line dispatcher drains FIFOs until empty |
| Router ready-queue overflow | Sized to the sum of the line's task capacities: each spawn adds exactly one FIFO element and one ready entry, so it cannot overflow |
| Router priority vs its dispatchers | Each router runs at the highest line priority of its pair, so it is at least as urgent as every dispatcher it pends; enqueueing happens before the pend |
| Project-dependent `ipc_dispatchers` count | Another binary adding a producer adds a line to the receiver; the receiver can derive its line count from its own declarations, and `build` fails with a precise error naming the missing line when `ipc_dispatchers` is too short |
| Renode does not model the M7 D-cache | A green Renode run validates boot sequencing, transport and interrupt routing only; the non-cacheable-region/MPU requirement needs hardware evidence or static review (§11.1) |
| Renode platform drift from upstream | The in-tree copy records its source commit in `renode/README.md`; the upstream simulation repository keeps the canonical Peripheral-Script sources and Python/Robot tests, so platform changes are re-copied deliberately |
| H7 doorbell wiring | HSEM (IRQ 125/126) and per-core-masked EXTI lines are both available on the harness; the portable per-pair doorbell word + router IRQ fits both, and the router IRQ is armed through the core pass's used-IRQ machinery |
| Local vs global core ids in out-of-tree distros | The identity default keeps rp2040/riscv correct; they adopt `core_ids` in follow-up PRs per `COMPATIBILITY.md` after the M5 release |
| No project-level IPC pin (M6.9) | With pools distro-owned, two images built by one `cargo xbin sync` stay consistent (the topology hash catches drift), but a **pre-built peer image** would silently break if a distro change relocates a pool. Supported scope is one project build per system; a project-level pin is the documented future option if an independently built peer appears |
| Pool budget contention (M6.9) | Both directions of a dual share one budget, so a busy direction can exhaust the other's room. `PoolBudgetExceeded` names the dual, pool, needed and budget; the fix is a larger distro budget (the per-dual shared budget is deliberate, to avoid fragmentation) |
| ELF verification is detection, not prevention (M6.9) | The check sees static `SHF_ALLOC` overlap only (`.data`/`.bss`/`.noinit`), not runtime stack/heap growth, and needs the unstripped link output; it still turns the silent-corruption class into a build error |

---

## 15. Glossary

- **IDL** — the type-only interface description (`ipc-types.toml`).
- **System view** — the driver-generated `system.json` describing the whole project.
- **Region** — distro-declared shared-memory block for one `(source → target)`
  direction, containing all per-task FIFOs of that pair. Replaced by **pools**
  in M6.9 ([§13](#13-remaining-milestones)).
- **Pool** — distro-owned shared-memory block for one unordered core pair (a
  *dual*), with per-core base views and a budget shared by both directions;
  carries the FIFOs of both `A→B` and `B→A` (M6.9).
- **Dual** — an unordered pair of physical cores that share one IPC pool; both
  directions allocate inside its budget.
- **Physical core id** — the distro's stable identifier of a physical core
  (for example `"cm7"`, `"cm4"`), used to match the two endpoints of a dual
  across application manifests.
- **Pool budget** — the maximum number of bytes a distro reserves in a pool for
  IPC; the FIFOs of both directions of the dual must fit it together.
- **FIFO** — per-task atomic SPSC ring inside a pool (formerly a region), one
  element per spawn input.
- **Doorbell** — hardware signal (mailbox/IPI/HSEM) that wakes a target's
  doorbell router for one `(source → target)` pair.
- **`ipc_dispatchers`** — pass-owned per-core `#[app]` list of interrupt lines
  used by the generated cross-binary line dispatchers, one entry per
  `(source, priority)` line.
- **Line dispatcher** — the generated hardware task bound to an `ipc_dispatchers`
  entry at a line's priority; it drains its ready queue and the task FIFOs and
  calls the receiver tasks' `exec`.
- **Doorbell router** — the generated per-`(source → target)` hardware task bound
  to the pair's doorbell IRQ; it reads task ids, enqueues the task in its line's
  ready queue and pends the line dispatcher.
- **Ring / read-doorbell function** — the pass-generated
  `__rticx_xbin_ring_{source}_{target}(task_id) -> Result<(), ()>` and
  `__rticx_xbin_read_{source}_{target}() -> Option<u32>` whose bodies the
  distribution fills through the `XbinPassBackend` bindings.
- **Priority line** — a target-core priority reserved for tasks arriving from one
  specific remote source core; producer cores on one target must hold disjoint
  lines (validated at `sync` for producer-vs-producer, by the pass at `build` for
  collisions with the target application's own sw/async tasks; never re-planned).
- **Cross-binary receiver** — a native `#[sw_task]` in the consuming application
  whose `spawn_by` names an external (global) core; it is the only declaration of
  the cross-binary task.
- **`SpawnInput`** — the `RticSwTask` associated type carrying a software task's
  input; for cross-binary receivers it must implement `CrossCoreMessage`, enforced
  by a generated const assertion.
- **Sender stub** — the `pub struct <Task>;` plus `Task::cross_spawn(input)` API
  the pass generates inside a producer application's `#[app]` module for every
  system-view task whose `spawner_core` is one of its cores; producer sources do
  not declare it.
- **Local core index** — a core's position in an application's `0..cores`, as used
  by task `core` syntax and `pacs` (and by `spawn_by` in applications that do not
  declare `external_cores`).
- **Global core id** — the project-wide id used by runtime core checks,
  `external_cores` and the system view: local index `i` maps to `core_ids[i]`
  (identity by default).
- **Epoch** — shared counter used to detect peer reset and re-synchronize FIFOs.
- **Renode harness** — the dual-core STM32H7 platform, startup script and
  `run.sh` launcher under `experiments/multibinary/renode/` used as the M7
  acceptance target (§11.1).
