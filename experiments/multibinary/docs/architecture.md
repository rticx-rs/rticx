# Architecture

This document explains how the multi-binary multicore extension is put
together: the two-phase build, the contract that keeps independently built
binaries consistent, memory layout, hashing, locking and the
distribution/backend contract.

The [user guide](user-guide.md) is the task-oriented companion — read it first
if you want to *build* an application. Read this document if you want to
understand or extend the machinery.

**Contents**

1. [Motivation](#1-motivation)
2. [Problems and constraints](#2-problems-and-constraints)
3. [Goals and non-goals](#3-goals-and-non-goals)
4. [Design decisions](#4-design-decisions)
5. [Layers and crates](#5-layers-and-crates)
6. [Phase 1: `cargo xbin sync`](#6-phase-1-cargo-xbin-sync)
7. [Phase 2: `cargo xbin build`](#7-phase-2-cargo-xbin-build)
8. [Determinism and hashing](#8-determinism-and-hashing)
9. [Memory and layout](#9-memory-and-layout)
10. [Priorities and locking](#10-priorities-and-locking)
11. [Boot sequencing (distribution-owned) and reset](#11-boot-sequencing-distribution-owned-and-reset)
12. [Distribution/backend contract](#12-distributionbackend-contract)
13. [Failure modes](#13-failure-modes)
14. [Testing strategy](#14-testing-strategy)
15. [Open questions](#15-open-questions)

---

## 1. Motivation

Modern MCUs increasingly ship multiple cores, and those cores are frequently
**heterogeneous** — different instruction sets and microarchitectures on the
same die. STM32H7 pairs a Cortex-M7 with a Cortex-M4; RP2040 has dual
Cortex-M0+; several ESP32 parts run RISC-V cores.

RTICX already supports multicore **within a single binary** (`cores = N`): one
`#[app]`, one linker image, one build, all cores homogeneous. That model does
not fit heterogeneous cores, which need different target triples, linker
scripts, startup code and memory maps. Each core group naturally wants its own
Cargo binary and its own distribution.

Real workloads want that split: an M7 might run a control loop while an M4
handles sensor I/O, and the two need to exchange **typed** messages at runtime.

The multi-binary extension explores exactly that: **one binary per homogeneous
core group**, where a task in one binary can spawn a software task in another
binary through a typed, cross-core message.

## 2. Problems and constraints

Splitting one system into independently built binaries raises problems a single
binary does not have:

- **Shared data must be byte-identical.** Both binaries must agree on the exact
  layout of every cross-core message, without a shared crate whose version and
  build they would have to keep in step.
- **Topology must be declared once and validated across binaries.** Which task
  runs on which core, at what priority, with what capacity, and which producer
  feeds it — all of this spans binaries that are compiled separately.
- **Shared memory layout must be deterministic.** Both binaries must compute the
  same FIFO offsets and sizes from the same inputs, or they silently disagree at
  runtime.
- **Interrupt coordination spans cores.** Cross-core notifications (doorbells),
  dispatcher interrupt lines and priorities have to be consistent between the
  producer and the consumer.
- **Memory attributes and cache coherency are per-core.** A shared pool must be
  usable from every core that touches it as effectively non-cacheable and
  shared.
- **Independently built binaries drift.** Cargo builds each binary separately; a
  change on one side must be caught on the other, at compile time, with an
  actionable message.
- **Producers depend on consumers.** A producer cannot be type-checked before
  the receiver's task exists, so a naive one-shot build cannot work.

## 3. Goals and non-goals

**Goal.** One binary per homogeneous core group, with cross-binary spawn of
software tasks carrying input values between cores in different binaries.

Three single-source-of-truth rules keep the system consistent:

- the **IDL** is the only source of shared data types;
- the **application syntax** is the only source of tasks, priorities and
  capacities;
- the **receiver's native `#[sw_task]` declaration** is the only task-topology
  declaration — producer applications declare nothing.

The **driver** is the only merger and the only address allocator.

**Out of scope (v1):** async cross-binary tasks, and integration with the root
workspace / release process.

## 4. Design decisions

Each decision below answers a problem from section 2. Together they define the
architecture.

1. **The IDL is the only source of shared types — there is no shared crate.**
   `ipc-types.toml` is parsed once, laid out canonically, and its generated
   `ipc_types.rs` is re-emitted *into* every `#[app]` module. Nothing is added
   to any `Cargo.toml`, so the binaries never have to agree on a crate version.

2. **The receiver declares the task; the producer declares nothing.**
   A cross-binary task is an ordinary `#[sw_task]` with a `spawn_by` core id.
   The build infers the producer from `spawn_by` and generates the sender stub
   (`Task::cross_spawn`) in every producing application. There is exactly one
   declaration of each task.

3. **The build has two phases: `sync`, then `build`.** A producer cannot be
   type-checked before the receiver's task exists, so phase 1 collects metadata
   without type-checking (the pass terminates the compiler), merges and
   allocates, and seals a view; phase 2 compiles each binary against that view.

4. **The sealed `system.json` is the inter-binary contract.** One deterministic
   document carries the topology, the types, the FIFO offsets and the doorbell
   lines. Every binary is compiled against the *same* view, so the two images
   cannot disagree about layout.

5. **`rticx.toml` declares no IPC memory; the distribution owns the pools.**
   Pools, their addresses and aliases, the shared per-dual budget, the
   cache/MPU policy and the linker reservations are distribution vocabulary. The
   project only maps applications to cores and declares visibility.

6. **The transport is generated code; the runtime is a pure data structure.**
   The pool base, the executing core id and the doorbell bodies are resolved at
   compile time and baked into the generated helpers, so `rticx-xbin-rt` has no
   runtime backend trait and threads no runtime backend value.

7. **Allocation is deterministic by construction.** FIFOs are sorted and placed
   byte-identically every run; canonical JSON makes identical inputs produce
   byte-identical `system.json`.

8. **Drift is caught at compile time.** Hashes (`topology_hash`,
   `interface_hash`, `layout_hash`) plus `include_str!` dep-info anchors make a
   stale view, a changed interface or an edited IDL a hard compile error naming
   `cargo xbin sync`.

9. **Nothing touches the root workspace.** The extension is its own workspace,
   and the generated code only uses the already-frozen external `task_trait`
   surface, so the core and its API stay untouched.

10. **v1 uses a coarse critical section for spawning.** `cross_spawn` enqueues
    inside `__rticx_interrupt_free`; a `Producer` shared resource with proper SRP
    locking is the future replacement (see section 15).

## 5. Layers and crates

```text
distribution (out-of-tree H7 / in-tree mock)
  └─ backend: IPC pools (capability binding), cache/MPU, boot sequencing
     (doorbell transport = the generated ring/read functions)
compilation passes
  └─ rticx-xbin-pass: metadata mode + codegen mode
core
  └─ rticx-xbin-proto:  IDL + layout + project manifest + system.json schema
     rticx-xbin-rt:     cross-core SPSC FIFO and ready-queue re-export
     rticx-xbin-driver: cargo-xbin sync/build/verify (
                        ELF verification)
```

The layer rule: the distribution supplies platform facts, the pass consumes
them and emits code, and the core crates hold the shared model and data
structures.

### `rticx-xbin-proto` (core)

- IDL parse/validate and the canonical layout engine.
- The `ipc_types.rs` module generator with change-detecting write
  (`write_if_changed`).
- `rticx.toml` parse/validate (`[[application]]` `core_ids` plus optional
  `external_cores`; no IPC memory).
- `Hash64`.
- `system.json` (schema 3) and manifest (schema 5) schemas.
- Merge + validation + FIFO allocation over the distro capability graph (pool
  matching, shared per-dual budget).
- `system_view` emission and the canonical FIFO image (`fifo`).

### `rticx-xbin-driver` (core)

- **`sync`:** project discovery; per-application `cargo clean -p` +
  `cargo check` metadata collection; IDL parse; merge/validation; FIFO
  allocation; `system.json` emit; `target/rticx-xbin/ipc_types.rs` generation
  with change detection.
- **`build`:** `sync`, then `cargo build --package … --bin …` per application
  with `RTICX_XBIN_SYSTEM` pointing at the just-written view; optionally
  verifying each linked ELF against the pools (`--verify-elf`).
- **`verify`:** the same ELF check for a plain `cargo build` output
  (`--release` for the release profile).

### `rticx-xbin-pass` (compilation pass)

**Metadata mode (phase 1):**

- `#[app]` additions: `device`, optional `cores`, the shared `dispatchers`;
  `core_ids`/`external_cores` rejected as moved to `rticx.toml`.
- Resolves the application's topology from the nearest `rticx.toml`
  (`topology.rs`).
- Parses native `#[sw_task]` cross receivers and the shared `dispatchers` pool;
  steals the cross-line front and re-emits the remainder for the
  software/async pass.
- Emits the per-core distro capability binding (physical core id + reachable
  pools) and `<target>.xbin.json`.
- Then terminates the compiler, so the application is never parsed, analyzed,
  code-generated or type-checked in phase 1. (A producer may call a
  phase-2-only sender stub, and RTICX allows task bodies the `#[app]` macro
  cannot see.)

**Codegen mode (phase 2):**

- Loads `system.json` (`RTICX_XBIN_SYSTEM` or the default project path) for
  every application listed in it, and filters it by the application's cores.
- Emits the generated sender stubs (`pub struct <Task>;` + pool-relative FIFO
  views + `Task::cross_spawn` for every view task it produces).
- Emits the per-pair `__rticx_xbin_ring_{s}_{t}` /
  `__rticx_xbin_read_{s}_{t}` functions from the `XbinPassBackend` templates.
- Emits receiver FIFO views, the injected `CrossCoreMessage` marker trait, the
  `pub mod ipc_types` re-emit of the generated module and the
  `SpawnInput: CrossCoreMessage` const assertion.
- Emits one **line dispatcher** per `(source, priority)` line bound to the entry
  stolen from the front of `dispatchers`, and one **doorbell router** per
  `(source → target)` pair.
- Emits the init hooks (the distribution's
  `XbinPassBackend::configure_shared_memory` tokens inlined on every core, and
  `__rticx_xbin_init_fifos_core<N>()` per producer core; no per-line arming)
  wired into the entry functions via `RticPass::main_injection`.
- Runs the build-phase priority-line validation over the application's raw
  sw/async declarations plus the view's cross receivers.
- Emits the freshness anchors (`__RTICX_XBIN_TOPOLOGY_HASH`, `include_str!` of
  the view, of the generated module, of `rticx.toml` and of the IDL) plus the
  stale-view, stale-interface and stale-IDL hard errors. A discovered
  `rticx.toml` without a synced view is also a hard error, so a plain
  `cargo build` never silently skips code generation.

The pass is bound before `rticx-sw-pass` and requires the distribution's
`swtasks` feature: sw-pass generates the `RticSwTask` trait the injected
`task_trait = RticSwTask` receivers compile through, and the
`__rticx_local_irq_pend` the routers call, so without the feature the generated
code fails on the unresolved trait.

### `rticx-xbin-rt` (core)

- `Fifo<T, DEPTH>` atomic SPSC ring mirroring the canonical image (Vyukov,
  `Release`/`Acquire`, `AtomicU32` head/tail in an 8-byte header,
  `view_at`/`init`/`split`), with a drift guard against `rticx-xbin-proto` and a
  threaded hand-off test.
- The `Queue` re-export backing the generated line ready queues.
- A pure shared-memory data-structure runtime: the pool base, the executing core
  id and the doorbell transport are all resolved at compile time by
  `XbinPassBackend` and baked into the generated code, so there is no runtime
  backend trait and no doorbell methods.

### `rticx-xbin-mock` (test support)

- `MockSystem`: aligned, zeroed in-process IPC **pools**
  (`add_pool(core_a, core_b, size)`, unordered duals, both directions resolving
  to the one backing).
- Per-`(source → target)` condvar **pair doorbell message word** carrying the
  task id to the target's router (`doorbell_send`/`take_message`/`router_wait`).
- Per-handle `current_global_core_id`.
- Two-thread spawn→drain over a raw `Fifo` notified through the pair word.

No public API of the root workspace changes in v1; the generated code only uses
the frozen external `task_trait` surface.

## 6. Phase 1: `cargo xbin sync`

`sync` turns a project's source declarations into one sealed system view.

It parses `rticx.toml` and `ipc-types.toml`, lays out `target/rticx-xbin/`, and
for every `[[application]]` runs `cargo clean -p <package>` +
`cargo check -p <package> --bin <target>` with
`RTICX_XBIN_META_OUT=<output dir>`; the pass writes `<target>.xbin.json`, which
`sync` reads back.

The manifests are then merged and validated by
`rticx_xbin_proto::merge_project` (pool-aware): it resolves global core ids,
matches the two endpoints' distro capability entries into IPC pools, checks the
receiver topology (`spawn_by` visibility in both directions), priority
disjointness, type existence and pool-budget fit, and yields a `MergedProject`
with the deterministic per-task FIFO allocations inside the shared per-dual
budgets. `rticx_xbin_proto::system_view` then emits the sealed `system.json`
view, which `sync` writes.

When the project has an `ipc-types.toml`, `rticx_xbin_proto::generate_module`
builds the generated `target/rticx-xbin/ipc_types.rs` module in memory and
`write_if_changed` rewrites it only when its contents differ. The module is
plain text next to `system.json`; the cross-binary pass re-emits it into each
application's `#[app]` module (section 7), so there is no crate, no
`Cargo.toml` and no `rticx-xbin-rt` dependency to keep in sync. An unchanged IDL
therefore touches nothing, and the CLI reports `target/rticx-xbin/ipc_types.rs
is up to date` (or `created`/`updated` with the path). The
`crates/mock/fixtures/metadata` project covers this end to end.

### Steps

1. Parse `rticx.toml` (`rticx-xbin-proto::parse_project_file`).
2. Per application: `cargo clean -p <package>` (metadata env vars are invisible
   to Cargo fingerprints), then `cargo check` with
   `RTICX_XBIN_META_OUT=<dir>`. The pass writes `<app>.xbin.json`: package/
   target, interface fingerprint, cross receivers with their `SpawnInput` type
   paths, priorities, capacities, and the per-local-core distro capability
   binding (physical core id and reachable pools). The manifest (schema 5)
   carries no topology echo — the application's `core_ids` and `external_cores`
   are read from `rticx.toml` by the pass. There are no sender declarations: the
   driver infers each task's single producer application from the receivers'
   `spawn_by`.
3. Parse `ipc-types.toml`, compute canonical layout, validate the subset.
4. Merge and validate:
   - every receiver's `spawn_by` names a single core in another application, is
     listed in the receiver's `external_cores`, and the producer application
     lists the receiver's core in its `external_cores`;
   - per-target priority lines disjoint across producer cores (collisions with
     the target application's own tasks are checked by the pass at `build`);
   - referenced types exist and are cross-core safe;
   - global core ids consistent across apps and `rticx.toml`;
   - the capability entries match pairwise into pools: same pool id with
     opposite physical cores, the same shared budget and swapped base views
     (`IpcPoolMismatch` otherwise); a used `(source -> target)` direction with no
     pool is `NoIpcPath`; the per-core pool views must not overlap
     (`PoolViewOverlap`).
5. Allocate FIFO offsets deterministically (inside the merge): sort by
   `(source_global, target_global, task_name)`, align each FIFO to 8 bytes,
   depth = `capacity + 1`, and place both directions of a dual inside its one
   shared pool budget. The first FIFO that does not fit fails with the task,
   dual and pool named in the error (`PoolBudgetExceeded`).
6. Write `target/rticx-xbin/system.json` (`rticx_xbin_proto::system_view`:
   copies the merged view, computes the doorbell lines, sets `layout_hash` and
   seals `topology_hash`).
7. Generate/update `target/rticx-xbin/ipc_types.rs` and report changes.

### `system.json` shape (schema 3)

Fields: `schema_version`, `rticx_generation`, `topology_hash`, `layout_hash`,
`apps`, `cores` (each with its distro `physical_core`), `types`, `tasks` (with
a pool-relative `fifo`), `pools` (one entry per dual, with both base views, the
shared `budget` and the bytes `used`), `doorbells`.

The canonical serde types are `rticx_xbin_proto::system::SystemView` (with
`canonical_topology_text`, `seal` and `verify_topology_hash`); the
per-application manifest is `rticx_xbin_proto::manifest::AppManifest`
(`<target>.xbin.json`, written by `rticx-xbin-pass` in metadata mode). This
section is the human-readable reference for both.

## 7. Phase 2: `cargo xbin build`

`cargo xbin build` runs phase 2 for every application. After `sync`, the driver
builds each `[[application]]` with
`cargo build --package <package> --bin <target>` and
`RTICX_XBIN_SYSTEM=<project root>/target/rticx-xbin/system.json`, so the pass
runs in codegen mode and the freshness checks reject a stale view, an interface
changed since `sync`, or a stale IDL.

The checked-in `crates/mock/fixtures/e2e` project (its own mock distribution
over `MockCoreBackend` and `MockBackend`) is the in-tree acceptance: both
binaries compile and link on the host.

The pass reads `system.json` (`RTICX_XBIN_SYSTEM` or the default path), loads
the distribution backend configured with `XbinPass::with_backend` (code
generation needs it; see `rticx_xbin_pass::XbinPassBackend`), filters to its own
cores and emits, per app:

- a generated sender stub (`pub struct <Task>;`) plus the sender-side FIFO view
  at the task's pool view (`base_local`) + pool-relative offset for every view
  task whose `spawner_core` belongs to the application (no local input queue, no
  forwarder). The producer source declares nothing; a generated stub name
  colliding with a user item is a dedicated compile error;
- receiver-side FIFO views plus, per `(source → target, priority)` line, one
  generated **line dispatcher**: a
  `#[task(binds = <dispatchers entry>, priority, core, init = generated)]`
  hardware task whose `exec` drains the line's ready queue and, per popped task,
  drains its FIFO until empty and calls the receiver task's `exec(input)`. One
  **doorbell router** per `(source → target)` pair runs at the pair's highest
  line priority, reads task ids from the pair doorbell word and pends the line
  dispatcher through the software pass's
  `__rticx_local_irq_pend[_core{N}]`; unknown ids are ignored. The native
  `#[sw_task]` receiver structs themselves are rewritten into
  `#[task(priority, core, task_trait = RticSwTask, init = generated)]`
  (preserving `shared`), the same external-`task_trait` shape the single-binary
  software-task pass uses, so the core pass generates the task static and
  enforces the user's `impl RticSwTask`;
- `Task::cross_spawn(input)`: runtime global-core guard, enqueue inside
  `__rticx_interrupt_free` (v1), then call the per-pair ring function
  `__rticx_xbin_ring_{source}_{target}(task_id)`; `Ok(())` / `Err(None)`
  (enqueued, notification failed) / `Err(Some(input))` (not enqueued: full FIFO
  or wrong core);
- init hooks: `configure_shared_memory` on every core before its user `init`
  and `init_fifos_core<N>` on every producer core before its `post_init`. No
  per-line arming: the router IRQs are enabled and prioritized by the core
  pass's used-IRQ machinery;
- IDL types: the injected `pub unsafe trait CrossCoreMessage: Copy + 'static`
  plus a `pub mod ipc_types` that re-emits the tokens of the driver-generated
  `target/rticx-xbin/ipc_types.rs` (wrapped with `use super::CrossCoreMessage;`
  and `#[allow(non_camel_case_types, non_snake_case, unused_imports)]`), so user
  code names `ipc_types::TypeX` inside `#[app]` without a checked-in crate or a
  runtime dependency. The pass rejects a user `CrossCoreMessage`/`ipc_types`
  item in the app module, and an `include_str!` anchor keeps rustc rebuilding
  the application when the file changes; a second `include_str!` anchor on
  `ipc-types.toml` makes a pure IDL edit re-run the macro so the stale-IDL check
  below can fire;
- assertions: layout consts, `__RTICX_XBIN_TOPOLOGY_HASH`, FIFO bounds and
  alignment, the application's interface fingerprint against the view and the
  live IDL layout hash against `layout_hash`, and the
  `SpawnInput: CrossCoreMessage` const assertion per receiver.

Every application listed in the view is processed, including ones that declare
no cross task (their stubs are still generated and every core still runs the
shared-memory configure hook). The generated `#[task]` items use the core pass's
external `task_trait` paths, and the receivers require the distribution's
`swtasks` feature (see section 5), so the root workspace is untouched. Async
cross-binary tasks remain out of scope for v1.

## 8. Determinism and hashing

Identical inputs must produce byte-identical `system.json`: FIFO allocation is
sorted and JSON output is canonical.

### Hashing

`topology_hash` covers the merged view. Each app embeds
`const __RTICX_XBIN_TOPOLOGY_HASH: u64` and fails to compile on mismatch with a
"run `cargo xbin sync`" hint. Phase 2 also:

- recomputes the topology hash from the loaded view (`load_system`);
- recomputes the application's `interface_hash` against the one recorded at sync
  time (`check_interface_hash`);
- recomputes the IDL's live layout hash against the view's `layout_hash`
  (`check_idl_hash`).

Each mismatch is a hard error naming `cargo xbin sync`.

### What the interface fingerprint covers

The fingerprint covers only the parts the driver merges into the project
topology:

- the resolved topology (`cores`, `core_ids`, `external_cores`);
- the stolen cross-dispatcher front (each interrupt path rendered
  host-independently by `write_canonical`);
- the receiver declarations (`name`, `priority`, `capacity`, `core`, `spawn_by`,
  `input_type`).

Deliberately excluded: task bodies, doc/`//` comments, local
`#[sw_task]`/`#[async_task]` declarations, `shared`, `device`/`pacs` and surplus
(local) dispatcher entries — so editing them cannot make a synced application
look stale under rust-analyzer.

A per-field receiver edit (priority/capacity/core/spawn_by/input_type) is
normally reported by the targeted `resolve_receiver` check with a more specific
message before the fingerprint is compared; both name `cargo xbin sync`.

### IDL freshness

The IDL check compares the layout hash the project synced with against the live
`ipc-types.toml` (an absent file compares against `IpcTypes::empty()`). The
generated `ipc_types` module embeds `LAYOUT_HASH`. The pass emits
`include_str!("system.json")` (and, when present, `include_str!("rticx.toml")`
and `include_str!("ipc-types.toml")`) so rustc's dep-info rebuilds when any of
them changes.

## 9. Memory and layout

### Pools are distro-owned

`rticx.toml` declares no IPC memory. The distribution exposes a *capability
binding* — `physical_core(local_core)` and `ipc_pools(local_core)` — and one
**pool per unordered core pair** (a *dual*). Each `IpcPool` names the peer
physical core, this core's view (`base_local`), the peer's view (`base_peer`)
and a shared `budget`.

The driver:

- matches the two endpoints' entries (same id, opposite physical cores, same
  budget, swapped bases; `IpcPoolMismatch` otherwise);
- rejects a used direction with no pool (`NoIpcPath`);
- rejects overlapping pool views on one core (`PoolViewOverlap`);
- allocates both directions of a dual inside the one shared budget, 8-byte
  aligned and deterministically (pool, then task order); an overflow is
  `PoolBudgetExceeded`.

The canonical in-pool image (two 4-byte `AtomicU32` indices, then the element
payload, 8-byte FIFO alignment, depth `capacity + 1`) and the size math live in
`rticx_xbin_proto::fifo`.

### Canonical layout

`repr(C)`, little-endian, natural alignment capped at 4. `bool`, pointers and
64-bit scalars are rejected by the IDL subset.

### Runtime FIFO

`rticx_xbin_rt::Fifo<T, DEPTH>` is an atomic SPSC ring: head/tail as 4-byte
`AtomicU32` indices in an 8-byte header (Release publish / Acquire consume),
element in place, `Copy` payload, `view_at(addr)` placement, `split()`
producer/consumer endpoints. It is deliberately `!Sync`, so safe code cannot
alias the FIFO.

The pass places each view at the pool base recorded in `system.json` (both
directions of a dual share one pool and budget) plus the task's pool-relative
offset; a host/mock backend whose pools are heap-backed substitutes its own base
through `XbinPassBackend::ipc_base_override`.

### Cache/MPU

`XbinPassBackend::configure_shared_memory` makes the executing core's view of the
pools **effectively non-cacheable and shared** by whatever mechanism the
platform provides (see [Cache policy and supported
platforms](#cache-policy-and-supported-platforms)). There are no cache hooks: a
cacheable mapping without transport-owned cache maintenance is unsupported in
v1.

### Link-time verification

`cargo xbin build --verify-elf` / `cargo xbin verify` parse each linked ELF with
`object` and reject an `SHF_ALLOC` section or the stack bound symbol
intersecting a pool view of the application's cores, and a distro
`__rticx_xbin_pool_<id>_start/_end` that disagrees with `system.json`.

### Worked example: layout of `EncryptReq`

`ipc-types.toml` declares `fields = { addr = "u32", len = "u32", key = "u32" }`;
the canonical layout (little-endian, natural alignment capped at 4) is

| Field | Type | Offset | Size |
|---|---|---|---|
| `addr` | `u32` | 0 | 4 |
| `len` | `u32` | 4 | 4 |
| `key` | `u32` | 8 | 4 |

so `size_of = 12` and `align_of = 4`. The generated module emits
`SIZE_ENCRYPT_REQ = 12`, `ALIGN_ENCRYPT_REQ = 4`, `OFF_ENCRYPT_REQ_* = 0/4/8`,
the `LAYOUT_HASH` and a `const` assertion block in which the compiler checks
every number (and `target_endian = "little"`) on the target core — the layout is
a contract, not a hope.

`EncryptTask` has `capacity = 2`, so the driver allocates its FIFO inside the
`{0, 1}` dual's pool at 8-byte alignment:

```text
offset  0   head index (AtomicU32, 4 bytes)
offset  4   tail index (AtomicU32, 4 bytes)
offset  8   elements, depth = capacity + 1 = 3 × 12 bytes
offset 44   end of the FIFO (the next FIFO starts at the next 8-byte boundary, 48)
```

`FIFO_HEADER` (8) and the 8-byte `FIFO_ALIGN` are pinned between
`rticx_xbin_proto::fifo` and the runtime `Fifo<T, DEPTH>`. The wasted ring slot is
what makes the effective capacity exact, because the line dispatcher drains the
FIFO directly instead of queueing a copy.

### Cache policy and supported platforms

> An IPC pool must be usable from every core that touches it as **effectively
> non-cacheable and shared**, by whatever mechanism the platform provides: a
> core with no data cache, an MPU/MMU region attribute, a cache-attribute/PMA
> register, an uncached address alias, or a design-time non-cacheable region.
> This covers the overwhelming majority of multicore embedded MCUs: all
> heterogeneous Cortex-M parts in the M7+M4 class, all homogeneous multicore
> M-class parts whose cores have no data cache, and most multicore RISC-V MCUs.
> A cacheable shared region without the cache maintenance the transport would
> have to perform is unsupported in v1. Cores that also lack cross-core atomic
> primitives (for example ARMv6-M, which has no `LDREX`/`STREX`) additionally
> require a distribution-provided cross-core lock; `portable-atomic`'s
> `critical-section` fallback is local-core only and is not sufficient.

`configure_shared_memory()` runs first in every entry, before the user `init`, so
the mapping is in place before any generated access. The framework cannot
portably detect an MPU/MMU at compile time, so the rule is a documented contract
plus the per-distro hook — not a compile-time gate.

### Reserving pools in the linker scripts (distribution authors)

The distribution owns the pools, their addresses and aliases, the shared budget,
and the linker reservations that keep them out of every binary's allocated data.

Pool addresses are reached through raw fixed-address pointers
(`Fifo::view_at(base + offset)`); a pool is **not** a Rust `static`, so the
linker will not avoid it. If `.data`/`.bss` lands on a pool, the application and
the FIFOs silently share bytes. For each application, take the pool views its
core can see (`base_local` on the endpoint that owns that local core) and keep
allocated data out of those ranges:

1. *Preferred:* put the pools in a dedicated shared SRAM block the firmware uses
   for nothing else, and keep that block out of the `RAM` `MEMORY` regions that
   back `.data`/`.bss`.
2. If a pool sits inside a RAM block, split the `MEMORY` declaration around it
   and give the pool range its own region.

A dual's two endpoints may see the same physical bytes at different addresses
(aliases, e.g. the H7's SRAM3 at `0x3004_0000` for the CM7 and `0x1004_0000`
through the D2 alias for the CM4). Each core reserves **its own view**, on that
core only: the two linker scripts are independent, and reserving different SRAM
blocks on the two cores is a silent aliasing bug.

Give each pool a `NOLOAD` output section so an overflow is a link error rather
than a silent wrap, and export bounds the driver can audit:

```ld
SECTIONS {
  .rticx_xbin_ipc (NOLOAD) : {
    KEEP(*(.rticx_xbin_ipc))
    . = ORIGIN(IPC) + LENGTH(IPC);   /* force the reservation extent */
  } > IPC

  __rticx_xbin_pool_mock_0_1_start = ORIGIN(IPC);
  __rticx_xbin_pool_mock_0_1_end   = ORIGIN(IPC) + LENGTH(IPC);
}
```

`LENGTH(IPC)` must be **>=** the pool's declared `budget`, and the pool must be
mapped effectively non-cacheable and shared (see the cache policy above) with a
base and size that satisfy the platform's alignment rules. Choose addresses that
satisfy the linker and the MPU at once, and re-check whenever the memory map,
the MPU configuration or the capability binding changes. `cargo xbin build
--verify-elf` audits the linked images against the pools (see the link-time
verification bullet above); run it in CI so a drifted linker script fails the
build instead of corrupting IPC at runtime. The check is **detection, not
prevention**: it sees static `SHF_ALLOC` sections only, not runtime stack/heap
growth, and its symbol checks need the unstripped link output.

## 10. Priorities and locking

- **Local resources** use SRP (existing core behaviour). **Cross-binary spawn**
  uses `__rticx_interrupt_free` in v1, with the future `Producer` type as the
  proper SRP-locked replacement.
- **Priority lines on a target core must be disjoint** between local tasks and
  each remote source core. This extends `check_disjoint_priorities` and
  `check_uniform_spawn_by` to the project view. `sync` rejects
  producer-vs-producer collisions from the manifests; the pass rejects a cross
  receiver sharing its line with the target application's own
  `#[sw_task]`/`#[async_task]` declarations, which only exist at `build`. The
  driver never shifts priorities.
- **One dispatcher line per `(source, priority)`** on a target core, taken from
  the cross-line front of the application's shared `dispatchers` pool in
  ascending `(source, priority)` order; the generated dispatcher runs at the
  line's priority. The remainder of `dispatchers` is handed to the
  software/async pass for the local task dispatchers.
- **One doorbell router per `(source → target)` pair** runs at the pair's
  highest line priority and pends the line dispatchers, so a router is at least
  as urgent as every dispatcher it wakes, and enqueueing happens before the
  pend.

## 11. Boot sequencing (distribution-owned) and reset

Boot ordering is **distribution-owned**. The framework defines no cross-core
boot handshake: the generated code only configures shared memory and zeroes the
FIFOs each core produces. A distribution guarantees that its peers are booted
before IPC or the router interrupts are used, most naturally through
`CorePassBackend::post_init` (the H7 and RP2040 distributions release their
secondary core there) or its own `RticPass::main_injection`.

The generated code wires exactly two boot hooks:

- the distribution's `XbinPassBackend::configure_shared_memory(local_core)`
  tokens are inlined at the start of each core's generated entry, before the
  user `init`: memory attributes are per-core, so each core makes its own view of
  the IPC pools effectively non-cacheable and shared before any generated
  access;
- `__rticx_xbin_init_fifos_core<N>()` runs on every core that produces
  cross-binary tasks, before that core's `post_init`: it zeroes the ring indices
  of exactly the FIFOs the core produces (its outbound half of each dual's
  pool), so a topology whose owner core is not an endpoint of a pool still
  initializes correctly.

The router IRQs are enabled and prioritized by the core pass's used-IRQ
machinery from the generated router `#[task(binds = …)]`, so there is no
per-line arming step.

The generated `cross_spawn` does not gate on a target-readiness flag: it checks
the caller's core, enqueues the input in the task's FIFO and rings the pair's
doorbell.

| Result | Meaning |
|---|---|
| `Ok(())` | enqueued and notified |
| `Err(None)` | enqueued but the notification failed; the next spawn's notification drains it, because every line dispatcher drains its FIFOs until empty |
| `Err(Some(input))` | nothing was enqueued — the FIFO is full or the caller runs on the wrong core |

Spawning before the consumer's router IRQ is enabled is safe only while the
distribution's doorbell **latches** (the H7 HSEM status does) and the FIFO lives
in shared memory: the notification is then delivered once the consumer services
the latched doorbell. This is a distribution contract, not a framework
guarantee.

**Recovering from a reset is a distribution responsibility.** The framework
neither detects nor reports one: a distribution that does not handle it can
consume stale FIFO state.

- A reset **producer** discards its own pending inputs when its boot sequence
  re-zeroes the FIFO indices it owns.
- A reset **target's** queued inputs stay in shared memory and drain on the next
  notification once the target runs again.
- Inputs enqueued into a FIFO whose producer resets afterwards are discarded
  with it.

## 12. Distribution/backend contract

The distribution is the single owner of IPC memory: it chooses the pool
addresses and aliases, the shared per-dual budget, the cache/MPU policy and the
linker reservations, and reports them through the single code-generation
contract `rticx_xbin_pass::XbinPassBackend`:

| Binding | Emits / names |
|---|---|
| `physical_core(local_core) -> PhysicalCore` | the distro physical-core id of a local core, used to match the two endpoints of a dual |
| `ipc_pools(local_core) -> Vec<IpcPool>` | the adjacency: every pool (dual) this core can reach, with the peer, both views and the shared budget |
| `ring_doorbell_fn(source, target, template)` | `__rticx_xbin_ring_{source}_{target}(task_id) -> Result<(), ()>` on the producer side |
| `doorbell_interrupt(target, source) -> Ident` | the router's `binds` |
| `read_doorbell_msg_fn(target, source, template)` | `__rticx_xbin_read_{source}_{target}() -> Option<u32>` on the target side |
| `custom_interrupt_path(core)` | the interrupt type the router pends through (mirrors the software pass's backend) |
| `rt_path()` / `ipc_types_path()` | the generated code's runtime `Fifo` path and injected `ipc_types` module path |
| `current_global_core_id()` | the expression of the executing core's global id, used by the `cross_spawn` core guard (required) |
| `configure_shared_memory(local_core)` | the statements making local core `local_core`'s pool view effectively non-cacheable and shared (mechanism-agnostic), inlined at `BeforeInit` |
| `ipc_base_override(view_core, source, target)` | an optional runtime pool-base expression for host/mock backends; real fixed-address distributions keep the default `None` and the pass emits the literal `system.json` address |

A portable implementation writes the task id to a per-pair shared atomic word
and triggers one IRQ; hardware with a payload-capable doorbell can implement the
same contract directly. The router IRQ is enabled and prioritized by the core
pass's used-IRQ machinery (no `doorbell_setup` call remains).

The pass bakes the pool base (the literal `system.json` address, or the
`ipc_base_override` expression) and the executing core id into the generated
helpers, so the generated code threads **no runtime backend value**:
`rticx-xbin-rt` is a pure shared-memory data-structure runtime (`Fifo` plus the
ready `Queue`), and the transport is the generated per-pair ring/read bodies
above. The platform-support rule and the cacheable-mapping caveat are in section
9; there are no cache hooks. Boot sequencing is distribution-owned. The
distribution also ships the linker reservations that keep each pool out of every
binary's `.data`/`.bss`; `cargo xbin build --verify-elf` / `cargo xbin verify`
independently check the linked images against the pools.

## 13. Failure modes

| Failure | Detection | Behaviour |
|---|---|---|
| topologies disagree (receiver `spawn_by` visibility, priorities, FIFO overflow) | `sync` merge | hard error naming the offending task/pool |
| the two endpoints of a dual report inconsistent capabilities | `sync` merge | `IpcPoolMismatch` naming the pool and dual |
| a used `(source -> target)` direction has no distro pool | `sync` merge | `NoIpcPath` naming the task and cores |
| the FIFOs of a dual do not fit its shared budget | `sync` allocation | `PoolBudgetExceeded` with needed vs. reserved bytes |
| two pool views overlap on one core | `sync` merge | `PoolViewOverlap` naming both pools and the core |
| a linked binary places allocated data (or its stack bound) on a pool | `cargo xbin build --verify-elf` / `verify` | `Overlap`/`StackOverlap` naming the application, section/symbol, range and pool; a distro pool bound symbol that disagrees with `system.json` is `PoolBounds` |
| app interface or IDL changed since `sync` | recorded `interface_hash` / `layout_hash` / `TOPOLOGY_HASH` / dep-info | compile error: run `cargo xbin sync` |
| FIFO full or caller on the wrong core | runtime check in `cross_spawn` | `Err(Some(input))`, input returned |
| doorbell ring failed | runtime | `Err(None)` (already enqueued) |

## 14. Testing strategy

The tests are the executable specification; this document and the
[user guide](user-guide.md) describe the same `cargo xbin sync`/`build` path they
exercise.

### Unit and host tests

- **Proto units:** IDL/layout goldens, allocator determinism, every validation
  error, and JSON round-trip.
- **Syntax/extraction:** native-syntax extraction and `spawn_by` resolution
  (identity default, in-app global→local mapping, external classification,
  unknown-core error).
- **Codegen snapshots:** a receiver expansion run through the full core pass and
  compiled on the host; a receiver with `shared` passed through to the core pass;
  an application declaring no cross task receiving its generated stubs; a missing
  `RticSwTask` failing to compile; the generated init hooks executed against the
  mock.
- **Host mock:** two threads over `rticx-xbin-rt`, including backpressure.
- **Cross-compile layout checks:** `thumbv7em-none-eabihf`,
  `thumbv6m-none-eabi`, optionally `riscv32imc`.
- **Capability-graph units** (`proto/tests/merge.rs`): symmetric and mismatched
  entries, impossible links, the shared per-dual budget fitting two directions
  together but not separately, and overflow.
- **Schema units** (`proto/tests/{alloc,system}.rs`): the schema-2 `pools[]` and
  pool-relative FIFO offsets.
- **Pass units:** the pool-pinned sender snapshot and the unknown-pool
  rejection.
- **Driver verification** (`tests/verify.rs`): the linked-ELF verification over
  `crates/mock/fixtures/e2e` (clean pass, an injected overlap naming the section
  and pool, the standalone `verify`).
- **`visualize` module:** one panel per pool (both directions in one panel,
  occupied-extent scaling, the unpooled fallback).

### In-tree fixtures

- **`crates/mock/fixtures/e2e`** — two applications, one cross task, built via
  `cargo xbin build` with its own mock distribution, so the generated phase-2
  code compiles and links on the host. Its extended runtime harness expands both
  applications into one host binary and drives the generated code end to end:
  `cross_spawn` on the sender, the per-pair ring function, the generated router
  ISR, the pended line dispatcher, input verification and FIFO backpressure
  (`Err(Some(input))`), including coalesced and duplicate notifications.
- **`crates/mock/fixtures/three-app`** — two producers onto one receiver, two
  lines and two per-pair routers, built via `cargo xbin build`, alongside the
  build-phase priority-line validation tests (cross vs. core-local sw/async, vs.
  in-app `spawn_by`, cross-producer collisions, same-producer sharing). Its
  extended runtime harness runs three applications in one process: spawns from
  both producer cores drained through their own router and dispatcher,
  per-source backpressure, and the non-owner producer initializing its own pool
  half.
- **`crates/mock/fixtures/metadata`** — binds the mock capability table (shared
  through `xbin-mock-capability`) so `sync` emits `pools[]` and physical core ids
  for the metadata-only fixture too.

### Negative acceptance suite

Each case is asserted against its documented message:

- a plain build without a synced view;
- an interface changed after `sync`;
- a hand-edited `system.json`;
- a priority line shared by two source cores;
- an input type absent from the IDL;
- an edited IDL after `sync`;
- a pool budget too small for its FIFOs.

## 15. Open questions

- **`Producer` shared resource** replacing `cross_spawn` for SRP locking.
- **Async cross-binary tasks and cross-binary locks** are explicitly out of
  scope for v1; the pass requires the distribution's `swtasks` feature.
- **A project-level pool pin** (deliberately not added): pools are distro
  vocabulary, so two images built by one `cargo xbin sync` stay consistent and
  the topology hash catches drift, but a **pre-built peer image** would silently
  break if a distro change relocates a pool. A pin becomes worth adding if an
  independently built peer appears.
- **ELF verification is detection, not prevention**: it sees static `SHF_ALLOC`
  sections only, not runtime stack/heap growth, and its symbol checks need the
  unstripped link output.
- **Pool budgets are shared per dual**: a busy direction can exhaust the other's
  room. A larger distro budget is the documented fix; the per-dual budget is
  deliberate, to avoid fragmentation.
