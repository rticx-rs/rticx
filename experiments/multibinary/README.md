# RTICX multi-binary extension (experimental)

Standalone experimental workspace for the multi-binary / heterogeneous
multi-core extension of RTICX. See
[`multibinary-multicore-plan.md`](multibinary-multicore-plan.md) at the
repository root for the design and task checklist.

This workspace is deliberately **not** part of the root workspace: it
path-depends on the root crates and is meant to be extracted into its own
repository later.

## Crates

| Crate | Purpose |
|---|---|
| `rticx-xbin-proto` | IDL parser, canonical layout engine, merge + validation, JSON schemas, canonical FIFO image |
| `rticx-xbin-pass` | `RticPass` implementation (metadata mode + codegen mode) |
| `rticx-xbin-rt` | Atomic cross-core SPSC queue, marker trait, ready/epoch helpers (`SharedState`, spawn-side `ReadyCache`) |
| `rticx-xbin-driver` | `cargo-xbin` subcommand (`sync`, `build [--verify-elf]`, `verify`) |
| `rticx-xbin-mock` | Mock distro/backend for host tests |
| `xbin-mock-capability` | Fixture IPC capability table shared by the mock distributions |
| `xbin-mock-distro`, `xbin-mock-runtime` | Mock `#[app]` distribution and runtime used by the fixtures |

## Documentation

- [user guide](docs/user-guide.md) — install, topology, task syntax, worked
  example, build/run, distribution-author guidance, troubleshooting.
- [architecture](docs/architecture.md) — phases, memory/pool rules,
  boot/reset, failure modes.
- [design plan](multibinary-multicore-plan.md) — the normative reference and
  the milestone checklist.

## Fixtures

`fixtures/metadata/` is a standalone two-application project (its own
`[workspace]`, so it stays outside this workspace's lockfile and target dir).
`app-m7` declares the sender stub, `app-m4` the receiver, and
`metadata-macro` is a minimal `#[app]` stand-in that runs only the
cross-binary metadata pass and binds the mock capability table (M6.9-T9).
`cargo xbin sync` is exercised over it end to end
(metadata collection, merge/validation, `system.json` with `pools[]` and
physical core ids, and `ipc-types/` generation) by the driver's
`tests/fixture.rs`; the generated `ipc-types/` crate is checked in, like in a
real project.

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
source, priority conflicts, unknown types and pool-budget overflow.

`fixtures/three-app/` is the M6-T1 project, in a fully connected cross-binary
topology: `app-m7` (global core 0), `app-m5` (global core 2) and `app-m4`
(global core 1) each own one binary, every unordered pair has its own distro
pool (`0<->1`, `1<->2`, `0<->2`), and every application is both a producer and a
receiver (eight cross-binary tasks over the three pools, with `0->1` and `2->1`
packing two FIFOs each). The
driver's `tests/three_app.rs` builds it with `cargo xbin build`, and the pass
crate's `tests/e2e_runtime.rs` expands all three applications into one host
binary and runs the multi-source scenario: per-pair routers and dispatcher
lines, per-source backpressure, and the non-owner producer initializing its
own pool (`tests/priority_lines.rs` covers the build-phase priority-line
validation). The two-application harness also drives the M6-T2 ready/epoch
path: a simulated receiver reset makes `cross_spawn` return `Err(Some(input))`
without enqueueing, and the spawn after the receiver re-marks itself ready
refreshes the stale epoch and executes.

## Renode acceptance harness (M7)

[`renode/`](renode/README.md) carries the dual-core STM32H7 (Cortex-M7 +
Cortex-M4) Renode platform used as the M7 acceptance target, plus a
`run.sh <cm7.elf> <cm4.elf> [seconds]` launcher that boots both images
headless and routes the two UARTs to the log. It is the harness the future
out-of-tree `rticx-stm32h7` distribution's cross-binary spawn demo runs on.
The `.repl` is self-contained (RCC/PWR/HSEM/EXTI models embedded); see its
README for provenance, the shared-memory map and the HSEM/EXTI doorbell
paths.

## Build & test

```bash
cd experiments/multibinary
cargo build
cargo test

# try the driver skeleton (`cargo xbin`)
cargo install --path crates/rticx-xbin-driver
cargo xbin --help
```
