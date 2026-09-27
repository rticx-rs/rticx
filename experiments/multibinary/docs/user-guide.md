# User guide (outline)

Status: **outline**. Sections marked *implemented* describe behaviour that
exists in M0; the rest is the target workflow, completed by
[M6-T3](../../../multibinary-multicore-plan.md#m6--multi-sourcetarget-readyepoch-complete-docs)
once M1–M4 land.

1. [Installing `cargo-xbin`](#1-installing-cargo-xbin)
2. [Project layout](#2-project-layout)
3. [Writing `rticx.toml`](#3-writing-rticxtoml) — *implemented*
4. [Writing `ipc-types.toml`](#4-writing-ipc-typestoml) — *implemented*
5. [Declaring cross-binary tasks](#5-declaring-cross-binary-tasks) — *planned (M1)*
6. [Building and running](#6-building-and-running) — *skeleton implemented*
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

*Syntax confirmed and implemented (M1-T3).* The extension pass parses both
attributes and strips them (together with the `#[app]` extensions) before the
core pass runs, so other passes and distributions never see them.

Receiver binary (the task runs here):

```rust
#[cross_bin_task(priority = 3, capacity = 2, spawned_by = [0])]
struct EncryptTask;

impl CrossBinTask for EncryptTask {
    type Input = ipc_types::EncryptReq;
    fn exec(&mut self, input: Self::Input) {}
}
```

Sender binary (lightweight stub; the name must match the receiver):

```rust
#[cross_bin_spawn(core = 1, priority = 3, capacity = 2)]
struct EncryptTask;                 // optionally mirror the input type:
                                    //
                                    // impl CrossBinSpawn for EncryptTask {
                                    //     type Input = ipc_types::EncryptReq;
                                    // }
```

`#[app(...)]` gains `core_ids = [g...]` (local → global mapping) and
`external_cores = [g...]`; both are stripped by the extension pass. The
confirmed argument rules:

| Attribute | Key | Required | Meaning |
|---|---|---|---|
| `cross_bin_task` | `priority` | no (default `1`) | priority line on the receiver core |
| | `capacity` | no (default `1`, `>= 1`) | pending inputs the FIFO holds |
| | `core` | no (default `0`) | **local** core index running the task (`< cores`) |
| | `spawned_by` | no | **global** ids of the cores allowed to spawn |
| `cross_bin_spawn` | `core` | yes | **global** id of the target core that runs the task |
| | `priority` | no (default `1`) | must match the receiver |
| | `capacity` | no (default `1`, `>= 1`) | must match the receiver |

A distribution that does **not** bind the extension pass never errors on these
arguments: `core_ids` and `external_cores` show up as unknown `#[app]`
arguments and only warn.

Rejected with a precise error: unknown, duplicate or multi-segment keys;
non-integer `priority`/`capacity`/`core`; `capacity = 0`; non-array
`spawned_by`; a receiver without `impl CrossBinTask { type Input = … }`; a
sender without `core`; a receiver `core` outside `0..cores`; `external_cores`
overlapping the application's own `core_ids` or repeating an id; and either
attribute on a non-struct item or both on one item.

*TODO(M6-T3):* worked example, capacity/backpressure semantics, visibility
rules, what happens on a rejected spawn (`Err(Some(input))`).

## 6. Building and running

*Implemented through M4-T1:*

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

The collected manifests are then merged and validated (M1-T5): sender and
receiver declarations must agree on name, target core, priority, capacity and
input type; every `[[application]]`'s `core_ids`/`external_cores` must be
consistent with `rticx.toml`; every referenced type must exist in the IDL; and
the per-task FIFOs must fit their `(source → target)` region. The same
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
`sync`. Without a synced view (for example after `cargo clean`), an
application with cross-binary declarations fails with
``failed to read the system view …; run `cargo xbin sync` `` instead of
silently generating no code.

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
| ``sender `T` … has no matching `#[cross_bin_task]` receiver`` | declare the receiver in the target binary |
| ``receiver `T` … is never spawned`` | add a `#[cross_bin_spawn]` stub or remove the receiver |
| ``sender `T` … declares `priority`/`capacity` …, but the receiver declares …`` | mirror the receiver's values in every sender stub |
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
