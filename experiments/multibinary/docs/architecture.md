# Architecture

The design is specified in
[`multibinary-multicore-plan.md`](../multibinary-multicore-plan.md);
this document condenses it and records the implemented design through M6.5
plus M6-T1–M6-T3. The [user guide](user-guide.md) is the task-oriented
companion; this document explains how the pieces fit together.

1. [Goals and non-goals](#1-goals-and-non-goals)
2. [Layers and crates](#2-layers-and-crates)
3. [Phase 1: `cargo xbin sync`](#3-phase-1-cargo-xbin-sync)
4. [Phase 2: `cargo xbin build`](#4-phase-2-cargo-xbin-build)
5. [Determinism and hashing](#5-determinism-and-hashing)
6. [Memory and layout](#6-memory-and-layout)
7. [Priorities and locking](#7-priorities-and-locking)
8. [Boot, ready/epoch and reset](#8-boot-readyepoch-and-reset)
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
  └─ backend: regions, ready/epoch, cache/MPU
     (doorbell transport = the generated ring/read functions, M6.5)
compilation passes
  └─ rticx-xbin-pass: metadata mode (M1) + codegen mode (M3/M4/M6.5)
core
  └─ rticx-xbin-proto:  IDL + layout + project manifest + system.json schema
     rticx-xbin-rt:     cross-core SPSC FIFO, marker trait, ready/epoch (M2)
     rticx-xbin-driver: cargo-xbin sync/build (M0 skeleton, M1 pipeline)
```

| Crate | State (through M6-T2) |
|---|---|
| `rticx-xbin-proto` | IDL parse/validate, layout engine, crate generator with change-detecting write (`GeneratedCrate::write_if_changed`), `rticx.toml` parse/validate, `Hash64`, `system.json` / manifest schemas, merge + validation + FIFO allocation, `system_view` emission and the canonical FIFO image (`fifo`) |
| `rticx-xbin-driver` | `sync`: project discovery, per-application `cargo clean -p` + `cargo check` metadata collection, IDL parse, merge/validation, FIFO allocation, `system.json` emit, `ipc-types` generation with change detection; `build`: `sync`, then `cargo build --package … --bin …` per application with `RTICX_XBIN_SYSTEM` pointing at the just-written view (M4-T1) |
| `rticx-xbin-pass` | metadata mode: `#[app]` additions, native `#[sw_task]` cross receivers and the `ipc_dispatchers` pool parsed and stripped (M1-T3, M5.5, M6.5-T1), `<target>.xbin.json` emit (M1-T2); codegen mode: loads `system.json` (`RTICX_XBIN_SYSTEM` or the default project path) for every application listed in it (M5.5 removed the cross-declaration gate), filters it by the application's cores and emits the generated sender stubs (`pub struct <Task>;` + FIFO views + `Task::cross_spawn` for every view task it produces, M3-T1/M5.5), the per-pair `__rticx_xbin_ring_{s}_{t}` / `__rticx_xbin_read_{s}_{t}` functions from the `XbinPassBackend` templates (M6.5-T2), the receiver FIFO views, the `SpawnInput: CrossCoreMessage` const assertion, one **line dispatcher** per `(source, priority)` line bound to its `ipc_dispatchers` entry and one **doorbell router** per `(source → target)` pair (M6.5-T3/T4), the init hooks (`__rticx_xbin_init_shared` on the owner, `__rticx_xbin_init_fifos_core<N>` per producer core, `__rticx_xbin_mark_ready_core<N>` per core, no per-line arming) wired into the entry functions via `RticPass::main_injection` (M3-T3/M6-T1), the build-phase priority-line validation over the application's raw sw/async declarations plus the view's cross receivers (M6-T1), the `ReadyCache` ready gate in every generated `cross_spawn` (M6-T2), and the freshness anchors (`__RTICX_XBIN_TOPOLOGY_HASH`, `include_str!` of the view) plus the stale-view and stale-source hard errors (M3-T4); a discovered `rticx.toml` without a synced view is also a hard error, so plain `cargo build` never silently skips code generation (M4-T3). The pass is bound before `rticx-sw-pass` and requires the distribution's `swtasks` feature: sw-pass generates the `RticSwTask` trait the injected `task_trait = RticSwTask` receivers compile through and the `__rticx_local_irq_pend` the routers call, so without the feature the generated code fails on the unresolved trait (M5.5) |
| `rticx-xbin-rt` | marker trait `CrossCoreMessage`; `Fifo<T, DEPTH>` atomic SPSC ring mirroring the canonical image (Vyukov, `Release`/`Acquire`, cache-line-padded indices, `view_at`/`init`/`split`), with a drift guard against `rticx-xbin-proto` and a threaded hand-off test; `SharedState` magic/ready-bitmap/epoch helpers and the spawn-side `ReadyCache` (M2-T2/M6-T2) with stale-epoch reset tests; the `Queue` re-export backing the generated line ready queues (M6.5-T3); `backend::CrossBinBackend` + `IpcRegion` contract (M2-T3) with no-op `configure_shared_memory`/`clean_range`/`invalidate_range` hooks and the Normal/Non-cacheable/Shareable + Device-forbidden rules documented in rustdoc (M2-T4). No doorbell methods: the generated ring/read bodies are the transport (M6.5-T5) |
| `rticx-xbin-mock` | `MockSystem`: aligned, zeroed in-process IPC regions, per-`(source → target)` condvar **pair doorbell message word** carrying the task id to the target's router (`doorbell_send`/`take_message`/`router_wait`, M6.5), per-handle `current_global_core_id`, ready/epoch defaults over one `SharedState` with a peer-reset recovery test (M2-T3/M6-T2); two-thread spawn→drain over a raw `Fifo` notified through the pair word (M2-T3, M6.5-T5) |

No public API of the root workspace changes in v1; the generated code only
uses the frozen external `task_trait` surface.

## 3. Phase 1: `cargo xbin sync`

*Landed through M1-T7:* `sync` parses `rticx.toml` and `ipc-types.toml`, lays
out `target/rticx-xbin/` and, for every `[[application]]`, runs
`cargo clean -p <package>` + `cargo check -p <package> --bin <target>` with
`RTICX_XBIN_META_OUT=<output dir>`; the pass writes `<target>.xbin.json`, which
`sync` reads back. The manifests are then merged and validated by
`rticx_xbin_proto::merge_project` (M1-T5): it resolves global core ids, checks
the receiver topology (`spawn_by` visibility in both directions), priority
disjointness, type existence and region fit, and yields a `MergedProject` with
the deterministic per-task FIFO allocations.
`rticx_xbin_proto::system_view` (M1-T6) then emits the sealed `system.json`
view and `sync` writes it. When the project has an
`ipc-types.toml`, `rticx_xbin_proto::generate_crate` builds the generated
crate in memory and `GeneratedCrate::write_if_changed` (M1-T7) writes only the
files whose contents differ; the `rticx-xbin-rt` dependency is a path relative
to `<project root>/ipc-types`, so the checked-in crate is portable. An
unchanged IDL therefore touches nothing and the CLI reports
`ipc-types is up to date` (or `created`/`updated` per file). The checked-in
`fixtures/metadata` project covers this end to end, including the checked-in
generated crate.

1. Parse `rticx.toml` (`rticx-xbin-proto::parse_project_file`).
2. Per application: `cargo clean -p <package>` (metadata env vars are
   invisible to Cargo fingerprints), then `cargo check` with
   `RTICX_XBIN_META_OUT=<dir>`; the pass writes `<app>.xbin.json` (package/
   target, source hash, cores + `core_ids`, external cores, cross receivers
   with their `SpawnInput` type paths, priorities, capacities). There are no
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
   - global core ids consistent across apps and `rticx.toml`.
5. Allocate FIFO offsets deterministically (inside the merge): sort by
   `(source_global, target_global, task_name)`, align each FIFO to 8 bytes,
   depth = `capacity + 1`; the first FIFO that does not fit fails with the
   task and region named in the error.
6. Write `target/rticx-xbin/system.json`
   (`rticx_xbin_proto::system_view`: copies the merged view, computes the
   doorbell lines, sets `layout_hash` and seals `topology_hash`).
7. Generate/update `ipc-types/` and report changes (M1-T7).

`system.json` shape (schema 1): `schema_version`, `rticx_generation`,
`topology_hash`, `layout_hash`, `apps`, `cores`, `types`, `tasks` (with
per-task `fifo`), `regions`, `doorbells` — see
[plan §7.2](../../../multibinary-multicore-plan.md#72-systemjson-contents).
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
changed since `sync`. The checked-in `fixtures/e2e` project (its own mock
distribution over `MockCoreBackend` and `MockBackend`) is the in-tree
acceptance: both binaries compile and link on the host.*

The pass reads `system.json` (`RTICX_XBIN_SYSTEM` or the default path), loads
the distribution backend configured with `XbinPass::with_backend` (code
generation needs it; see `rticx_xbin_pass::XbinPassBackend`), filters to its
own cores and emits, per app:

- a generated sender stub (`pub struct <Task>;`) plus the sender-side FIFO
  view at `region.base_for(core) + offset` for every view task whose
  `spawner_core` belongs to the application (M3-T1/M5.5; no local input queue,
  no forwarder). The producer source declares nothing; a generated stub name
  colliding with a user item is a dedicated compile error;
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
- `Task::cross_spawn(input)`: runtime global-core guard, target-ready gate
  through the spawner-local `rticx_xbin_rt::ReadyCache` (M6-T2), enqueue inside
  `__rticx_interrupt_free` (v1), call the per-pair ring function
  `__rticx_xbin_ring_{source}_{target}(task_id)`; `Ok(())` / `Err(None)`
  (enqueued, notification failed) / `Err(Some(input))` (not enqueued: full
  FIFO, wrong core or target not ready) (M3-T1/M6.5-T2/M6-T2);
- init hooks: `init_shared`, `mark_ready`, optional
  `configure_shared_memory` (M3-T3). No per-line arming: the router IRQs are
  enabled and prioritized by the core pass's used-IRQ machinery (M6.5-T4);
- assertions: layout consts, `__RTICX_XBIN_TOPOLOGY_HASH`, FIFO bounds and
  alignment (layout consts in M3-T1, freshness in M3-T4), and the
  `SpawnInput: rticx_xbin_rt::CrossCoreMessage` const assertion per receiver
  (M5.5).

Every application listed in the view is processed, including ones that declare
no cross task (their stubs and init hooks are still generated, M5.5). The
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
Generated `ipc-types` embeds `LAYOUT_HASH`. The pass emits
`include_str!("system.json")` so rustc's dep-info rebuilds when the file
changes.

## 6. Memory and layout

- One region per `(source, target)` direction, with per-core base views
  (aliases allowed); one FIFO per cross-binary task inside it. The canonical
  in-region image (two 32-byte-padded ring indices, then the element payload,
  8-byte FIFO alignment, depth `capacity + 1`) and the size math live in
  `rticx_xbin_proto::fifo`; the phase-1 merge checks the FIFOs fit their
  region.
- Canonical layout: `repr(C)`, little-endian, natural alignment capped at 4;
  `bool`/pointers/64-bit scalars rejected by the IDL subset.
- Runtime FIFO (`rticx_xbin_rt::Fifo<T, DEPTH>`, M2-T1): atomic SPSC ring,
  head/tail as `AtomicUsize` (Release publish / Acquire consume), element in
  place, `Copy` payload, cache-line-padded indices, `view_at(addr)` placement,
  `split()` producer/consumer endpoints; deliberately `!Sync` so safe code
  cannot alias the FIFO.
- Cache/MPU: `CrossBinBackend::configure_shared_memory` maps the regions
  Normal, Non-cacheable, Shareable; Device/Strongly-ordered is forbidden
  (`ldrex`/`strex` invalid there). `clean_range`/`invalidate_range` are no-op
  fallback hooks for a cacheable mapping (M2-T4).

### Worked example: layout of `EncryptReq`

`ipc-types.toml` declares `fields = { addr = "u32", len = "u32", key = "u32" }`;
the canonical layout (little-endian, natural alignment capped at 4) is

| Field | Type | Offset | Size |
|---|---|---|---|
| `addr` | `u32` | 0 | 4 |
| `len` | `u32` | 4 | 4 |
| `key` | `u32` | 8 | 4 |

so `size_of = 12` and `align_of = 4`. The generated crate emits
`SIZE_ENCRYPT_REQ = 12`, `ALIGN_ENCRYPT_REQ = 4`, `OFF_ENCRYPT_REQ_* = 0/4/8`,
the `LAYOUT_HASH` and a `const` assertion block in which the compiler checks
every number (and `target_endian = "little"`) on the target core — the layout
is a contract, not a hope.

`EncryptTask` has `capacity = 2`, so the driver allocates its FIFO inside the
`0->1` region at 8-byte alignment:

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
Is the shared region mapped Normal, Non-cacheable, Shareable?
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

## 8. Boot, ready/epoch and reset

Boot sequencing is distribution-owned (H7: CM7 initializes shared memory,
then releases CM4). The project's owner core is the lowest global core id.
`rticx_xbin_rt::SharedState` (M2-T2) supplies the magic word, the atomic ready
bitmap (one bit per global core id, Release/Acquire) and the wrapping epoch
counter: `init()` clears every ready bit and bumps the epoch, and a peer boot
does `clear_ready(own)` + `bump_epoch()` before re-marking. The generated
hooks (M3-T3, M6-T1) implement the protocol: every core runs
`__rticx_xbin_configure_shared_memory` (`configure_shared_memory()`) at the
start of its entry, before the user `init`; the owner application emits
`__rticx_xbin_init_shared`, which runs `init_shared()`, and every core that
produces cross-binary tasks emits `__rticx_xbin_init_fifos_core<N>`, which
zeroes the ring indices of exactly the FIFOs it produces (its own outbound
regions), so a topology whose owner core is not an endpoint of a region still
initializes correctly; every core emits `__rticx_xbin_mark_ready_core<N>`,
which calls `mark_ready(core)` at the end of its `post_init`
(`MainInjectionPoint::BeforeIdle`). The router
IRQs are enabled and prioritized by the core pass's used-IRQ machinery from
the generated router `#[task(binds = …)]`, so there is no per-line arming
step (M6.5-T4).

The generated `cross_spawn` keeps one `rticx_xbin_rt::ReadyCache` per task
(a `static` inside the function, so it cannot collide with user items) and
gates on it before enqueueing: the cache holds the epoch the spawner last
observed with the target ready, `is_ready_at(target, cached)` costs two atomic
loads while it stays valid, and a changed epoch (peer reset, owner
reinitialization) makes the check refresh the epoch once and reject the spawn
with `Err(Some(input))` until the target has re-marked itself ready in the new
epoch. The check is conservative while a reset races it: it may report
`false`, never a stale `true`.

Reset recovery: a reset target clears its own bit and bumps the epoch before
re-initializing, so producers stop spawning into it; once it re-marks ready,
the next spawn attempt refreshes the producer's cached epoch and proceeds.
Inputs enqueued before the reset are not lost while the FIFO indices survive
(they are producer-owned in shared memory): the next router run after recovery
drains the FIFO until empty, so a notification lost during the reset is
recovered by the next spawn's notification. A reset of the **producer** core
discards its pending inputs by design: its boot sequence re-zeroes the FIFO
indices it owns. The residual race — a spawn that passed the ready check just
before the target resets — is narrowed by the check and covered by the
distribution-owned boot sequencing, not eliminated by it; inputs enqueued into
a FIFO whose *producer* resets afterwards are discarded with it.

## 9. Distribution/backend contract

`rticx_xbin_rt::backend::CrossBinBackend` (owned by the extension; defined in
M2-T3/T4, implemented by the out-of-tree H7 distribution and by the in-tree
mock): `ipc_region()`, `configure_shared_memory()` plus the no-op
`clean_range`/`invalidate_range` fallback hooks, `current_global_core_id()`,
`shared_state()` with default `init_shared()`/`mark_ready()`/`is_ready()`/
`epoch()`. It carries **no doorbell methods** (M6.5-T5): the transport is the
generated per-pair ring/read bodies, whose templates the distribution fills
through the code-generation contract
`rticx_xbin_pass::XbinPassBackend`:

| Binding | Emits / names |
|---|---|
| `ring_doorbell_fn(source, target, template)` | `__rticx_xbin_ring_{source}_{target}(task_id) -> Result<(), ()>` on the producer side |
| `doorbell_interrupt(target, source) -> Ident` | the router's `binds` |
| `read_doorbell_msg_fn(target, source, template)` | `__rticx_xbin_read_{source}_{target}() -> Option<u32>` on the target side |
| `custom_interrupt_path(core)` | the interrupt type the router pends through (mirrors the software pass's backend) |

A portable implementation writes the task id to a per-pair shared atomic word
and triggers one IRQ; hardware with a payload-capable doorbell can implement
the same contract directly. The router IRQ is enabled and prioritized by the
core pass's used-IRQ machinery (no `doorbell_setup` call remains, M6.5).

## 10. Failure modes

| Failure | Detection | Behaviour |
|---|---|---|
| topologies disagree (receiver `spawn_by` visibility, priorities, FIFO overflow) | `sync` merge | hard error naming the offending task/region |
| app source changed since `sync` | recorded `source_hash` / `TOPOLOGY_HASH` / dep-info | compile error: run `cargo xbin sync` |
| spawn before target ready | runtime `ReadyCache` ready check | `Err(Some(input))`, input returned (M6-T2) |
| doorbell ring failed | runtime | `Err(None)` (already enqueued) |
| peer reset | epoch mismatch through the cached epoch | spawn rejected until the peer re-marks ready; the next successful spawn refreshes the cache, pending FIFO entries drain on the next notification (M6-T2) |
| region overflow | `sync` allocation | hard error with needed vs. available bytes |

## 11. Testing strategy

Unit (IDL/layout goldens, allocator determinism, every validation error,
native-syntax extraction and `spawn_by` resolution (identity default, in-app
global→local mapping, external classification, unknown-core error, M5.5),
codegen snapshots — including a receiver expansion run through the full core
pass and compiled on the host (M3-T2), a receiver with `shared` passed through
to the core pass, an application declaring no cross task receiving its
generated stubs, a missing `RticSwTask` failing to compile (M5.5), and the
generated init hooks executed against the mock until the owner core is ready
(M3-T3) —, JSON round-trip);
host mock (two threads over `rticx-xbin-rt`, backpressure, ready/epoch,
simulated reset);
cross-compile layout checks (`thumbv7em-none-eabihf`, `thumbv6m-none-eabi`,
optionally `riscv32imc`); the in-tree two-app fixture `fixtures/e2e` built via
`cargo xbin build` (M4-T1), with its own mock distribution so the generated
phase-2 code compiles and links on the host.
M4-T2 expands both fixture applications into one host binary and drives the
generated code end to end: `cross_spawn` on the sender, the per-pair ring
function, the generated router ISR, the pended line dispatcher, input
verification and FIFO backpressure (`Err(Some(input))`), including coalesced
and duplicate notifications (M6.5-T3/T5). M4-T3 adds the negative acceptance
suite: a plain build without a synced view, a source changed after `sync`, a
hand-edited `system.json`, a priority line shared by two source cores, an input
type absent from the IDL and a region too small for its FIFOs, each asserted
against its documented message.
M6-T1 adds the three-application fixture `fixtures/three-app` (two producers
onto one receiver, two lines and two per-pair routers) built via `cargo xbin
build`, the build-phase priority-line validation tests (cross vs. core-local
sw/async, vs. in-app `spawn_by`, cross-producer collisions, same-producer
sharing) and the extended runtime harness: three applications in one process,
spawns from both producer cores drained through their own router and
dispatcher, per-source backpressure, and the non-owner producer initializing
its own region.
M6-T2 adds the `ReadyCache` tests (`rticx-xbin-rt`: not-ready, stale epoch
after a peer reset, recovery on re-mark, refresh after reinitialization), the
mock peer-reset recovery test, the generated-spawn ready-gate snapshot, and
the M4-T2 runtime harness extension: a simulated receiver reset makes the real
generated `cross_spawn` return `Err(Some(input))` without enqueueing, and the
spawn after the receiver re-marks itself ready refreshes the epoch and
executes through the dispatcher.
M6-T3 completes this document and the [user guide](user-guide.md): the
commands and listings there describe the same `cargo xbin sync`/`build` path
that the tests above exercise over `fixtures/e2e` and `fixtures/three-app`, so
a fresh reader follows tested steps.

## 12. Open questions

- `Producer` shared resource replacing `cross_spawn` for SRP locking.
- Async cross-binary tasks and cross-binary locks are explicitly out of scope
  for v1; the pass requires the distribution's `swtasks` feature (M5.5).
