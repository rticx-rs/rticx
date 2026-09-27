# RTICX multi-binary extension (experimental)

Standalone experimental workspace for the multi-binary / heterogeneous
multi-core extension of RTICX. See
[`multibinary-multicore-plan.md`](../../multibinary-multicore-plan.md) at the
repository root for the design and task checklist.

This workspace is deliberately **not** part of the root workspace: it
path-depends on the root crates and is meant to be extracted into its own
repository later.

## Crates

| Crate | Purpose |
|---|---|
| `rticx-xbin-proto` | IDL parser, canonical layout engine, merge + validation, JSON schemas, canonical FIFO image |
| `rticx-xbin-pass` | `RticPass` implementation (metadata mode + codegen mode) |
| `rticx-xbin-rt` | Atomic cross-core SPSC queue, marker trait, ready/epoch helpers |
| `rticx-xbin-driver` | `cargo-xbin` subcommand (`sync`, `build`) |
| `rticx-xbin-mock` | Mock distro/backend for host tests |

## Documentation

- [docs/](docs/README.md) — user guide outline, architecture outline and
  PlantUML sources (rendered manually).

## Fixtures

`fixtures/metadata/` is a standalone two-application project (its own
`[workspace]`, so it stays outside this workspace's lockfile and target dir).
`app-m7` declares the sender stub, `app-m4` the receiver, and
`metadata-macro` is a minimal `#[app]` stand-in that runs only the
cross-binary metadata pass. `cargo xbin sync` is exercised over it end to end
(metadata collection, merge/validation, `system.json` emission and
`ipc-types/` generation) by the driver's `tests/fixture.rs`; the generated
`ipc-types/` crate is checked in, like in a real project.

`fixtures/e2e/` is the M4 end-to-end project: the same two applications, but
with a mock **distribution** (`xbin-mock-distro` + `xbin-mock-runtime`) that
runs the full core pass on `MockCoreBackend` and the cross-binary pass against
an in-process `MockBackend`. `cargo xbin build` is exercised over it by the
driver's `tests/e2e.rs` (M4-T1): phase 1 syncs the project and phase 2
generates the sender `cross_spawn`, the receiver doorbell dispatcher and the
init hooks, then builds and links both host binaries. The pass crate's
`tests/e2e_runtime.rs` (M4-T2) expands both applications into one host binary
and **runs** the generated code over the mock runtime: spawn, doorbell, FIFO
backpressure and dispatcher execution; the driver's `tests/negative.rs`
(M4-T3) asserts the documented errors for a missing sync, a stale view or
source, priority conflicts, unknown types and region overflow.

## Build & test

```bash
cd experiments/multibinary
cargo build
cargo test

# try the driver skeleton (`cargo xbin`)
cargo install --path crates/rticx-xbin-driver
cargo xbin --help
```
