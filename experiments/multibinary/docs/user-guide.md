# User guide

This guide shows how to build a **multi-binary / heterogeneous multicore**
RTICX application: one Cargo binary per core, where a task in one binary can
spawn a software task in another binary through a typed, cross-core message.

The extension is **experimental** and lives in its own workspace
(`experiments/multibinary/`), deliberately outside the root workspace and its
release process. This guide is task-oriented; the internals — the two-phase
build, hashing, memory layout, locking and the distribution/backend contract —
are documented in [architecture.md](architecture.md).

**Contents**

1. [What you get](#1-what-you-get)
2. [Install `cargo-xbin`](#2-install-cargo-xbin)
3. [Project layout](#3-project-layout)
4. [Project topology: `rticx.toml`](#4-project-topology-rticxtoml)
5. [Shared types: `ipc-types.toml`](#5-shared-types-ipc-typestoml)
6. [Declaring cross-binary tasks](#6-declaring-cross-binary-tasks)
7. [Worked example](#7-worked-example)
8. [Building and running](#8-building-and-running)
9. [Common errors](#9-common-errors)

---

## 1. What you get

- **One binary per core.** A project is a Cargo workspace whose members are the
  per-core applications. Each application is a normal RTICX `#[app]`; tasks,
  priorities, resources and `#[init]`/`#[idle]` work exactly as in a
  single-binary application, and each owns exactly one global core id.
- **Cross-binary spawn.** A producer calls `Task::cross_spawn(input)`. The input
  travels through a driver-allocated FIFO and is delivered to the
  `exec(input)` of a software task declared in the receiving binary.
- **Typed shared data.** Shared message and enum types are declared once in a
  small IDL file and generated into every application — no shared crate.
- **Validated at build time.** `cargo xbin sync` checks the topology, types,
  priorities and pool budgets before anything is compiled; `cargo xbin build`
  compiles every binary against that sealed view.

## 2. Install `cargo-xbin`

From the repository root:

```bash
cargo install --path experiments/multibinary/crates/rticx-xbin-driver
```

Cargo discovers external subcommands by `<name>` on `PATH`, so `cargo-xbin` is
invoked as `cargo xbin`. Verify with:

```bash
cargo xbin --help
```

Run `cargo xbin` from inside a multicore project's Cargo workspace (or from any
of its crates). The driver finds the project root by walking up from the current
directory until it sees `rticx.toml`, and runs Cargo from there.

## 3. Project layout

```text
my-project/
├── Cargo.toml            # (optional) Cargo workspace
├── rticx.toml            # topology: which core each binary runs on
├── ipc-types.toml        # shared data types (the type-only IDL)
├── .cargo/config.toml    # target triple, linker, runner (as usual)
└── app-m7/, app-m4/      # one Cargo package per binary
```

`cargo xbin` writes everything it generates under `target/rticx-xbin/`. That
directory is regenerated, never edited:

```text
target/rticx-xbin/
├── <app>.xbin.json       # per-application metadata, written by `cargo xbin sync`
├── system.json           # the merged, validated, allocated system view
└── ipc_types.rs          # the generated IDL types; re-emitted into each `#[app]`
```

Complete examples ship under `experiments/multibinary/crates/mock/fixtures/`:
`e2e/` (two applications, one cross task), `metadata/` (metadata phase only) and
`three-app/` (two producers onto one receiver). Section 7 walks through `e2e/`.

## 4. Project topology: `rticx.toml`

`rticx.toml` is the only place the project's cores are declared. `schema` must
be `1`, and each `[[application]]` describes one binary running on exactly one
core:

| Key | Required | Meaning |
|---|---|---|
| `package` | yes | Cargo package name |
| `target` | yes | `{ kind = "bin", name = "<bin>", triple = "<triple>"? }`; `kind` is `"bin"` in v1, `triple` overrides `.cargo/config.toml` |
| `core_id` | yes | this binary's globally unique core id |

`core_id` must be unique across applications. 

The project declares **no IPC memory** and **no cross-binary edges**. The
distribution owns the IPC pools (their addresses, aliases and shared budgets)
and exposes which core pairs it can reach; each cross-binary edge is instead
inferred from the receiving task's `spawn_by` (section 6).

```toml
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_id = 0

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_id = 1
```

These global core ids are what `spawn_by` refers to, and what the generated code
uses in its runtime core checks. `#[app]` may not declare `cores` argument.

## 5. Shared types: `ipc-types.toml`

Cross-binary messages must be `#[repr(C)]`, 32-bit-safe types. Declare them once
here; `cargo xbin sync` generates `target/rticx-xbin/ipc_types.rs`, and the
cross-binary pass injects it into every application's `#[app]` module as
`pub mod ipc_types`, so user code writes `ipc_types::EncryptReq` with no crate
dependency.

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
`bool`, `char`, strings, pointers/references, tuples, generics, zero-sized and
unknown messages, and recursive messages. Enums are C-like (`repr(u32)`):
`variants` is either an array of names (implicit discriminants `0..N`) or a table
`{ Name = <u32> }` with unique discriminants.

Every cross-binary receiver's `SpawnInput` must be one of these generated types.
The pass emits a const assertion that it implements the injected
`CrossCoreMessage` marker, so a wrong or hand-written type fails at compile time.
Inside `#[app]` and its descendants the module is reachable as `ipc_types`; from
an external module (for example one holding `impl RticSwTask`) import it with
`crate::app::ipc_types::…` — see [Declaring cross-binary tasks](#6-declaring-cross-binary-tasks).

## 6. Declaring cross-binary tasks

Cross-binary receivers are **ordinary RTICX software tasks**: a `#[sw_task]`
struct plus an `impl RticSwTask` with a `SpawnInput` associated type. There is
no xbin-specific attribute or trait, and the receiver declaration is the *only*
declaration of the task.

**What you write vs. what is generated.** You write the receiver and call the
sender. The build generates everything between them:

| Piece | Written by | Produced by |
|---|---|---|
| receiver task (`#[sw_task]` + `impl RticSwTask`) | receiver application | rewritten into a native `#[task(…, task_trait = RticSwTask)]` |
| sender stub (`pub struct <Task>;` + `Task::cross_spawn`) | nobody | the build, in every producing application |
| FIFOs, line dispatchers, doorbell routers, shared-memory setup | nobody | the build |

### The receiver

The task runs here. Declare it like any software task and give it the producer's
global core id via `spawn_by`:

```rust
use my_distro::app;

#[app(
    device = my_pac,
    dispatchers = [IPC_LINE_0]
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

`RticSwTask` (and the machinery the generated dispatcher calls) is provided
inside `#[app]` by the software pass the distribution binds through its
`swtasks` feature — do not import it from the distribution.

`dispatchers` is one shared pool of interrupt lines per application. **The
cross-binary lines come first**: one entry per distinct `(producer core,
priority)` pair, in ascending order, followed by the local task dispatchers the
software/async pass assigns (one per local priority group, ascending). The list
is **flat** — an application owns exactly one core, so the nested
one-array-per-core shape is a hard error — and within the cross-line front every
entry must be unique.

A `#[sw_task]` at one target core may be fed by only one origin core per
priority: receivers fed by different producer cores need disjoint priorities
(see [priorities and locking](architecture.md#10-priorities-and-locking)).
Receivers from the same producer may share a line.

### The producer

The producer declares nothing — the build generates the sender stub from the
synced system view. Just call it:

```rust
use my_distro::app;

#[app(device = my_pac)]
mod app {
    // ... `#[idle]`, `#[init]`, local tasks ...

    fn some_task() {
        EncryptTask::cross_spawn(ipc_types::EncryptReq { addr: 0, len: 0, key: 0 });
    }
}
```

`cross_spawn` returns:

| Result | Meaning |
|---|---|
| `Ok(())` | input enqueued, target doorbell rung |
| `Err(None)` | input enqueued, but the doorbell could not be rung; re-notify the target |
| `Err(Some(input))` | nothing enqueued (FIFO full or wrong core); retry later or raise `capacity` |

The generated stub exists only once `system.json` does. `cargo xbin sync` only
collects metadata and does not type-check the application; `cargo xbin build`
generates the typed stub, so a misspelled task or a wrong input type is a hard
build error.

### `#[app]` arguments

Topology is declared only in `rticx.toml`: the pass injects the application's
single `core_id` (as the scalar `core_ids = [core_id]`) for the core pass, while
`cores` keeps its default of `1`. A cross-binary `#[app]` may **not** declare
`cores`.

| Key | Meaning |
|---|---|
| `dispatchers = [IRQ…]` | flat pool of interrupt lines: the cross-binary line dispatchers first (one entry per `(source, priority)` cross line of the application, ascending), then the local task dispatchers. Required when the application has cross-binary receivers or local `#[sw_task]`/`#[async_task]` priority groups |

### `spawn_by` resolution

`spawn_by` **always** names a global producer core id (as declared in
`rticx.toml`), so a `#[sw_task]` carrying one is a cross-binary receiver:

| `spawn_by` | Classification |
|---|---|
| absent | plain native `#[sw_task]`, left untouched |
| another application's `core_id` | cross-binary receiver |
| this application's own `core_id` | hard error: it is a core-local task, so drop `spawn_by` |
| not declared by any application | hard error naming the unknown core |

A `#[sw_task]` with no `spawn_by` is a plain local software task. A cross
receiver always runs on the application's single core (local index `0`), so the
pass injects `core = 0` on the generated `#[task]`.

### Cross-receiver arguments

| Key | Required | Meaning |
|---|---|---|
| `priority` | no (default `1`) | priority line on the receiver core; cross receivers require `>= 1` |
| `capacity` | no (default `1`, `>= 1`) | pending inputs the FIFO holds |
| `spawn_by` | yes | the single **global** producer core id |
| `init` | no (default `generated`) | must be `generated`; the build generates the receiver's init |
| `shared` | no | passed to the core pass, which computes the SRP ceilings |
| `ipc` | no | path of the spawn-input type, for a receiver whose `impl RticSwTask` is outside `#[app]`; must resolve inside `#[app]` (for example `ipc_types::PingMsg`). Absent by default — see below |

A cross receiver may **not** declare `core`: it always runs on the application's
single core, and the pass injects `core = 0` on the generated `#[task]`. Invalid
arguments (unknown or duplicate keys, non-integer values, `capacity = 0`,
`priority = 0` on a cross receiver, a `spawn_by` array or a `core` key, an `ipc`
value that is not a type path, `ipc` without `spawn_by`, a receiver with no
`impl RticSwTask` and no `ipc`, or a malformed `dispatchers` list) are reported
with a precise error.

Cross-binary code requires the distribution's `swtasks` feature; without it
`RticSwTask` is undefined and the receiver fails to compile.

### Implementing a receiver outside the `#[app]` module

By default the pass reads `type SpawnInput = …;` from the receiver's
`impl RticSwTask` block, which must then live inside `#[app]`. When the
implementation lives elsewhere — a sibling module, or a module shared by both
binaries — name the input type on the declaration with `ipc`:

```rust
use my_distro::app;

#[app(
    device = my_pac,
    dispatchers = [IPC_LINE_0]
)]
mod app {
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0, ipc = ipc_types::PingMsg)]
    pub struct PingTask;
}

mod external {
    use embedded_io::Write as _; // only when the console is a `Usart`
    use crate::app::{PingTask, RticMutex, RticSwTask, ipc_types};

    impl RticSwTask for PingTask {
        type SpawnInput = ipc_types::PingMsg;

        fn exec(&mut self, input: Self::SpawnInput) {
            self.shared().console.lock(|console| {
                let _ = writeln!(console, "ping seq={}", input.seq);
            });
        }
    }
}
```

`ipc` must resolve **inside `#[app]`**: write `ipc = ipc_types::PingMsg`, not
`crate::app::ipc_types::PingMsg`. The build emits a compile-time assertion that
`<PingTask as RticSwTask>::SpawnInput` is exactly the declared type, so a
mismatch fails the build instead of silently sending the wrong layout; the
`CrossCoreMessage` assertion of [section 5](#5-shared-types-ipc-typestoml)
applies as well. The external module needs `RticSwTask` in scope for the `impl`
and `RticMutex` (the trait providing `.lock`) to reach a `shared` resource
through the generated `self.shared()`. A `Usart` console implements
`embedded_io::Write` rather than `core::fmt::Write`, so `writeln!` needs
`embedded_io::Write as _` in scope.

## 7. Worked example

`experiments/multibinary/crates/mock/fixtures/e2e/` is a complete two-core
project that builds and links on the host (its mock distribution replaces the
real target distribution and PAC). Nothing needs to be written to build it.

```text
crates/mock/fixtures/e2e/
├── rticx.toml, ipc-types.toml
├── app-m7/  (binary `m7`, global core 0)   # producer, declares nothing
└── app-m4/  (binary `m4`, global core 1)   # receiver, declares EncryptTask
```

**1. Install the driver** (section 2) and enter the fixture:

```bash
cargo install --path experiments/multibinary/crates/rticx-xbin-driver
cd experiments/multibinary/crates/mock/fixtures/e2e
```

**2. Configuration.** `ipc-types.toml` declares the single `EncryptReq`
message, and `rticx.toml` maps the two binaries to global cores 0 and 1 and
declares no IPC memory (both as in sections 4 and 5). The mock distribution
provides the shared `mock-0-1` pool.

**3. The receiver** (`app-m4/src/main.rs`, abridged) is the `EncryptTask` from
section 6, plus the usual `#[idle]`/`#[init]`:

```rust
use xbin_mock_distro::app;

#[app(
    device = mock_pac,
    dispatchers = [XBIN_IPC_LINE_0]
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

    // `#[idle]` + `#[init]` as usual.
}
```

**4. The producer** (`app-m7/src/main.rs`, abridged) declares no task:

```rust
use xbin_mock_distro::app;

#[app(device = mock_pac)]
mod app {
    // `#[idle]` + `#[init]` as usual.

    // A local task or `#[init]` would call, once the view is synced:
    //     EncryptTask::cross_spawn(ipc_types::EncryptReq { addr: 0, len: 0, key: 0 });
}
```

Each application's `Cargo.toml` adds the PAC and the distribution and pins the
binary name (the fixture adds `xbin-mock-runtime` for the host backend). No
generated-crate dependency is needed:

```toml
[[bin]]
name = "m4"
path = "src/main.rs"

[dependencies]
my-pac = "0.1"       # the fixture uses mock-pac
my-distro = "0.1"    # the fixture uses xbin-mock-distro
```

**5. Sync and build** with one command:

```bash
cargo xbin build
```

`build` runs `sync` and then compiles every application, streaming each Cargo
step it drives. From the sealed view it generates, inside the `#[app]` modules:

- in `app-m7`, `pub struct EncryptTask;` with an associated
  `cross_spawn(input: ipc_types::EncryptReq) -> Result<(), Option<ipc_types::EncryptReq>>`,
  plus that task's FIFO view;
- in `app-m4`, the receiver's FIFO view, the `SpawnInput` assertion, the
  `EncryptTask` line dispatcher and the `0->1` doorbell router;
- in both, the injected `ipc_types` module and `CrossCoreMessage` marker, the
  shared-memory setup, and the freshness anchor that ties the binary to the view.

**6. Run it.** `cargo xbin build` compiles the host binaries. The end-to-end
behaviour (spawn → doorbell → line dispatcher → `exec`, plus backpressure) is
exercised by the in-tree harness, which expands both applications into one
process. From `experiments/multibinary/`:

```bash
cargo test -p rticx-xbin-pass --test e2e_runtime
```

### Multi-source variant

`crates/mock/fixtures/three-app/` is the same project with a second producer
(`app-m5`, global core 2) spawning `SensorTask` onto the same receiver core. The
receiver lists one cross line per `(source, priority)` in ascending order, and
the producers use disjoint priorities:

```rust
#[app(
    device = mock_pac,
    dispatchers = [XBIN_IPC_LINE_0, XBIN_IPC_LINE_1]  // (0, 3), then (2, 4)
)]
mod app {
    #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
    pub struct EncryptTask;

    #[sw_task(priority = 4, capacity = 1, spawn_by = 2)]
    pub struct SensorTask;
    // ...
}
```

## 8. Building and running

### Commands

```bash
cargo xbin sync                 # validate the project and write the system view
cargo xbin build                # sync, then build every application
cargo xbin build --verify-elf   # ... and verify each linked binary against the pools
cargo xbin verify               # verify a plain `cargo build` output (--release for release)
```

`sync` parses `rticx.toml` and `ipc-types.toml`, collects each application's
receiver declarations, then merges and validates the whole project: that every
receiver's `spawn_by` names a declared core of another application, type
existence, disjoint priority lines, and whether the per-task FIFOs fit their
pool budget. It writes `target/rticx-xbin/system.json`
and (re)generates `ipc_types.rs` when the IDL changed. Without `rticx.toml`,
`sync` and `build` are no-ops.

`build` always runs `sync` first. A plain `cargo build` after an explicit `sync`
is supported, but once the cross-binary interface or the IDL changes it fails
with a staleness error naming `cargo xbin sync`; a missing view (for example
after `cargo clean`) is reported the same way.

### Running on real hardware

`cargo xbin build` only builds. Running is plain Cargo, so the distribution's
runner configuration applies:

```toml
# .cargo/config.toml
[build]
target = "thumbv7em-none-eabihf"

[target.thumbv7em-none-eabihf]
# probe-rs, OpenOCD, QEMU or a Renode wrapper, depending on your setup:
runner = "probe-rs run --chip STM32H745ZITx"
rustflags = ["-C", "link-arg=-Tlink.x"]
```

```bash
cargo xbin sync                 # one system view for every binary
cargo run -p app-m7 --bin m7    # builds and runs/uploads through the runner
cargo run -p app-m4 --bin m4
```

Both binaries must be flashed (or loaded by the simulator), because each owns
its own core. Heterogeneous targets may need per-application settings (a
different triple, linker script or MPU configuration); Cargo supports a
`.cargo/config.toml` per package for that. The in-tree fixtures use the host mock
instead; the out-of-tree STM32H7 acceptance demo is the reference for a real
M7+M4 setup.

### Boot order

Boot sequencing is **distribution-owned**; the generated code only configures
shared memory and zeroes the FIFOs each core produces. In short:

1. the distribution releases the cores (for example the H7 CM7 initializes
   shared memory and then releases the CM4 from reset);
2. every core runs the distribution's shared-memory configuration before its
   user `init`, and every producing core zeroes its FIFOs before its
   `post_init` can spawn;
3. the distribution guarantees its peers are up before IPC or the router
   interrupts are relied on.

See [boot sequencing and reset](architecture.md#11-boot-sequencing-distribution-owned-and-reset).

### Cache and MPU checklist

The shared pools are special memory; getting this wrong is the most common
source of runtime-only failures. When bringing up a new target, check:

- **Attributes:** every core that touches a pool must see it as **effectively
  non-cacheable and shared** — an MPU/MMU region attribute, a cache-attribute
  register, an uncached alias, or a design-time non-cacheable region. A
  cacheable mapping without cache maintenance is unsupported in v1.
- **Linker/MPU consistency:** reserve each pool outside every binary's
  `.data`/`.bss`, with a base and size that satisfy the platform's alignment
  rules.
- **Aliases:** the two endpoints of a pool may see it at different addresses
  (aliases, e.g. the D2 window of SRAM3); each core must use its own view.
- **IRQs:** every `dispatchers` entry and every doorbell IRQ must be enabled and
  assigned a priority at least as urgent as the tasks it wakes.

The distribution owns the pools, so these are usually a distribution concern;
see [memory and layout](architecture.md#9-memory-and-layout) for the details.

### Verifying your setup

- `cargo xbin sync` validates the whole topology (cores, `spawn_by` edges,
  priorities, types, pool budget fit).
- `cargo xbin build --verify-elf` additionally checks every linked binary
  against the distribution's pools; `cargo xbin verify` does the same for a
  plain `cargo build` output.
- Compile-time checks catch type and topology drift: the layout constants and
  `LAYOUT_HASH` in the injected `ipc_types` module, and the freshness anchor in
  every application.
- The in-tree fixtures are the executable specification; from
  `experiments/multibinary/`:

  ```bash
  cargo test -p rticx-xbin-pass --test e2e_runtime    # two-app spawn/dispatch
  cargo test -p rticx-xbin-driver --test e2e          # `cargo xbin build` over fixtures/e2e
  cargo test -p rticx-xbin-driver --test three_app    # multi-source fixture
  cargo test -p rticx-xbin-driver --test verify       # linked-ELF verification
  ```

## 9. Common errors

Most errors name the fix. The frequent ones:

*Configuration and validation (`sync`):*

| Message (excerpt) | Fix |
|---|---|
| ``missing required top-level key `schema` `` / ``unsupported rticx.toml schema version N`` | set `schema = 1` |
| ``package `x` is declared by more than one `[[application]]` `` | give each binary a unique `package` |
| ``global core id N is declared by both …`` | core ids must be unique across applications |
| ``references unknown type `T` `` | declare `[message.T]` or `[enum.T]` in `ipc-types.toml` |
| ``cyclic message reference: A -> B -> A`` | break the cycle; messages are stored inline |
| ```x` references global core id N in `spawn_by` of receiver `T`, but no application declares it `` | fix the id or add the missing `[[application]]` entry |
| ``receiver `T` declares `spawn_by = N`, which is its own core id`` | drop `spawn_by`; the task is a core-local software task |
| ``receiver `T` is declared by both `x` and `y` `` | task names must be unique across the project |
| ``tasks `A` and `B` share priority P on core N`` | give tasks from different source cores disjoint priority lines |
| ``the FIFOs of the `A<->B` pool `P` need N bytes, but the distribution reserves M`` | you must lower that task's `capacity` or use lighter IPC types |

*Build-phase dispatcher errors:*

| Message (excerpt) | Fix |
|---|---|
| ``this application has the cross line `(source S, priority P)`, but `dispatchers` declares only M entries`` | put one interrupt per cross line at the front of `dispatchers`, in ascending `(source, priority)` order, before the local task dispatchers |
| ```dispatchers` lists `IRQ` more than once`` | two cross lines cannot share one interrupt |
| ``cross-binary receiver `T` … but the software task `L` … already owns that priority line`` | give the receiver a priority no core-local task of that core uses |

*Freshness / staleness:*

| Message (excerpt) | Fix |
|---|---|
| ``failed to read the system view `…/system.json` `` | run `cargo xbin sync` (or `cargo xbin build`); common after `cargo clean` |
| ``the system view `…/system.json` is stale `` | the view was edited by hand; run `cargo xbin sync` |
| ``application `x` interface changed since the last `cargo xbin sync` `` | run `cargo xbin sync` before a plain `cargo build` |
| ``the IDL `ipc-types.toml` changed since the last `cargo xbin sync` `` | run `cargo xbin sync` before a plain `cargo build` |

*Runtime symptoms (hardware only):*

| Symptom | Likely cause | Fix |
|---|---|---|
| inputs arrive corrupted, or a receiver sees a stale value | pool not cache-coherent, or a wrong base alias | map the pool effectively non-cacheable and shared; reconcile the endpoints' views |
| a hard fault / bus error on the first spawn | pool mapping is not atomic-capable, or one core has no access | give both cores a shared, non-cacheable, atomic-capable mapping |
| `Ok(())` but the receiver's `exec` never runs | a doorbell/dispatcher IRQ is not enabled or mapped | map every doorbell IRQ in the distribution, then re-run `cargo xbin sync` |
| `Err(None)` repeatedly | the input is enqueued but the doorbell notification fails | fix the doorbell IRQ/word mapping; the enqueued input drains on the next successful notification |

The full lists are in [failure modes](architecture.md#13-failure-modes).
