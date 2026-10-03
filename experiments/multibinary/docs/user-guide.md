# User guide

This guide covers the multi-binary /
heterogeneous multi-core extension as implemented in `experiments/multibinary/`.

The extension is experimental and lives in its own workspace
(`experiments/multibinary/`), deliberately outside the root workspace and its
release process.

1. [Installing `cargo-xbin`](#1-installing-cargo-xbin)
2. [Project layout](#2-project-layout)
3. [Writing `rticx.toml`](#3-writing-rticxtoml)
4. [Writing `ipc-types.toml`](#4-writing-ipc-typestoml)
5. [Declaring cross-binary tasks](#5-declaring-cross-binary-tasks)
6. [Worked example: the two-application fixture](#6-worked-example-the-two-application-fixture)
7. [Building and running](#7-building-and-running)
8. [Common errors](#8-common-errors)

---

## 1. Installing `cargo-xbin`

```bash
cargo install --path experiments/multibinary/crates/rticx-xbin-driver
```

(Run that from the repository root.) Cargo discovers external subcommands by
`<name>` on `PATH`, so `cargo-xbin` is invoked as `cargo xbin`. Verify with:

```bash
cargo xbin --help
```

Run `cargo xbin` from inside a multicore project cargo workspace or one of the crates of the workspace.
the driver discovers the project root by walking up from the current directory until it finds `rticx.toml`, and
runs Cargo from there.

## 2. Multibin Multicore Project layout

```text
my-project/
├── Cargo.toml            # (optional) cargo workspace
├── rticx.toml            # RTICX Multicore project workspace describing the projects topology (cores, applications, IPC regions ..etc)
├── ipc-types.toml        # shared data types, the type-only IDL
├── ipc-types/            # generated crate; kept in VCS, synced by the driver
├── .cargo/config.toml    # target triple, linker, runner (as usual)
└── app-m7/, app-m4/      # one Cargo package per binary
```

`target/rticx-xbin/` is driver output; it is regenerated, not edited:

```text
target/rticx-xbin/
├── <app>.xbin.json       # per-application metadata, written by `cargo xbin sync`
└── system.json           # merged, validated, allocated system view
```

The repository ships complete examples under
`experiments/multibinary/fixtures/`: `e2e/` (two applications, one cross task),
`metadata/` (metadata phase only) and `three-app/` (two producers onto one
receiver). Section 6 walks through `e2e/`.

## 3. Writing `rticx.toml`

*Implemented (M0).* Parsed and validated by `cargo xbin sync`; canonical
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
| `base` | integer | shorthand | region base for **both** cores; expands to equal `base_from_source`/`base_from_target` |
| `base_from_source` | integer | yes\* | region base as seen by the source core |
| `base_from_target` | integer | yes\* | region base as seen by the target core (aliases allowed) |
| `size` | integer | yes | region size in bytes (`>= 1`) |

\* The granular form requires **both** `base_from_source` and `base_from_target`.
`base` is an alternative to that pair and may not be combined with either of
them; the shorthand only applies when the source and target see the region at
the same absolute address.

Addresses and sizes are bare TOML integers; the `0x` prefix is accepted and
the canonical form is lowercase hex. Validation at parse time:

- exactly one `schema = 1`;
- unique `package` names and globally unique core ids;
- region keys of the form `<source>-><target>` with integers and
  `source != target`;
- each region uses either `base` or the `base_from_source`/`base_from_target`
  pair, never a mix;
- the ranges a single core sees do not overlap: every region contributes one
  range to each endpoint core (the source view to the source, the target view
  to the target), and two ranges assigned to the same core must not overlap,
  checked on `[base, base + size)`. This catches a core that would otherwise
  see two different regions at the same addresses (for example the same
  `0x30040000` used both to send to and to receive from another core);
- no unknown top-level, `[[application]]`, `target` or region keys.

Declare only the directions that carry cross-binary tasks: the driver sizes and
allocates a region only for the directions it sees in the application
manifests (`sync` fails with ``needs a `S->T` region`` otherwise).

Example (the `fixtures/e2e` topology, using the shorthand because both cores
see the region at the same address):

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
"0->1" = { base = 0x30040000, size = 4096 }
```

The same region written in the more granular form:

```toml
[ipc.regions]
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x30040000, size = 4096 }
```

Use the granular form when the two cores see the region through **different**
addresses (aliases), for example the D2/M4 alias window of SRAM3:

```toml
[ipc.regions]
# Same physical memory: CM7 sees SRAM3 at 0x3004_0000, CM4 at 0x1004_0000.
"0->1" = { base_from_source = 0x30040000, base_from_target = 0x10040000, size = 4096 }
```

Canonical rendering (`ProjectConfig::to_toml_string`) always expands `base` to
the explicit `base_from_source`/`base_from_target` pair, so tooling that
round-trips a manifest normalizes the shorthand.

`core_ids` is the only place global core ids are declared: the `#[app]` macro
uses them in the runtime core checks, `external_cores`/`spawn_by` refer to
them, and the system view records them. 

## 4. Writing `ipc-types.toml`

*Implemented (M0 parser, M1 generation).* `rticx_xbin_proto::{parse_idl_str,
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

Every cross-binary receiver's `SpawnInput` must be one of these generated
types; the pass emits a const assertion that it implements
`rticx_xbin_rt::CrossCoreMessage`, so a wrong or hand-written type fails at
compile time.

## 5. Declaring cross-binary tasks

*Implemented (M5.5).* Cross-binary receivers are **ordinary native software
tasks**: a `#[sw_task]` struct plus `impl RticSwTask` with a `SpawnInput`
associated type. The extension adds no xbin-specific attribute or trait, and
the receiver declaration is the only declaration of the task. The pass parses
and strips its syntax (the `#[app]` extensions and the cross-receiver
declaration) before the software/core passes run, so other passes and
distributions never see it.

Who declares what:

| Piece | Declared by | Produced by |
|---|---|---|
| receiver task (`#[sw_task]` + `impl RticSwTask`) | receiver application | rewritten into `#[task(…, task_trait = RticSwTask)]` by the pass |
| sender stub (`struct <Task>;` + `Task::cross_spawn`) | nobody | the pass, in every producing application, from `system.json` |
| line dispatcher (`#[task(binds = <ipc_dispatchers entry>)]`) | nobody | the pass, one per `(source, priority)` line |
| doorbell router (`#[task(binds = <pair IRQ>)]`) | nobody | the pass, one per `(source → target)` pair |
| `ipc_dispatchers` pool | receiver application | consumed by the pass |
| per-task FIFO | nobody | driver allocation inside the region, view generated by the pass |
| regions | `rticx.toml` | — |
| shared types | `ipc-types.toml` | the generated `ipc-types/` crate |

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

`RticSwTask` is generated inside the `#[app]` module by the software pass the
distribution binds (its `swtasks` feature), together with the
`__rticx_local_irq_pend` the generated router calls — do not import it from
the distribution.

The receiver's `ipc_dispatchers` pool names the interrupt line of every cross
line on every local core: one entry per distinct `(producer core, priority)`
pair, in ascending `(producer core, priority)` order (a flat array for
`cores = 1`, one inner array per core otherwise). The pool is consumed by the
pass; on each core its entries must be unique and disjoint from that core's
software `dispatchers` entries.

At runtime a spawn travels through the M6.5 dispatch chain: `cross_spawn`
first rejects a target that is not ready (M6-T2, see below), then enqueues the
input in the task's FIFO and calls the generated per-pair ring function, which
publishes the task id on the pair's doorbell word and triggers the target's
**doorbell router**; the router enqueues the task in its line's ready queue and
pends the line's **line dispatcher** from the pool, whose `exec` drains the
task FIFO until empty and calls the receiver's `exec(input)`. Duplicate and
coalesced notifications are idempotent because the dispatcher drains until
empty. 

On one target core every priority level belongs to exactly one origin core:
receivers fed by different producer cores need disjoint priorities. `cargo
xbin sync` rejects producer-vs-producer collisions; at `cargo xbin build` the
pass also rejects a cross receiver that shares its priority with a core-local
`#[sw_task]`/`#[async_task]` or with an in-app `spawn_by` task of the
receiving application, naming the conflicting task. Receivers from the same
producer may share a line. The FIFO indices of each task are initialized by
its producer core before that core's `post_init`, so the boot order and the
owner core's location do not matter.

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

### Ready state and peer reset

A spawn is rejected with `Err(Some(input))` while the target core has not
marked itself ready — including after the target resets: a resetting peer
clears its own ready bit and bumps the shared epoch before re-initializing, so
the spawner notices that its cached epoch is stale. The generated
`cross_spawn` keeps one `ReadyCache` per task: while the epoch is unchanged,
the readiness check costs two atomic loads (the target's ready bit and the
epoch); after a reset the cache refreshes the epoch on the next attempt and
only lets the spawn through once the target has re-marked itself ready in the
new epoch.

Inputs enqueued before a target reset are not lost: they stay in the FIFO and
the next router run after recovery drains them, because every line dispatcher
drains its FIFOs until empty. A reset of a **producer** core discards that
core's pending inputs, since its boot sequence re-zeroes the FIFO indices it
owns.

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

The pass is bound **before** `rticx-sw-pass` and requires the distribution's
`swtasks` feature: the software pass generates the `RticSwTask` trait and the
core pass compiles the receiver through its external `task_trait` mechanism.
Without the feature `RticSwTask` is undefined and the receiver fails to
compile. A distribution that does **not** bind the extension pass only warns
about `external_cores` as an unknown `#[app]` argument (`core_ids` is
understood natively with or without the pass) and treats `spawn_by` as a local
index, so cross-binary declarations require the pass.

## 6. Worked example: the two-application fixture

The repository ships this project at
`experiments/multibinary/fixtures/e2e/`; a fresh checkout can build it
without writing any code. Follow the steps, then use section 7 to run the
same layout on real hardware.

```text
fixtures/e2e/
├── rticx.toml, ipc-types.toml      # sections 3 and 4
├── ipc-types/                      # generated crate (checked in)
├── app-m7/  (binary `m7`, global core 0)   # producer, declares nothing
└── app-m4/  (binary `m4`, global core 1)   # receiver, declares EncryptTask

crates/mock/                        # shared by every fixture
├── mock-pac/                       # stand-in for a real PAC
└── xbin-mock-distro/, xbin-mock-runtime/   # stand-in for a real distribution
```

The mock distribution binds the cross-binary pass and the software pass on a
host-testable backend, so the example compiles and links on your machine; a
real project replaces it with the target distribution and PAC (section 7.2).

**Step 1 — install the driver** (section 1), then go to the fixture:

```bash
cargo install --path experiments/multibinary/crates/rticx-xbin-driver
cd experiments/multibinary/fixtures/e2e
```

**Step 2 — the two configuration files.** `ipc-types.toml` declares one
message — the `EncryptReq` declaration from section 4 — and `rticx.toml` is
exactly the example from section 3: it maps the two binaries to global cores 0
and 1 and declares the single `0->1` region.

**Step 3 — the receiver** (`app-m4/src/main.rs`, abridged):

```rust
use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [1],
    external_cores = [0],
    ipc_dispatchers = [XBIN_IPC_LINE_0]
)]
mod app {
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    pub struct EncryptTask;

    impl RticSwTask for EncryptTask {
        type SpawnInput = ipc_types::EncryptReq;

        fn exec(&mut self, _input: Self::SpawnInput) {
            // the generated line dispatcher calls this once per input
        }
    }

    #[idle]
    struct Idle;

    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            loop {
                core::hint::spin_loop();
            }
        }
    }

    #[init]
    fn init() -> TaskInits {
        TaskInits { idle: Idle }
    }
}
```

**Step 4 — the producer** (`app-m7/src/main.rs`, abridged): it declares no
task at all.

```rust
use xbin_mock_distro::app;

#[app(device = mock_pac, cores = 1, core_ids = [0], external_cores = [1])]
mod app {
    #[idle]
    struct Idle;

    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            loop {
                core::hint::spin_loop();
            }
        }
    }

    #[init]
    fn init() -> TaskInits {
        TaskInits { idle: Idle }
    }

    // A local task or `#[init]` would call, once the view is synced:
    //     EncryptTask::cross_spawn(ipc_types::EncryptReq { addr: 0, len: 0, key: 0 });
}
```

Each application's `Cargo.toml` adds the generated `ipc-types` crate, the
PAC and the distribution, and pins the binary name (the fixture additionally
adds `xbin-mock-runtime` for the host backend):

```toml
[package]
name = "app-m4"
version = "0.1.0"
edition = "2024"

[[bin]]
name = "m4"
path = "src/main.rs"

[dependencies]
ipc-types = { path = "../ipc-types" }
my-pac = "0.1"          # the fixture uses mock-pac
my-distro = "0.1"       # the fixture uses xbin-mock-distro
```

**Step 5 — sync and build** with one command:

```bash
cargo xbin build
```

`build` runs phase 1 (`sync`) and then compiles every application. It streams
the output of each `cargo` step it drives, so the per-application progress is
visible. Checking the project the first time also prints one `created`/`updated`
line per changed file of `ipc-types/`; a checked-in, up-to-date crate prints
nothing else. Abridged:

```text
[cargo-xbin] running `cargo clean --package app-m7`
     Removed 34 files, 10.9MiB total
[cargo-xbin] running `cargo check --package app-m7 --bin m7`
    Checking app-m7 v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.06s
...
[cargo-xbin] running `cargo build --package app-m7 --bin m7`
   Compiling app-m7 v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
[cargo-xbin] running `cargo build --package app-m4 --bin m4`
   Compiling app-m4 v0.1.0 (...)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
[cargo-xbin] ipc-types is up to date
```

**Step 6 — inspect what was generated.**

```text
target/rticx-xbin/
├── m7.xbin.json    # producer manifest: package, cores, source hash, no tasks
├── m4.xbin.json    # receiver manifest: the EncryptTask declaration
└── system.json     # sealed whole-project view
```

`system.json` is the single source of truth phase 2 consumes; the parts that
matter here (abridged):

```jsonc
"tasks": [ {
    "id": 1, "name": "EncryptTask",
    "receiver_core": 1, "spawner_core": 0,
    "priority": 3, "capacity": 2, "input_type": "EncryptReq",
    "fifo": { "source": 0, "target": 1, "offset": 0, "elem_size": 12, "depth": 3 }
} ],
"regions":   [ { "source": 0, "target": 1,
                 "base_from_source": "0x30040000",
                 "base_from_target": "0x30040000", "size": 4096 } ],
"doorbells": [ { "source": 0, "target": 1, "priority": 3, "line": 0 } ]
```

From the view, the pass generated inside the `#[app]` modules:

- in `app-m7`: `pub struct EncryptTask;` with

  ```rust
  pub fn cross_spawn(input: ipc_types::EncryptReq)
      -> Result<(), Option<ipc_types::EncryptReq>>;
  ```

  and its FIFO view at `region 0->1 + 0`;
- in `app-m4`: the `#[sw_task]` rewritten to
  `#[task(priority = 3, task_trait = RticSwTask, init = generated)]`, the FIFO
  view, the `SpawnInput: CrossCoreMessage` assertion, the `EncryptTask` line
  dispatcher bound to `XBIN_IPC_LINE_0`, and the `0->1` doorbell router;
- in both: the ready/epoch entry hooks (`configure_shared_memory` on every
  core, `init_shared` on the owner core 0, `init_fifos` on core 0 as the
  producer, `mark_ready` on every core) and the `__RTICX_XBIN_TOPOLOGY_HASH`
  freshness anchor.

**Step 7 — run it.** `cargo xbin build` compiles the host binaries but does
not run them. The end-to-end behaviour (spawn → ring function → router →
line dispatcher → `exec`, backpressure and the ready/epoch path) is driven by
the in-tree harness, which expands both applications into one process over a
shared mock system. Back in `experiments/multibinary/`:

```bash
cd ../..
cargo test -p rticx-xbin-pass --test e2e_runtime
```

### Multi-source variant

`fixtures/three-app/` is the same project with a second producer
(`app-m5`, global core 2) spawning `SensorTask` onto the same receiver core:

```toml
[ipc.regions]
"0->1" = { base = 0x30040000, size = 4096 }
"2->1" = { base = 0x30041000, size = 4096 }
```

The receiver lists one dispatcher line per cross line, in ascending
`(source, priority)` order, and the two producers use disjoint priorities:

```rust
#[app(
    device = mock_pac,
    cores = 1,
    core_ids = [1],
    external_cores = [0, 2],
    ipc_dispatchers = [XBIN_IPC_LINE_0, XBIN_IPC_LINE_1]  // (0, 3), then (2, 4)
)]
mod app {
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    pub struct EncryptTask;

    #[sw_task(priority = 4, capacity = 1, spawn_by = 2)]
    pub struct SensorTask;
    // ...
}
```

Each producer gets its own region, ring function, router and dispatcher line. See
[architecture §7](architecture.md#7-priorities-and-locking) for why the
priority lines must be disjoint.

## 7. Building and running

### 7.1 `sync` and `build`

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

### 7.2 Running on real hardware

`cargo xbin build` only builds; running is plain Cargo, so the distribution's
runner configuration applies. A typical `.cargo/config.toml` for a Cortex-M
target:

```toml
[build]
target = "thumbv7em-none-eabihf"

[target.thumbv7em-none-eabihf]
# probe-rs, OpenOCD, QEMU or a Renode wrapper, depending on your setup:
runner = "probe-rs run --chip STM32H745ZITx"
rustflags = ["-C", "link-arg=-Tlink.x"]
```

Then:

```bash
cargo xbin sync                 # one system view for every binary
cargo run -p app-m7 --bin m7    # builds and runs/uploads through the runner
cargo run -p app-m4 --bin m4
```

Both binaries must be flashed (or loaded by the simulator) because each owns
its own core. Heterogeneous targets may need per-application settings (a
different triple, linker script or MPU configuration); Cargo supports a
`.cargo/config.toml` per package for that. The in-tree fixtures use the host
mock instead: `.cargo/config.toml` is absent, the apps build for the host, and
the runtime harnesses exercise the generated code (section 7.5). The
out-of-tree STM32H7 acceptance demo (M7) is the reference for a real
M7+M4/Renode setup.

### 7.3 Expected boot order and readiness

Boot sequencing is **distribution-owned**; the generated code only relies on
the ready/epoch protocol:

1. the distribution releases the cores (H7: the CM7 initializes shared memory
   and then releases the CM4 from reset);
2. the **owner core** — the lowest global core id — runs
   `__rticx_xbin_init_shared`: `init_shared()` sets the magic word, bumps the
   epoch and clears every ready bit;
3. **every core** runs `__rticx_xbin_configure_shared_memory` before its user
   `init`, and every core that produces cross-binary tasks zeroes the ring
   indices of exactly the FIFOs it produces at `BeforePostInit`, before its
   `post_init` can spawn;
4. at the end of its `post_init` (`BeforeIdle`) every core calls
   `mark_ready(core)`;
5. a producer that spawns early gets `Err(Some(input))` until the target has
   marked itself ready, so boot order does not matter. After a peer reset the
   same protocol re-synchronizes the producers (section 5, "Ready state and
   peer reset"; [architecture §8](architecture.md#8-boot-readyepoch-and-reset)).

### 7.4 Cache and MPU checklist

The shared region is special memory; getting these wrong is the most common
source of runtime-only failures:

- **Attributes:** map every region Normal, **Non-cacheable, Shareable** (MPU
  or equivalent). Never Device/Strongly-ordered memory: the FIFO's
  `ldrex`/`strex`-based atomics are invalid there.
- **Timing:** `configure_shared_memory()` runs at the start of each entry,
  before the user `init` and before any generated code touches the region, so
  configure it there (the distribution implements the backend method).
- **Linker/MPU consistency:** reserve the region outside every binary's
  `.data`/`.bss`; keep `base_from_source`/`base_from_target` in `rticx.toml`
  consistent with the MPU region (base and limit alignment/size follow your
  device's MPU rules, e.g. power-of-two regions on Cortex-M).
- **Aliases:** if the two cores see the region at different addresses, declare
  both views (`base_from_source`/`base_from_target`); the generated code uses
  the view of the core that runs it. When both cores see the same absolute
  address, use the `base = <addr>` shorthand instead
  (see [`rticx.toml` regions](#3-writing-rticxtoml)).
- **Cacheable fallback (unsupported v1):** if the region cannot be made
  non-cacheable, use the `clean_range`/`invalidate_range` hooks on the
  producer/consumer paths and keep the atomic ordering; the
  [architecture decision tree](architecture.md#6-memory-and-layout) spells
  this out. Prefer non-cacheable.
- **IRQs:** every `ipc_dispatchers` entry and every pair doorbell IRQ must be
  enabled and assigned a priority at least as urgent as the tasks they wake;
  the core pass's used-IRQ machinery does this from the generated
  `#[task(binds = …)]` items, but the distribution must map the IRQ numbers.

### 7.5 Verifying your setup

- `cargo xbin sync` alone validates the whole topology (cores, visibility,
  priorities, types, region fit) and re-renders `ipc-types/`.
- The in-tree fixtures are the executable specification of the documented
  behaviour; from `experiments/multibinary/`:

  ```bash
  cargo test -p rticx-xbin-pass --test e2e_runtime    # two-app spawn/dispatch/ready-epoch
  cargo test -p rticx-xbin-pass --test priority_lines # build-phase priority-line errors
  cargo test -p rticx-xbin-driver --test e2e          # `cargo xbin build` over fixtures/e2e
  cargo test -p rticx-xbin-driver --test three_app    # multi-source fixture
  cargo test -p rticx-xbin-driver --test negative     # documented error messages
  ```

- Compile-time checks catch type and topology drift: layout consts and
  `LAYOUT_HASH` in `ipc-types/`, `__RTICX_XBIN_TOPOLOGY_HASH` and the
  `include_str!` freshness anchor in every application.

## 8. Common errors

*Parser errors (M0):*

| Message (excerpt) | Fix |
|---|---|
| ``unsupported rticx.toml schema version N; this tool supports schema = 1`` | set `schema = 1` or upgrade the driver |
| ``missing required top-level key `schema` `` | add `schema = 1` |
| ``package `x` is declared by more than one `[[application]]` `` | give each binary a unique `package` |
| ``global core id N is declared by both ...`` | core ids must be unique across applications |
| ``key `x` is not of the form `<source>-><target>` `` | quote the key: `"0->1"` |
| ``key `0->0` connects core 0 to itself`` | regions are directional; declare `0->1` and `1->0` separately |
| ``combines the `base` shorthand with `base_from_source` `` | use `base` alone when both views are equal, or both explicit views when they differ |
| ``is missing the required `base_from_source` key`` | set `base` for equal views, or set both `base_from_source` and `base_from_target` |
| ```ipc.regions` overlap on core N`` | on core `N` two regions map to intersecting address ranges; move one region's view (use a distinct `base`/alias) so each core sees disjoint ranges |
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

*Build-phase priority-line errors (M6-T1):*

| Message (excerpt) | Fix |
|---|---|
| ``cross-binary receiver `T` is scheduled at priority P on global core N, but the software task `L` (spawned by global core N) already owns that priority line`` | give the receiver a priority no core-local `#[sw_task]`/`#[async_task]` (or in-app `spawn_by` task) of that core uses |
| ``cross-binary receiver `T` is scheduled at priority P on global core N, but the cross-binary receiver `U` (spawned by global core M) already owns that priority line`` | producers of one target core must declare disjoint priorities; `sync` normally catches this already |

*Freshness/staleness errors (M3-T4, M4-T3):*

| Message (excerpt) | Fix |
|---|---|
| ``failed to read the system view `…/system.json`: …`` | no view was synced (for example after `cargo clean`); run `cargo xbin sync` (or `cargo xbin build`) |
| ``the system view `…/system.json` is stale: its `topology_hash` (…) does not match its contents (…)`` | the view was edited without a full sync; run `cargo xbin sync` (or `cargo xbin build`) |
| ``application `x` changed since the last `cargo xbin sync` (the system view records source hash …, the source hashes to …)`` | the source changed after the last `sync`; run `cargo xbin sync` before a plain `cargo build` |

*Runtime, cache and IRQ symptoms (M6-T3):* these appear only on hardware; the
host fixtures in section 7.5 cannot reproduce them.

| Symptom | Likely cause | Fix |
|---|---|---|
| inputs arrive corrupted, or a receiver sees a stale/zeroed value | region not cache-coherent: cacheable mapping without maintenance, or wrong base alias | map the region Normal, Non-cacheable, Shareable; reconcile `base_from_source`/`base_from_target` with the MPU view; see section 7.4 |
| a hard fault or bus error on the first spawn | region in Device/Strongly-ordered memory (exclusive atomics invalid), or the MPU denies access to one core | map Normal memory and grant both cores the MPU region |
| `cross_spawn` always returns `Err(Some(input))` although the target booted | target never ran `mark_ready` (owner `init_shared` missing, boot released the core but its entry did not run), or its ready bit is not in the shared region | check the generated init hooks and the distribution's boot release; inspect `system.json`/the ready bitmap |
| `Ok(())` but the receiver's `exec` never runs | router or dispatcher IRQ not enabled/mapped, or `ipc_dispatchers` order does not match the view's ascending `(source, priority)` lines | map every pool entry and pair IRQ in the distribution; re-run `cargo xbin sync` and compare `doorbells`/`line` with `ipc_dispatchers` |
| FIFO stays full while the receiver is idle | dispatcher bound to a line whose IRQ never fires; router read the id but the pend is lost | same IRQ mapping check as above; duplicate notifications are harmless (the dispatcher drains until empty) |
| `Err(None)` repeatedly | the input is enqueued but the doorbell notification fails (IRQ masked, doorbell word not shareable) | fix the pair IRQ/word mapping; the enqueued input is drained by the next successful notification |

Every generated application embeds the `__RTICX_XBIN_TOPOLOGY_HASH` it was
built from and `include_str!`s the system view, so rustc's dep-info rebuilds
the application when `target/rticx-xbin/system.json` changes.

Runtime semantics are covered above: a not-ready target (including after a
peer reset) returns `Err(Some(input))` without enqueueing, and a failed
doorbell ring returns `Err(None)` with the input already enqueued (M6-T2).
