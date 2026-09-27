# User guide (outline)

Status: **outline**. Sections marked *implemented* describe behaviour that
exists through M5.5; the worked examples and troubleshooting are completed by
[M6-T3](../../../multibinary-multicore-plan.md#m6--multi-sourcetarget-readyepoch-complete-docs).

1. [Installing `cargo-xbin`](#1-installing-cargo-xbin)
2. [Project layout](#2-project-layout)
3. [Writing `rticx.toml`](#3-writing-rticxtoml) — *implemented*
4. [Writing `ipc-types.toml`](#4-writing-ipc-typestoml) — *implemented*
5. [Declaring cross-binary tasks](#5-declaring-cross-binary-tasks) — *implemented (M5.5)*
6. [Building and running](#6-building-and-running) — *implemented (M4)*
7. [Common errors](#7-common-errors)

---

## 1. Installing `cargo-xbin`

```bash
cargo install --path experiments/multibinary/crates/rticx-xbin-driver
```

Cargo discovers external subcommands by `<name>` on `PATH`, so `cargo-xbin`
is invoked as `cargo xbin`. Verify with:

```bash
cargo xbin --help
```

## 2. Project layout

The driver discovers the project root by walking up from the current
directory until it finds `rticx.toml`.

```text
my-project/
├── rticx.toml            # project topology (implemented)
├── ipc-types.toml        # shared data types, IDL (implemented)
├── ipc-types/            # generated crate (implemented M1, kept in VCS)
├── .cargo/config.toml    # target triple, linker, runner (as usual)
└── app-m7/, app-m4/      # one Cargo package per binary
```

`target/rticx-xbin/` is driver output; it is regenerated, not edited:

```text
target/rticx-xbin/
├── <app>.xbin.json       # per-application metadata, written by `cargo xbin sync`
└── system.json           # merged, validated, allocated system view
```

## 3. Writing `rticx.toml`

*Implemented in M0:* parsed and validated by `cargo xbin sync`; canonical
rendering is available through `rticx_xbin_proto::ProjectConfig::to_toml_string`.

`schema` must be `1`. Each `[[application]]` declares one binary:

| Key | Type | Required | Meaning |
|---|---|---|---|
| `package` | string | yes | Cargo package name |
| `target` | table | yes | `{ kind = "bin", name = "<bin>", triple = "<triple>"? }`; `kind` is `"bin"` in v1, `triple` overrides `.cargo/config.toml` |
| `core_ids` | array of integers | yes | local core index `i` maps to the globally unique id `core_ids[i]` |

`[ipc.regions]` declares one shared-memory region per ordered core pair,
keyed `"<source>-><target>"`:

| Key | Type | Required | Meaning |
|---|---|---|---|
| `base_from_source` | integer | yes | region base as seen by the source core |
| `base_from_target` | integer | yes | region base as seen by the target core (aliases allowed) |
| `size` | integer | yes | region size in bytes (`>= 1`) |

Addresses and sizes are bare TOML integers; the `0x` prefix is accepted and
the canonical form is lowercase hex. Validation at parse time:

- exactly one `schema = 1`;
- unique `package` names and globally unique core ids;
- region keys of the form `<source>-><target>` with integers and
  `source != target`;
- no unknown top-level, `[[application]]`, `target` or region keys.

Example:

```toml
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]

[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
"1->0" = { base_from_source = 0x30041000, base_from_target = 0x30041000, size = 4096 }
```

See [diagram 1](diagrams/01-project-topology.puml) for the same topology.

## 4. Writing `ipc-types.toml`

*Implemented (M0 parser, M1 generation):* `rticx_xbin_proto::{parse_idl_str,
parse_idl_file}` plus the canonical
[layout engine](architecture.md#6-memory-and-layout) checks the 32-bit-safe
subset. `cargo xbin sync` writes `ipc-types/` from this file: the generated
crate contains the `#[repr(C)]` types, the `SIZE_*`/`ALIGN_*`/`OFF_*` layout
constants, the `LAYOUT_HASH` and the compile-time layout assertions. Only
files whose contents changed are rewritten, so an unchanged IDL is a no-op.

```toml
schema = 1

[message.EncryptReq]
fields = { addr = "u32", len = "u32", key = "u32" }

[message.SensorBatch]
fields = { tag = "u8", xs = "[i16; 8]", state = "State" }

[enum.State]
variants = { Idle = 0, Busy = 1, Failed = 7 }
```

Allowed field types (v1):

| Shape | Examples |
|---|---|
| integers | `u8 u16 u32 i8 i16 i32` |
| float | `f32` |
| fixed array | `[T; N]`, `N >= 1` |
| message reference | a `[message.X]` name |
| enum reference | an `[enum.X]` name |

Rejected with a specific error: `u64`/`i64`/`f64` and wider, `usize`/`isize`,
`bool`, `char`, strings, pointers/references, tuples, generics, zero-sized
and unknown messages, recursive messages. Enums are C-like with `repr(u32)`
semantics: `variants` is either an array of names (implicit discriminants
`0..N` in declaration order) or a table `{ Name = <u32> }` with unique
discriminants in `0..=u32::MAX`.

## 5. Declaring cross-binary tasks

*Implemented (M5.5).* Cross-binary receivers are **ordinary native software
tasks**: a `#[sw_task]` struct plus `impl RticSwTask` with a `SpawnInput`
associated type. The extension adds no xbin-specific attribute or trait, and
the receiver declaration is the only declaration of the task. The pass parses
and strips its syntax (the `#[app]` extensions and the cross-receiver
declaration) before the software/core passes run, so other passes and
distributions never see it.

Receiver binary (the task runs here):

```rust
use my_distro::app;

#[app(
    device = my_pac,
    cores = 1,
    core_ids = [1],
    external_cores = [0],
    ipc_dispatchers = [IPC_LINE_0]
)]
mod app {
    use my_distro::RticSwTask;

    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    pub struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = ipc_types::EncryptReq;

        fn exec(&mut self, input: Self::SpawnInput) {
            // runs on this binary's core when the producer spawns the task
        }
    }
}
```

The receiver's `ipc_dispatchers` pool names the interrupt line of every cross
line on every local core: one entry per distinct `(producer core, priority)`
pair, in ascending `(producer core, priority)` order (a flat array for
`cores = 1`, one inner array per core otherwise). The pool is consumed by the
pass; on each core its entries must be unique and disjoint from that core's
software `dispatchers` entries.

Producer binary (declares nothing; the pass generates the stub from the
synced system view):

```rust
use my_distro::app;

#[app(device = my_pac, cores = 1, core_ids = [0], external_cores = [1])]
mod app {
    // ... `#[idle]`, `#[init]`, local tasks ...

    fn some_task() {
        EncryptTask::cross_spawn(ipc_types::EncryptReq { addr: 0, len: 0, key: 0 });
    }
}
```

The pass generates `pub struct EncryptTask;` and
`EncryptTask::cross_spawn(input) -> Result<(), Option<Input>>` inside the
producer's `#[app]` module for every view task whose producer core belongs to
the application; the producer source never mentions the task and only adds the
generated `ipc-types` crate as a dependency. A generated stub name colliding
with a user item is a dedicated compile error. The error semantics match the
single-binary `cross_spawn`:

| Result | Meaning |
|---|---|
| `Ok(())` | input enqueued, target doorbell rung |
| `Err(None)` | input enqueued, but the doorbell could not be rung; re-notify the target |
| `Err(Some(input))` | nothing enqueued (FIFO full, wrong core, target not ready); retry later or raise `capacity` |

### `#[app]` arguments

| Key | Required | Meaning |
|---|---|---|
| `core_ids = [g...]` | no | local core index `i` → global id `g_i` (identity `0..cores` by default); native to `rticx-core` since M5, understood with or without the pass |
| `external_cores = [g...]` | no | global ids of cores in other binaries visible to this application; consumed by the pass |
| `ipc_dispatchers = [IRQ...]` | yes (when the application has cross-binary receivers) | per-core interrupt lines of the cross-binary line dispatchers: one entry per `(source, priority)` cross line of the core, in ascending `(source, priority)` order; consumed by the pass |

### `spawn_by` resolution

`spawn_by` names the producer core. Without `external_cores` the native
local-index reading applies (`spawn_by < cores`, validated by `rticx-sw-pass`).
In an application that declares `external_cores`, the pass resolves it in the
**global** namespace:

| `spawn_by` | Classification |
|---|---|
| absent | plain native `#[sw_task]`, left untouched |
| ∈ `core_ids` | in-app cross-core task; rewritten to the local index for the software pass |
| ∈ `external_cores` | cross-binary receiver |
| anything else | hard error naming the unknown core |

The identity `core_ids` default makes both readings identical. `core` always
stays a **local** index (`0..cores`, default `0`).

### Cross-receiver arguments

| Key | Required | Meaning |
|---|---|---|
| `priority` | no (default `1`) | priority line on the receiver core; cross receivers require `>= 1` |
| `capacity` | no (default `1`, `>= 1`) | pending inputs the FIFO holds |
| `core` | no (default `0`) | **local** core index running the task |
| `spawn_by` | yes | the single **global** producer core id |
| `init` | no (default `generated`) | must be `generated`; the pass generates the receiver's init |
| `shared` | no | passed through to the core pass, which computes the SRP ceilings |

Rejected with a precise error: unknown, duplicate or multi-segment keys;
non-integer `priority`/`capacity`/`core`; `capacity = 0`; `priority = 0` on a
cross receiver; `spawn_by` as an array (a task has exactly one producer core);
a receiver without an `impl RticSwTask { type SpawnInput = …; }` block; a
receiver `core` outside `0..cores`; `external_cores` overlapping or repeating
the application's own `core_ids`; an `ipc_dispatchers` shape, count, duplicate
or `dispatchers`-overlap violation (the error names the affected core and,
for a missing entry, the cross line).

The receiver's `SpawnInput` must implement `rticx_xbin_rt::CrossCoreMessage`;
the pass emits a const assertion for it instead of adding a bound to the
generated task shape. Async cross-binary tasks (`#[async_task]` receivers) are
**not supported** in v1 and are documented as out of scope.

The pass is bound **before** `rticx-sw-pass` and requires the distribution's
`swtasks` feature: the software pass generates the `RticSwTask` trait and the
core pass compiles the receiver through its external `task_trait` mechanism.
Without the feature `RticSwTask` is undefined and the receiver fails to
compile. A distribution that does **not** bind the extension pass only warns
about `external_cores` as an unknown `#[app]` argument (`core_ids` is
understood natively with or without the pass) and treats `spawn_by` as a local
index, so cross-binary declarations require the pass.

*TODO(M6-T3):* end-to-end worked example and the diagrams that go with it.

## 6. Building and running

*Implemented through M5.5:*

```bash
cargo xbin --help     # CLI overview
cargo xbin sync       # validate rticx.toml, collect <app>.xbin.json per application
cargo xbin build      # sync, then build every application
```

`sync` parses `rticx.toml` and `ipc-types.toml`, creates
`target/rticx-xbin/` and, for every `[[application]]`, runs
`cargo clean -p <package>` followed by
`cargo check -p <package> --bin <target>` with `RTICX_XBIN_META_OUT` pointing
at the output directory. The `#[app]` macro writes `<target>.xbin.json`, which
the driver reads back; a stale manifest is removed before each check, so an
application that stops expanding its `#[app]` macro fails with a clear error.
Without `rticx.toml`, `sync` and `build` stay no-ops.

The collected manifests carry only receiver declarations — there are no sender
declarations since M5.5, the driver infers each task's single producer
application from the receivers' `spawn_by` — and are then merged and validated
(M1-T5): every receiver's `spawn_by` must name a single core in another
application, be listed in the receiver's `external_cores`, and the producer
application must list the receiver's core in its `external_cores`; every
`[[application]]`'s `core_ids`/`external_cores` must be consistent with
`rticx.toml`; every referenced type must exist in the IDL; and the per-task
FIFOs must fit their `(source → target)` region. The same
deterministic pass allocates every FIFO address (8-byte aligned, plan order,
depth `capacity + 1`); the first FIFO that does not fit names its task and
region in the error. The sealed system view is written to
`target/rticx-xbin/system.json` (M1-T6), and — when the project has an
`ipc-types.toml` — the `ipc-types/` crate below the project root is
generated/updated (M1-T7). `sync` prints one line per created or updated file,
or `ipc-types is up to date` when nothing changed; since only changed files
are rewritten, an unchanged IDL does not retrigger builds of the apps that
depend on the crate.

After `sync`, `build` compiles every `[[application]]` with
`cargo build --package <package> --bin <target>` (plus `--target <triple>`
when the application declares one) and `RTICX_XBIN_SYSTEM` pointing at the
`system.json` it just wrote, so the pass generates the cross-binary code and
the freshness checks reject a stale view or a source changed after `sync`
(M4-T1). `build` always runs `sync` first; a plain `cargo build` after an
explicit `sync` is supported, but fails with a staleness error (stale
`system.json` or a source hash mismatch) once sources changed, until the next
`sync`. Without a synced view (for example after `cargo clean`), every
application listed in the view — even one declaring no cross task — fails with
``failed to read the system view …; run `cargo xbin sync` `` instead of
silently generating no code (M5.5 removed the cross-declaration gate).

*TODO(M6-T3):* flashing/running per target, QEMU/Renode runners, cache/MPU
setup checklist, expected boot order.

## 7. Common errors

*Implemented parser errors (M0):*

| Message (excerpt) | Fix |
|---|---|
| ``unsupported rticx.toml schema version N; this tool supports schema = 1`` | set `schema = 1` or upgrade the driver |
| ``missing required top-level key `schema` `` | add `schema = 1` |
| ``package `x` is declared by more than one `[[application]]` `` | give each binary a unique `package` |
| ``global core id N is declared by both ...`` | core ids must be unique across applications |
| ``key `x` is not of the form `<source>-><target>` `` | quote the key: `"0->1"` |
| ``key `0->0` connects core 0 to itself`` | regions are directional; declare `0->1` and `1->0` separately |
| ``references unknown type `T` `` | declare `[message.T]` or `[enum.T]` in `ipc-types.toml` |
| ``cyclic message reference: A -> B -> A`` | break the cycle; messages are stored inline |
| ``message `FooBar` and message `Foo_Bar` would both generate the constant `SIZE_FOO_BAR` `` | rename one IDL type so the generated constants stay unique |
| ``failed to write the generated `ipc-types` crate at `…` `` | fix the permissions or the conflicting path in the project root |

*Metadata collection errors (M1-T4):*

| Message (excerpt) | Fix |
|---|---|
| `` `cargo clean --package x` failed `` | the package name in `[[application]]` must exist in the Cargo workspace |
| `` `cargo check --package x --bin b` failed `` | fix the application's compile errors; `cargo check` diagnostics follow |
| `` `cargo check` for package `x` did not write `.../<b>.xbin.json` `` | bind the cross-binary pass in the application's `#[app]` macro |

*Merge/validation errors (M1-T5):*

| Message (excerpt) | Fix |
|---|---|
| ``application `x` has no manifest`` | the `#[app]` macro of `x` must bind the cross-binary pass |
| ``manifest for package `x` has no matching `[[application]]` `` | remove the stale manifest and re-run `cargo xbin sync` |
| ``rticx.toml maps N core(s) for `x`, but its manifest declares `cores = M` `` | keep `core_ids` in `rticx.toml` and `#[app(cores = …)]` in sync |
| ``… declares `core_ids = …`, but rticx.toml maps …`` | `rticx.toml` is authoritative; reconcile the mapping |
| ``references global core id N … but no application declares it`` | fix the id or declare the owning application |
| ``receiver `T` in `x` is spawned by global core N, but the application does not list it in `external_cores` `` | add the producer core to the receiver application's `external_cores` |
| ``task `T` runs on global core N, but its producer application `x` does not list that core in `external_cores` `` | add the receiver's core to the producer application's `external_cores` |
| ``receiver `T` is declared by both `x` and `y` `` | task names must be unique across the project |
| ``task `T` … uses type `Y`, which is not declared in `ipc-types.toml` `` | declare `[message.Y]`/`[enum.Y]` or fix the path |
| ``tasks `A` … and `B` … share priority P on core N`` | give tasks from different source cores disjoint priority lines |
| ``task `T` needs a `S->T` region`` | add that direction to `[ipc.regions]` |
| ``task `T` does not fit the `S->T` region: N bytes needed, M available`` | enlarge the region or lower task `capacity` |

*Freshness/staleness errors (M3-T4, M4-T3):*

| Message (excerpt) | Fix |
|---|---|
| ``failed to read the system view `…/system.json`: …`` | no view was synced (for example after `cargo clean`); run `cargo xbin sync` (or `cargo xbin build`) |
| ``the system view `…/system.json` is stale: its `topology_hash` (…) does not match its contents (…)`` | the view was edited without a full sync; run `cargo xbin sync` (or `cargo xbin build`) |
| ``application `x` changed since the last `cargo xbin sync` (the system view records source hash …, the source hashes to …)`` | the source changed after the last `sync`; run `cargo xbin sync` before a plain `cargo build` |

Every generated application embeds the `__RTICX_XBIN_TOPOLOGY_HASH` it was
built from and `include_str!`s the system view, so rustc's dep-info rebuilds
the application when `target/rticx-xbin/system.json` changes.

*TODO(M3–M6):* runtime errors (target not ready, doorbell failure), cache/MPU
misconfiguration and the remaining troubleshooting guide.
