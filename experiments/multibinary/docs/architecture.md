# Architecture (outline)

Status: **outline**. The design is specified in
[`multibinary-multicore-plan.md`](../../../multibinary-multicore-plan.md);
this document condenses it and records which parts exist in M0. Completed by
[M5-T3](../../../multibinary-multicore-plan.md#m5--multi-sourcetarget-readyepoch-complete-docs).

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
tasks/priorities/capacities; the driver is the only merger and address
allocator.

Out of scope (v1): async tasks, cross-binary `#[shared]` resources/locks,
zero-copy bulk transfer, dynamic memory, more than one binary per core group,
root-workspace/release integration.

*TODO(M5-T3): link the acceptance demo description (STM32H7 M7+M4 under
Renode, M6).*

## 2. Layers and crates

```text
distribution (out-of-tree H7 / in-tree mock)
  └─ backend: regions, doorbells, ready/epoch, cache/MPU
compilation passes
  └─ rticx-xbin-pass: metadata mode (M1) + codegen mode (M3/M4)
core
  └─ rticx-xbin-proto:  IDL + layout + project manifest + system.json schema
     rticx-xbin-rt:     cross-core SPSC FIFO, marker trait, ready/epoch (M2)
     rticx-xbin-driver: cargo-xbin sync/build (M0 skeleton, M1 pipeline)
```

| Crate | State (M1) |
|---|---|
| `rticx-xbin-proto` | IDL parse/validate, layout engine, crate generator with change-detecting write (`GeneratedCrate::write_if_changed`), `rticx.toml` parse/validate, `Hash64`, `system.json` / manifest schemas, merge + validation + FIFO allocation, `system_view` emission and the canonical FIFO image (`fifo`) |
| `rticx-xbin-driver` | `sync`: project discovery, per-application `cargo clean -p` + `cargo check` metadata collection, IDL parse, merge/validation, FIFO allocation, `system.json` emit, `ipc-types` generation with change detection; `build` = `sync` for now (app builds in M4) |
| `rticx-xbin-pass` | metadata mode: `#[app]` additions and `#[cross_bin_task]`/`#[cross_bin_spawn]` syntax confirmed and stripped (M1-T3), `<target>.xbin.json` emit (codegen lands in M3) |
| `rticx-xbin-rt` | marker trait `CrossCoreMessage` (FIFO lands in M2) |
| `rticx-xbin-mock` | empty (M2) |

No public API of the root workspace changes in v1; the generated code only
uses the frozen external `task_trait` surface.

## 3. Phase 1: `cargo xbin sync`

*Landed through M1-T7:* `sync` parses `rticx.toml` and `ipc-types.toml`, lays
out `target/rticx-xbin/` and, for every `[[application]]`, runs
`cargo clean -p <package>` + `cargo check -p <package> --bin <target>` with
`RTICX_XBIN_META_OUT=<output dir>`; the pass writes `<target>.xbin.json`, which
`sync` reads back. The manifests are then merged and validated by
`rticx_xbin_proto::merge_project` (M1-T5): it resolves global core ids, checks
the sender/receiver match, priority disjointness, type existence and region
fit, and yields a `MergedProject` with the deterministic per-task FIFO
allocations. `rticx_xbin_proto::system_view` (M1-T6) then emits the sealed
`system.json` view and `sync` writes it. When the project has an
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
   target, source hash, cores + `core_ids`, external cores, spawn stubs,
   receiver tasks, type paths, priorities, capacities).
3. Parse `ipc-types.toml`, compute canonical layout, validate the subset.
4. Merge and validate:
   - sender stub ↔ receiver task (name, input type, capacity, target priority);
   - per-target priority lines disjoint across remote sources and local tasks;
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

*Planned (M3/M4); M0 runs `sync` and stops.*

The pass reads `system.json` (`RTICX_XBIN_SYSTEM` or the default path),
filters to its own cores and emits, per app:

- FIFO views for every cross task at `base[core_view] + offset` (no local
  input queue, no forwarder);
- one doorbell ISR per remote `(source → target, priority)` line and one
  generated dispatcher per priority line that drains its FIFOs and calls
  `exec`;
- `Task::cross_spawn(input)`: runtime global-core guard, enqueue inside
  `__rticx_interrupt_free` (v1), ring the doorbell; `Ok(())` / `Err(None)`
  (enqueued, doorbell failed) / `Err(Some(input))` (not enqueued);
- init hooks: `init_shared`, `mark_ready`, doorbell arming before ready,
  optional `configure_shared_memory`;
