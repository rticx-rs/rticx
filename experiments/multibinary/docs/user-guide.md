# User guide (outline)

Status: **outline**. Sections marked *implemented* describe behaviour that
exists in M0; the rest is the target workflow, completed by
[M5-T3](../../../multibinary-multicore-plan.md#m5--multi-sourcetarget-readyepoch-complete-docs)
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
├── ipc-types/            # generated crate (planned M1, kept in VCS)
├── .cargo/config.toml    # target triple, linker, runner (as usual)
└── app-m7/, app-m4/      # one Cargo package per binary
```

`target/rticx-xbin/` is driver output; it is regenerated, not edited:

```text
target/rticx-xbin/
├── <app>.xbin.json       # per-application metadata (planned M1)
└── system.json           # merged, validated, allocated system view (planned M1)
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

*Implemented in M0:* `rticx_xbin_proto::{parse_idl_str, parse_idl_file}` plus
the canonical [layout engine](architecture.md#6-memory-and-layout) checks the
32-bit-safe subset. The `ipc-types/` crate generator exists in the proto crate;
`cargo xbin sync` will invoke it from M1 on.

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

*Planned (M1).* The syntax below is the proposal in
[plan §6.4–6.5](../../../multibinary-multicore-plan.md#64-app-additions); it is
confirmed when the metadata pass lands.

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
struct EncryptTask;
```

`#[app(...)]` gains `core_ids = [g...]` (local → global mapping) and
`external_cores = [g...]`; both are stripped by the extension pass. `core`
and `spawned_by` always use **global** core ids.

*TODO(M5-T3):* worked example, capacity/backpressure semantics, visibility
rules, what happens on a rejected spawn (`Err(Some(input))`).

## 6. Building and running

*Skeleton implemented in M0:*

```bash
cargo xbin --help     # CLI overview
cargo xbin sync       # validate rticx.toml, create target/rticx-xbin/
cargo xbin build      # sync, then build every application
```

In M0 `sync` and `build` succeed as no-ops for a project without
`rticx.toml`; once M1 lands they collect metadata, merge/validate the system
view, allocate FIFO addresses, write `target/rticx-xbin/system.json` and
regenerate `ipc-types/`. `build` always runs `sync` first; a plain
`cargo build` after an explicit `sync` is supported, but fails with a
topology-hash mismatch once sources changed until the next `sync`.

*TODO(M5-T3):* flashing/running per target, QEMU/Renode runners, cache/MPU
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

*TODO(M1–M5):* merge/validation errors (sender/receiver mismatch, priority
conflicts, region overflow, type not cross-core safe, not-ready target,
doorbell failure) with causes and fixes.
