# Architecture

The design is specified in
[`multibinary-multicore-plan.md`](../multibinary-multicore-plan.md);
this document condenses it and records the implemented design through M6.9
(distro-owned IPC pools and link-time verification). The
[user guide](user-guide.md) is the task-oriented companion; this document
explains how the pieces fit together.

1. [Goals and non-goals](#1-goals-and-non-goals)
2. [Layers and crates](#2-layers-and-crates)
3. [Phase 1: `cargo xbin sync`](#3-phase-1-cargo-xbin-sync)
4. [Phase 2: `cargo xbin build`](#4-phase-2-cargo-xbin-build)
5. [Determinism and hashing](#5-determinism-and-hashing)
6. [Memory and layout](#6-memory-and-layout)
7. [Priorities and locking](#7-priorities-and-locking)
8. [Boot sequencing (distribution-owned) and reset](#8-boot-sequencing-distribution-owned-and-reset)
9. [Distribution/backend contract](#9-distributionbackend-contract)
10. [Failure modes](#10-failure-modes)
11. [Testing strategy](#11-testing-strategy)
12. [Open questions](#12-open-questions)

---

## 1. Goals and non-goals

One binary per homogeneous core group; cross-binary spawn of software tasks
with input values between cores in different binaries. The IDL is the only
source of shared data types; application syntax is the only source of
tasks/priorities/capacities, and the receiver's native `#[sw_task]`
declaration is the only task-topology declaration — producer applications
declare nothing since M5.5. The driver is the only merger and address
allocator.

Out of scope (v1): async tasks, root-workspace/release integration.


## 2. Layers and crates

```text
distribution (out-of-tree H7 / in-tree mock)
  └─ backend: IPC pools (capability binding), cache/MPU, boot sequencing
     (doorbell transport = the generated ring/read functions, M6.5)
compilation passes
  └─ rticx-xbin-pass: metadata mode (M1) + codegen mode (M3/M4/M6.5/M6.9)
core
  └─ rticx-xbin-proto:  IDL + layout + project manifest + system.json schema
     rticx-xbin-rt:     cross-core SPSC FIFO and backend contract (M2)
     rticx-xbin-driver: cargo-xbin sync/build/verify (M0 skeleton, M1 pipeline,
                        M6.9-T7 ELF verification)
```

| Crate | State (through M6-T2) |
|---|---|
| `rticx-xbin-proto` | IDL parse/validate, layout engine, `ipc_types.rs` module generator with change-detecting write (`write_if_changed`), `rticx.toml` parse/validate (no IPC memory since M6.9-T3), `Hash64`, `system.json` (schema 2) / manifest (schema 2) schemas, merge + validation + FIFO allocation over the distro capability graph (pool matching, shared per-dual budget, M6.9-T4/T5), `system_view` emission and the canonical FIFO image (`fifo`) |
| `rticx-xbin-driver` | `sync`: project discovery, per-application `cargo clean -p` + `cargo check` metadata collection, IDL parse, merge/validation, FIFO allocation, `system.json` emit, `target/rticx-xbin/ipc_types.rs` generation with change detection; `build`: `sync`, then `cargo build --package … --bin …` per application with `RTICX_XBIN_SYSTEM` pointing at the just-written view, optionally verifying each linked ELF against the pools (`--verify-elf`, M6.9-T7); `verify`: the same ELF check for a plain `cargo build` output (`--release` for the release profile) |
| `rticx-xbin-pass` | metadata mode: `#[app]` additions, native `#[sw_task]` cross receivers and the `ipc_dispatchers` pool parsed and stripped (M1-T3, M5.5, M6.5-T1), the per-core distro capability binding (physical core id + reachable pools, M6.9-T2), `<target>.xbin.json` emit (M1-T2), then terminates the compiler so the application is never parsed, analyzed, code-generated or type-checked in phase 1 (a producer may call a phase-2-only sender stub, and RTICX allows task bodies the `#[app]` macro cannot see; M7-T2); codegen mode: loads `system.json` (`RTICX_XBIN_SYSTEM` or the default project path) for every application listed in it (M5.5 removed the cross-declaration gate), filters it by the application's cores and emits the generated sender stubs (`pub struct <Task>;` + pool-relative FIFO views + `Task::cross_spawn` for every view task it produces, M3-T1/M5.5/M6.9-T6), the per-pair `__rticx_xbin_ring_{s}_{t}` / `__rticx_xbin_read_{s}_{t}` functions from the `XbinPassBackend` templates (M6.5-T2), the receiver FIFO views, the injected `CrossCoreMessage` marker trait plus the `pub mod ipc_types` re-emit of the generated module and the `SpawnInput: CrossCoreMessage` const assertion, one **line dispatcher** per `(source, priority)` line bound to its `ipc_dispatchers` entry and one **doorbell router** per `(source → target)` pair (M6.5-T3/T4), the init hooks (`__rticx_xbin_configure_shared_memory` on every core and `__rticx_xbin_init_fifos_core<N>` per producer core, no per-line arming) wired into the entry functions via `RticPass::main_injection` (M3-T3/M6-T1), the build-phase priority-line validation over the application's raw sw/async declarations plus the view's cross receivers (M6-T1), and the freshness anchors (`__RTICX_XBIN_TOPOLOGY_HASH`, `include_str!` of the view and of the generated module) plus the stale-view and stale-source hard errors (M3-T4); a discovered `rticx.toml` without a synced view is also a hard error, so plain `cargo build` never silently skips code generation (M4-T3). The pass is bound before `rticx-sw-pass` and requires the distribution's `swtasks` feature: sw-pass generates the `RticSwTask` trait the injected `task_trait = RticSwTask` receivers compile through and the `__rticx_local_irq_pend` the routers call, so without the feature the generated code fails on the unresolved trait (M5.5) |
| `rticx-xbin-rt` | `Fifo<T, DEPTH>` atomic SPSC ring mirroring the canonical image (Vyukov, `Release`/`Acquire`, cache-line-padded indices, `view_at`/`init`/`split`), with a drift guard against `rticx-xbin-proto` and a threaded hand-off test; the `Queue` re-export backing the generated line ready queues (M6.5-T3); `backend::CrossBinBackend` + `IpcRegion` contract (M2-T3) whose `ipc_region(source, target)` returns the dual's shared pool from each endpoint's view (both directions the same pool and budget, M6.9-T6), with no-op `configure_shared_memory`/`clean_range`/`invalidate_range` hooks and the Normal/Non-cacheable/Shareable + Device-forbidden rules documented in rustdoc (M2-T4). No doorbell methods: the generated ring/read bodies are the transport (M6.5-T5) |
| `rticx-xbin-mock` | `MockSystem`: aligned, zeroed in-process IPC **pools** (`add_pool(core_a, core_b, size)`, unordered duals, both directions resolving to the one backing, M6.9-T6), per-`(source → target)` condvar **pair doorbell message word** carrying the task id to the target's router (`doorbell_send`/`take_message`/`router_wait`, M6.5), per-handle `current_global_core_id`; two-thread spawn→drain over a raw `Fifo` notified through the pair word (M2-T3, M6.5-T5) |

No public API of the root workspace changes in v1; the generated code only
uses the frozen external `task_trait` surface.

## 3. Phase 1: `cargo xbin sync`

*Landed through M1-T7:* `sync` parses `rticx.toml` and `ipc-types.toml`, lays
out `target/rticx-xbin/` and, for every `[[application]]`, runs
`cargo clean -p <package>` + `cargo check -p <package> --bin <target>` with
`RTICX_XBIN_META_OUT=<output dir>`; the pass writes `<target>.xbin.json`, which
`sync` reads back. The manifests are then merged and validated by
`rticx_xbin_proto::merge_project` (M1-T5; pool-aware since M6.9-T4): it resolves
global core ids, matches the two endpoints' distro capability entries into IPC
pools, checks the receiver topology (`spawn_by` visibility in both directions),
priority disjointness, type existence and pool-budget fit, and yields a
`MergedProject` with the deterministic per-task FIFO allocations inside the
shared per-dual budgets.
`rticx_xbin_proto::system_view` (M1-T6) then emits the sealed `system.json`
view and `sync` writes it. When the project has an
`ipc-types.toml`, `rticx_xbin_proto::generate_module` builds the generated
`target/rticx-xbin/ipc_types.rs` module in memory and `write_if_changed`
(M1-T7) rewrites it only when its contents differ. The module is plain text
next to `system.json`; the cross-binary pass re-emits it into each
application's `#[app]` module (section 4), so there is no crate, no
`Cargo.toml` and no `rticx-xbin-rt` dependency to keep in sync. An
unchanged IDL therefore touches nothing and the CLI reports
`target/rticx-xbin/ipc_types.rs is up to date` (or `created`/`updated` with
the path). The `crates/mock/fixtures/metadata` project covers this end to end.

1. Parse `rticx.toml` (`rticx-xbin-proto::parse_project_file`).
2. Per application: `cargo clean -p <package>` (metadata env vars are
   invisible to Cargo fingerprints), then `cargo check` with
   `RTICX_XBIN_META_OUT=<dir>`; the pass writes `<app>.xbin.json` (package/
   target, source hash, cores + `core_ids`, external cores, cross receivers
   with their `SpawnInput` type paths, priorities, capacities, and the
   per-local-core distro capability binding: physical core id and reachable
   pools, M6.9-T2). There are no
   sender declarations since M5.5: the driver infers each task's single
   producer application from the receivers' `spawn_by`.
3. Parse `ipc-types.toml`, compute canonical layout, validate the subset.
4. Merge and validate:
   - every receiver's `spawn_by` names a single core in another application,
     is listed in the receiver's `external_cores`, and the producer
     application lists the receiver's core in its `external_cores`;
   - per-target priority lines disjoint across producer cores (collisions with
     the target application's own tasks are checked by the pass at `build`,
     M6-T1);
   - referenced types exist and are cross-core safe;
   - global core ids consistent across apps and `rticx.toml`;
   - the capability entries match pairwise into pools: same pool id with
     opposite physical cores, the same shared budget and policy and swapped
     base views (`IpcPoolMismatch` otherwise); a used `(source -> target)`
     direction with no pool is `NoIpcPath`; the per-core pool views must not
     overlap (`PoolViewOverlap`), M6.9-T4.
5. Allocate FIFO offsets deterministically (inside the merge): sort by
   `(source_global, target_global, task_name)`, align each FIFO to 8 bytes,
   depth = `capacity + 1`, and place both directions of a dual inside its one
   shared pool budget; the first FIFO that does not fit fails with the task,
   dual and pool named in the error (`PoolBudgetExceeded`, M6.9-T4).
6. Write `target/rticx-xbin/system.json`
   (`rticx_xbin_proto::system_view`: copies the merged view, computes the
   doorbell lines, sets `layout_hash` and seals `topology_hash`).
7. Generate/update `target/rticx-xbin/ipc_types.rs` and report changes (M1-T7).

`system.json` shape (schema 2): `schema_version`, `rticx_generation`,
`topology_hash`, `layout_hash`, `apps`, `cores` (each with its distro
`physical_core`), `types`, `tasks` (with a pool-relative `fifo`),
`pools` (one entry per dual, with both base views, the shared `budget` and the
bytes `used`), `doorbells` — see
[plan §7.2](../multibinary-multicore-plan.md#72-systemjson-contents).
The canonical serde types are `rticx_xbin_proto::system::SystemView` (with
`canonical_topology_text`, `seal` and `verify_topology_hash`); the
per-application manifest is `rticx_xbin_proto::manifest::AppManifest`
(`<target>.xbin.json`, written by `rticx-xbin-pass` in metadata mode). This
section is the human-readable reference for both.

## 4. Phase 2: `cargo xbin build`

*Sender side landed in M3-T1, the receiver dispatchers in M3-T2, the init
hooks in M3-T3 and the M6.5 router/line-dispatcher split in M6.5; the
freshness anchors and hard errors landed in M3-T4. `cargo xbin build` runs
phase 2 for every application in M4-T1: after
`sync`, the driver builds each `[[application]]` with
`cargo build --package <package> --bin <target>` and
`RTICX_XBIN_SYSTEM=<project root>/target/rticx-xbin/system.json`, so the pass
runs in codegen mode and the freshness checks reject a stale view or a source
changed since `sync`. The checked-in `crates/mock/fixtures/e2e` project (its own mock
distribution over `MockCoreBackend` and `MockBackend`) is the in-tree
acceptance: both binaries compile and link on the host.*

The pass reads `system.json` (`RTICX_XBIN_SYSTEM` or the default path), loads
the distribution backend configured with `XbinPass::with_backend` (code
generation needs it; see `rticx_xbin_pass::XbinPassBackend`), filters to its
own cores and emits, per app:

- a generated sender stub (`pub struct <Task>;`) plus the sender-side FIFO
  view at the task's pool view (`base_local`) + pool-relative offset for every
  view task whose
  `spawner_core` belongs to the application (M3-T1/M5.5/M6.9-T6; no local input
  queue, no forwarder). The producer source declares nothing; a generated stub
  name colliding with a user item is a dedicated compile error;
- receiver-side FIFO views plus, per `(source → target, priority)` line, one
  generated **line dispatcher**: a `#[task(binds = <ipc_dispatchers entry>,
  priority, core, init = generated)]` hardware task whose `exec` drains the
  line's ready queue and, per popped task, drains its FIFO until empty and
  calls the receiver task's `exec(input)` (M6.5-T4). One **doorbell router**
  per `(source → target)` pair runs at the pair's highest line priority, reads
  task ids from the pair doorbell word and pends the line dispatcher through
  the software pass's `__rticx_local_irq_pend[_core{N}]`; unknown ids are
  ignored (M6.5-T3). The native `#[sw_task]` receiver structs themselves are
  rewritten into `#[task(priority, core, task_trait = RticSwTask,
  init = generated)]` (preserving `shared`), the same external-`task_trait`
  shape the single-binary software-task pass uses, so the core pass generates
  the task static and enforces the user's `impl RticSwTask` (M3-T2/M5.5);
- `Task::cross_spawn(input)`: runtime global-core guard, enqueue inside
  `__rticx_interrupt_free` (v1), then call the per-pair ring function
  `__rticx_xbin_ring_{source}_{target}(task_id)`; `Ok(())` / `Err(None)`
  (enqueued, notification failed) / `Err(Some(input))` (not enqueued: full
  FIFO or wrong core) (M3-T1/M6.5-T2);
- init hooks: `configure_shared_memory` on every core before its user `init`
  and `init_fifos_core<N>` on every producer core before its `post_init`
  (M3-T3/M6-T1). No per-line arming: the router IRQs are enabled and
  prioritized by the core pass's used-IRQ machinery (M6.5-T4);
- IDL types: the injected `pub unsafe trait CrossCoreMessage: Copy + 'static`
  plus a `pub mod ipc_types` that re-emits the tokens of the driver-generated
  `target/rticx-xbin/ipc_types.rs` (wrapped with `use super::CrossCoreMessage;`
  and `#[allow(non_camel_case_types, non_snake_case, unused_imports)]`), so user
  code names `ipc_types::TypeX` inside `#[app]` without a checked-in crate or a
  runtime dependency. The pass rejects a user `CrossCoreMessage`/`ipc_types`
  item in the app module, and an `include_str!` anchor keeps rustc rebuilding
  the application when the file changes (M1-T7);
- assertions: layout consts, `__RTICX_XBIN_TOPOLOGY_HASH`, FIFO bounds and
  alignment (layout consts in M3-T1, freshness in M3-T4), and the
  `SpawnInput: CrossCoreMessage` const assertion per receiver (M5.5).

Every application listed in the view is processed, including ones that declare
no cross task (their stubs are still generated and every core still runs the
shared-memory configure hook, M5.5). The
generated `#[task]` items use the core pass's external `task_trait` paths, and
the receivers require the distribution's `swtasks` feature (see section 2), so
the root workspace is untouched. Async cross-binary tasks remain out of scope
for v1.

## 5. Determinism and hashing

Identical inputs must produce byte-identical `system.json`. FIFO allocation
is sorted, JSON output is canonical. `topology_hash` covers the merged view;
each app embeds `const __RTICX_XBIN_TOPOLOGY_HASH: u64` and fails to compile
on mismatch with a "run `cargo xbin sync`" hint. Phase 2 recomputes the
topology hash from the loaded view (`load_system`, M3-T4) and recomputes the
application's `source_hash` against the one recorded at sync time
(`check_source_hash`, M3-T4); both are hard errors with the documented hint.
`source_hash` is computed from a canonical, proc-macro-host-independent
serialization of the application's token tree, not from
`TokenStream::to_string()`: rustc and rust-analyzer's proc-macro server render
the same tokens with different punctuation spacing and formatting, so hashing
the rendering would make a view synced by cargo look stale in the IDE.
Generated `ipc_types` module embeds `LAYOUT_HASH`. The pass emits
`include_str!("system.json")` so rustc's dep-info rebuilds when the file
changes.

## 6. Memory and layout

- **Pools are distro-owned (M6.9).** `rticx.toml` declares no IPC memory. The
  distribution exposes a *capability binding* — `physical_core(local_core)` and
  `ipc_pools(local_core)` — and one **pool per unordered core pair** (a
  *dual*). Each `IpcPool` names the peer physical core, this core's view
  (`base_local`), the peer's view (`base_peer`), a shared `budget` and a
  `CachePolicy`. The driver matches the two endpoints' entries (same id,
  opposite physical cores, same budget/policy, swapped bases;
  `IpcPoolMismatch` otherwise) and rejects a used direction with no pool
  (`NoIpcPath`). Both directions of a dual allocate inside the one shared
  budget, 8-byte aligned, deterministically (pool, then task order); an
  overflow is `PoolBudgetExceeded`. A core's pool views must be pairwise
  disjoint (`PoolViewOverlap`, checked from the distro-reported views). The
  canonical in-pool image (two 32-byte-padded ring indices, then the element
  payload, 8-byte FIFO alignment, depth `capacity + 1`) and the size math live
  in `rticx_xbin_proto::fifo`.
- Canonical layout: `repr(C)`, little-endian, natural alignment capped at 4;
  `bool`/pointers/64-bit scalars rejected by the IDL subset.
- Runtime FIFO (`rticx_xbin_rt::Fifo<T, DEPTH>`, M2-T1): atomic SPSC ring,
  head/tail as `AtomicUsize` (Release publish / Acquire consume), element in
  place, `Copy` payload, cache-line-padded indices, `view_at(addr)` placement,
  `split()` producer/consumer endpoints; deliberately `!Sync` so safe code
  cannot alias the FIFO. `CrossBinBackend::ipc_region(source, target)` returns
  the dual's pool from each endpoint's view; both directions return the same
  pool and budget (M6.9-T6).
- Cache/MPU: `CrossBinBackend::configure_shared_memory` maps the pools
  Normal, Non-cacheable, Shareable; Device/Strongly-ordered is forbidden
  (`ldrex`/`strex` invalid there). `clean_range`/`invalidate_range` are no-op
  fallback hooks for a cacheable mapping (M2-T4).
- Link-time verification (M6.9-T7): `cargo xbin build --verify-elf` /
  `cargo xbin verify` parse each linked ELF with `object` and reject an
  `SHF_ALLOC` section or the stack bound symbol intersecting a pool view of the
  application's cores, and a distro `__rticx_xbin_pool_<id>_start/_end` that
  disagrees with `system.json`.

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
every number (and `target_endian = "little"`) on the target core — the layout
is a contract, not a hope.

`EncryptTask` has `capacity = 2`, so the driver allocates its FIFO inside the
`{0, 1}` dual's pool at 8-byte alignment:

```text
offset   0   head index, one 32-byte cache-line slot
offset  32   tail index, one 32-byte cache-line slot
offset  64   elements, depth = capacity + 1 = 3 × 12 bytes
offset 100   end of the FIFO (the next FIFO starts at the next 8-byte boundary)
```

`FIFO_HEADER` (64) and the 8-byte `FIFO_ALIGN` are pinned between
`rticx_xbin_proto::fifo` and the runtime `Fifo<T, DEPTH>`. The wasted ring slot
is what makes the effective capacity exact, because the line dispatcher drains
the FIFO directly instead of queueing a copy.

### Cache-maintenance decision tree

```text
Is the shared pool mapped Normal, Non-cacheable, Shareable?
├─ yes → the v1 supported path: atomics order accesses, no maintenance
└─ no (cacheable mapping)
   ├─ can the MPU/attributes be fixed? → do that; the fallback is unsupported
   └─ otherwise, via the CrossBinBackend fallback hooks:
      producer: clean_range before publishing the element
      consumer: invalidate_range before consuming it
      both: keep the Release/Acquire atomic ordering and any required barriers
Device/Strongly-ordered memory is never an option: ldrex/strex are invalid.
```

`configure_shared_memory()` runs first in every entry, before the user `init`,
so the attributes are in place before any generated access.

## 7. Priorities and locking

- SRP for local resources (existing core behaviour); cross-binary spawn uses
  `__rticx_interrupt_free` in v1, with the future `Producer` type as the
  proper SRP-locked replacement.
- Priority lines on a target core must be disjoint between local tasks and
  each remote source core (extends `check_disjoint_priorities` and
  `check_uniform_spawn_by` to the project view). `sync` rejects
  producer-vs-producer collisions from the manifests; the pass rejects a cross
  receiver sharing its line with the target application's own
  `#[sw_task]`/`#[async_task]` declarations, which only exist at `build`
  (M6-T1). The driver never shifts priorities.
- One dispatcher line per `(source, priority)` on a target core, taken from
  the application's `ipc_dispatchers` pool in ascending `(source, priority)`
  order; the generated dispatcher runs at the line's priority. One doorbell
  router per `(source → target)` pair runs at the pair's highest line
  priority and pends the line dispatchers, so a router is at least as urgent
  as every dispatcher it wakes and enqueueing happens before the pend
  (M6.5).

## 8. Boot sequencing (distribution-owned) and reset

Boot ordering is **distribution-owned**. The framework defines no cross-core
boot handshake: the generated code only configures shared memory and zeroes the
FIFOs each core produces. A distribution guarantees that its peers are booted
before IPC or the router interrupts are used, most naturally through
`CorePassBackend::post_init` (the H7 and RP2040 distributions release their
secondary core there) or its own `RticPass::main_injection`.

The generated code emits exactly two boot hooks (M3-T3, M6-T1):

- `__rticx_xbin_configure_shared_memory` runs on every core at the start of its
  entry, before the user `init`, and calls
  `CrossBinBackend::configure_shared_memory()`: MPU/MMU attributes are
  per-core, so each core maps its own view of the IPC pools before any
  generated access;
- `__rticx_xbin_init_fifos_core<N>` runs on every core that produces
  cross-binary tasks, before that core's `post_init`: it zeroes the ring
  indices of exactly the FIFOs the core produces (its outbound half of each
  dual's pool), so a topology whose owner core is not an endpoint of a pool
  still initializes correctly.

The router IRQs are enabled and prioritized by the core pass's used-IRQ
machinery from the generated router `#[task(binds = …)]`, so there is no
per-line arming step (M6.5-T4).

The generated `cross_spawn` does not gate on a target-readiness flag: it checks
the caller's core, enqueues the input in the task's FIFO and rings the pair's
doorbell. `Ok(())` means enqueued and notified; `Err(None)` means enqueued but
the notification failed (the next spawn's notification drains it, because every
line dispatcher drains its FIFOs until empty); `Err(Some(input))` means nothing
was enqueued because the FIFO is full or the caller runs on the wrong core.

Spawning before the consumer's router IRQ is enabled is safe only while the
distribution's doorbell **latches** (the H7 HSEM status does) and the FIFO
lives in shared memory: the notification is then delivered once the consumer
services the latched doorbell. This is a distribution contract, not a framework
guarantee.

**Recovering from a reset is a distribution responsibility.** The framework
neither detects nor reports one: a distribution that does not handle it can
consume stale FIFO state. A reset producer discards its own pending inputs
when its boot sequence re-zeroes the FIFO indices it owns; a reset target's queued inputs
stay in shared memory and drain on the next notification once the target runs
again. Inputs enqueued into a FIFO whose producer resets afterwards are
discarded with it.

## 9. Distribution/backend contract

The distribution is the single owner of IPC memory (M6.9): it chooses the pool
addresses and aliases, the shared per-dual budget, the cache/MPU policy and the
linker reservations, and reports them through the code-generation contract
`rticx_xbin_pass::XbinPassBackend`:

| Binding | Emits / names |
|---|---|
| `physical_core(local_core) -> PhysicalCore` | the distro physical-core id of a local core, used to match the two endpoints of a dual (M6.9-T1) |
| `ipc_pools(local_core) -> Vec<IpcPool>` | the adjacency: every pool (dual) this core can reach, with the peer, both views, the shared budget and the policy (M6.9-T1) |
| `ring_doorbell_fn(source, target, template)` | `__rticx_xbin_ring_{source}_{target}(task_id) -> Result<(), ()>` on the producer side |
| `doorbell_interrupt(target, source) -> Ident` | the router's `binds` |
| `read_doorbell_msg_fn(target, source, template)` | `__rticx_xbin_read_{source}_{target}() -> Option<u32>` on the target side |
| `custom_interrupt_path(core)` | the interrupt type the router pends through (mirrors the software pass's backend) |

A portable implementation writes the task id to a per-pair shared atomic word
and triggers one IRQ; hardware with a payload-capable doorbell can implement
the same contract directly. The router IRQ is enabled and prioritized by the
core pass's used-IRQ machinery (no `doorbell_setup` call remains, M6.5).

The runtime half, `rticx_xbin_rt::backend::CrossBinBackend` (owned by the
extension; defined in M2-T3/T4, implemented by the out-of-tree H7 distribution
and by the in-tree mock): `ipc_region(source, target)` returns the dual's pool
from each endpoint's view (both directions the same pool and budget, M6.9-T6),
`configure_shared_memory()` plus the no-op
`clean_range`/`invalidate_range` fallback hooks, and
`current_global_core_id()`. It carries **no doorbell methods** (M6.5-T5): the
transport is the generated per-pair ring/read bodies above, and boot sequencing
is distribution-owned. The distribution also ships the
linker reservations that keep each pool out of every binary's `.data`/`.bss`;
`cargo xbin build --verify-elf` / `cargo xbin verify` independently check the
linked images against the pools (M6.9-T7).

## 10. Failure modes

| Failure | Detection | Behaviour |
|---|---|---|
| topologies disagree (receiver `spawn_by` visibility, priorities, FIFO overflow) | `sync` merge | hard error naming the offending task/pool |
| the two endpoints of a dual report inconsistent capabilities | `sync` merge | `IpcPoolMismatch` naming the pool and dual (M6.9-T4) |
| a used `(source -> target)` direction has no distro pool | `sync` merge | `NoIpcPath` naming the task and cores (M6.9-T4) |
| the FIFOs of a dual do not fit its shared budget | `sync` allocation | `PoolBudgetExceeded` with needed vs. reserved bytes (M6.9-T4) |
| two pool views overlap on one core | `sync` merge | `PoolViewOverlap` naming both pools and the core (M6.9-T4) |
| a linked binary places allocated data (or its stack bound) on a pool | `cargo xbin build --verify-elf` / `verify` | `Overlap`/`StackOverlap` naming the application, section/symbol, range and pool; a distro pool bound symbol that disagrees with `system.json` is `PoolBounds` (M6.9-T7) |
| app source changed since `sync` | recorded `source_hash` / `TOPOLOGY_HASH` / dep-info | compile error: run `cargo xbin sync` |
| FIFO full or caller on the wrong core | runtime check in `cross_spawn` | `Err(Some(input))`, input returned |
| doorbell ring failed | runtime | `Err(None)` (already enqueued) |

## 11. Testing strategy

Unit (IDL/layout goldens, allocator determinism, every validation error,
native-syntax extraction and `spawn_by` resolution (identity default, in-app
global→local mapping, external classification, unknown-core error, M5.5),
codegen snapshots — including a receiver expansion run through the full core
pass and compiled on the host (M3-T2), a receiver with `shared` passed through
to the core pass, an application declaring no cross task receiving its
generated stubs, a missing `RticSwTask` failing to compile (M5.5), and the
generated init hooks executed against the mock (M3-T3) —, JSON round-trip);
host mock (two threads over `rticx-xbin-rt`, backpressure);
cross-compile layout checks (`thumbv7em-none-eabihf`, `thumbv6m-none-eabi`,
optionally `riscv32imc`); the in-tree two-app fixture `crates/mock/fixtures/e2e` built via
`cargo xbin build` (M4-T1), with its own mock distribution so the generated
phase-2 code compiles and links on the host.
M4-T2 expands both fixture applications into one host binary and drives the
generated code end to end: `cross_spawn` on the sender, the per-pair ring
function, the generated router ISR, the pended line dispatcher, input
verification and FIFO backpressure (`Err(Some(input))`), including coalesced
and duplicate notifications (M6.5-T3/T5). M4-T3 adds the negative acceptance
suite: a plain build without a synced view, a source changed after `sync`, a
hand-edited `system.json`, a priority line shared by two source cores, an input
type absent from the IDL and a pool budget too small for its FIFOs, each
asserted against its documented message.
M6-T1 adds the three-application fixture `crates/mock/fixtures/three-app` (two producers
onto one receiver, two lines and two per-pair routers) built via `cargo xbin
build`, the build-phase priority-line validation tests (cross vs. core-local
sw/async, vs. in-app `spawn_by`, cross-producer collisions, same-producer
sharing) and the extended runtime harness: three applications in one process,
spawns from both producer cores drained through their own router and
dispatcher, per-source backpressure, and the non-owner producer initializing
its own pool half.
M6-T3 completes this document and the [user guide](user-guide.md): the
commands and listings there describe the same `cargo xbin sync`/`build` path
that the tests above exercise over `crates/mock/fixtures/e2e` and `crates/mock/fixtures/three-app`, so
a fresh reader follows tested steps.
M6.9 makes the distro the single owner of IPC memory: `proto/tests/merge.rs`
pins the capability graph (symmetric/mismatched entries, impossible links, the
shared per-dual budget fitting two directions together but not separately, and
overflow), `proto/tests/{alloc,system}.rs` pin the schema-2 `pools[]` and
pool-relative FIFO offsets, the pass codegen tests pin the pool-pinned sender
snapshot and the unknown-pool rejection, the driver's `tests/verify.rs` checks
the linked-ELF verification over `crates/mock/fixtures/e2e` (clean pass, an injected
overlap naming the section and pool, the standalone `verify`), and the
`visualize` module tests pin one panel per pool (both directions in one panel,
occupied-extent scaling, the unpooled fallback). `crates/mock/fixtures/metadata` binds the
mock capability table (shared through `xbin-mock-capability`) so `sync` emits
`pools[]` and physical core ids for the metadata-only fixture too (M6.9-T9).

## 12. Open questions

- `Producer` shared resource replacing `cross_spawn` for SRP locking.
- Async cross-binary tasks and cross-binary locks are explicitly out of scope
  for v1; the pass requires the distribution's `swtasks` feature (M5.5).
- A **project-level pool pin** (M6.9 decided against one): pools are distro
  vocabulary, so two images built by one `cargo xbin sync` stay consistent and
  the topology hash catches drift, but a **pre-built peer image** would silently
  break if a distro change relocates a pool. A pin becomes worth adding if an
  independently built peer appears.
- **ELF verification is detection, not prevention** (M6.9-T7): it sees static
  `SHF_ALLOC` sections only, not runtime stack/heap growth, and its symbol
  checks need the unstripped link output.
- Pool budgets are shared per dual (M6.9): a busy direction can exhaust the
  other's room. A larger distro budget is the documented fix; the per-dual
  budget is deliberate, to avoid fragmentation.