- assertions: layout consts, `TOPOLOGY_HASH`, FIFO bounds and alignment.

Generated `#[task]` items use the core pass's external `task_trait` paths, so
the root workspace is untouched.

## 5. Determinism and hashing

Identical inputs must produce byte-identical `system.json`. FIFO allocation
is sorted, JSON output is canonical. `topology_hash` covers the merged view;
each app embeds `const TOPOLOGY_HASH: u64` and fails to compile on mismatch
with a "run `cargo xbin sync`" hint. Generated `ipc-types` embeds
`LAYOUT_HASH`. The pass emits `include_str!("system.json")` so rustc's
dep-info rebuilds when the file changes.

## 6. Memory and layout

- One region per `(source, target)` direction, with per-core base views
  (aliases allowed); one FIFO per cross-binary task inside it. The canonical
  in-region image (two 32-byte-padded ring indices, then the element payload,
  8-byte FIFO alignment, depth `capacity + 1`) and the size math live in
  `rticx_xbin_proto::fifo`; the phase-1 merge checks the FIFOs fit their
  region.
- Canonical layout: `repr(C)`, little-endian, natural alignment capped at 4;
  `bool`/pointers/64-bit scalars rejected by the IDL subset.
- Runtime FIFO: atomic SPSC ring, head/tail as `AtomicUsize` (Release publish
  / Acquire consume), element in place, `Copy` payload, cache-line-padded
  indices, `view_at(addr)` placement.
- Cache/MPU: regions configured Normal, Non-cacheable, Shareable; Device/
  Strongly-ordered forbidden (`ldrex`/`strex` invalid there); optional
  `clean_range`/`invalidate_range` for cacheable fallback.

*TODO(M5-T3): byte-layout worked example (message → size/align/offsets) and
cache-maintenance decision tree.*

## 7. Priorities and locking

- SRP for local resources (existing core behaviour); cross-binary spawn uses
  `__rticx_interrupt_free` in v1, with the future `Producer` type as the
  proper SRP-locked replacement.
- Priority lines on a target core must be disjoint between local tasks and
  each remote source core (extends `check_disjoint_priorities` and
  `check_uniform_spawn_by` to the project view).
- One doorbell line per `(source, target, priority)`; the doorbell ISR runs at
  the highest priority and pends the matching dispatcher.

## 8. Boot, ready/epoch and reset

Boot sequencing is distribution-owned (H7: CM7 initializes shared memory,
then releases CM4). The extension supplies an atomic ready bitmap and an
epoch word; `cross_spawn` returns `Err` while the target is not ready, and a
peer reset is detected through the epoch. `init_shared` zeroes indices and
sets magic/epoch; `mark_ready(core)` runs at the end of each core's
`post_init` after arming the doorbell.

## 9. Distribution/backend contract

`CrossBinBackend` (owned by the extension; implemented out-of-tree and by the
mock): `ipc_regions()`, `configure_shared_memory()`, optional cache range ops,
`doorbell_setup/ring`, dispatcher IRQ paths, `current_global_core_id()`,
`init_shared()`, `mark_ready()`, `is_ready()`, `epoch()`.

## 10. Failure modes

| Failure | Detection | Behaviour |
|---|---|---|
| topologies disagree (sender/receiver, priorities, FIFO overflow) | `sync` merge | hard error naming the offending task/region |
| app source changed since `sync` | `TOPOLOGY_HASH` / dep-info | compile error: run `cargo xbin sync` |
| spawn before target ready | runtime ready check | `Err(Some(input))`, input returned |
| doorbell ring failed | runtime | `Err(None)` (already enqueued) |
| peer reset | epoch mismatch | reassess ready state; report via runtime API |
| region overflow | `sync` allocation | hard error with needed vs. available bytes |

## 11. Testing strategy

Unit (IDL/layout goldens, allocator determinism, every validation error,
codegen snapshots, JSON round-trip); host mock (two threads over
`rticx-xbin-rt`, backpressure, ready/epoch, simulated reset); cross-compile
layout checks (`thumbv7em-none-eabihf`, `thumbv6m-none-eabi`, optionally
`riscv32imc`); in-tree two-app fixture built via `cargo xbin build`.

## 12. Open questions

- Auto-generating sender stubs from receiver declarations (later relaxation).
- `Producer` shared resource replacing `cross_spawn` for SRP locking.
- Async and cross-binary locks are explicitly out of scope for v1.
