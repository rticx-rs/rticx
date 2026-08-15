# AGENTS.md — RTIC Modular Rewrite

This repository is a modular rewrite of the RTIC framework. There is **no root `Cargo.toml`** — every crate is independent with relative path deps.

## Architecture

Three layers:

- **`rticx-core`** — proc-macro core: parsing, SRP ceiling analysis, codegen, and a `RticMacroBuilder` API that chains compilation passes.
- **Compilation passes** — pure syntax-to-syntax transforms (e.g. software tasks, async tasks, deadline→priority, auto-assign). They implement `RticPass`.
- **Distributions** — target-specific crates providing a `#[<distro>::app]` macro. Each distribution implements backend traits, registers passes, and exposes target-specific runtimes.

## Crate Map

### Core

| Crate | Path |
|-------|------|
| `rticx-core` | `rticx-core/` |
| `rticx-spsc` | `rticx-spsc/` |
| `rticx-async` | `rticx-async/` |

### Passes

| Crate | Path | What it does |
|-------|------|---------------|
| `rticx-sw-pass` | `compilation-passes/rticx-sw-pass/` | Software tasks (`spawn`/`spawn_from`, dispatchers) |
| `rticx-auto-assign` | `compilation-passes/rticx-auto-assign/` | Auto-assigns `core = N` from shared resource usage |
| `rticx-deadline-pass` | `compilation-passes/rticx-deadline-pass/` | Converts `deadline = D` to priorities |
| `rticx-async-pass` | `compilation-passes/rticx-async-pass/` | Async/await software tasks (executors, channels, wakers) |

### Distributions

| Distribution | Path | Target |
|--------------|------|--------|
| `rticx-cortex-m` | `distributions/rticx-cortex-m/` | Single-core Cortex-M (BASEPRI or source-masking) |
| `rticx-riscv` | `distributions/rticx-riscv/` | Single-core RISC-V (SLIC, ESP32-C3, ESP32-C6) |
| `rticx-rp2040` | `distributions/rticx-rp2040/` | Dual-core Cortex-M0+ (RP2040) |


## Feature Flags

| Crate | Feature | Effect |
|-------|---------|--------|
| `rticx-core` | `debug_expand` | Writes expanded code to disk |
| `rticx-cortex-m` | `armv6m` | Source-masking lock (default: BASEPRI) |
| `rticx-cortex-m` | `async` | Enables async/await software tasks |
| `rticx-riscv` | `slic` / `esp32c3` / `esp32c6` | Mutually exclusive target selectors |
| `rticx-riscv` | `async` | Enables async/await software tasks |
| `rticx-rp2040` | `swtasks` / `autoassign` | Software tasks / auto core assignment |

Feature propagation pattern: distro crate feature → forwards to `*-macro` crate feature → macro crate enables `dep:<pass>` via `#[cfg(feature = "...")]`.

## Conventions

- **Attribute args** are parsed via `rticx_core::parse_utils::RticAttr` (typed accessors + supported-key checks). Unknown `#[app]` args produce warnings in the generated code; unknown task-level args are compile errors. Passes must strip the args they consume before returning from `run_pass` — see wiki "Writing Compilation Passes".

## GOTCHAS

- **`build-std` is required** for `riscv32imc-unknown-none-elf`. Add to `.cargo/config.toml`:
  ```toml
  [unstable]
  build-std = ["alloc", "core"]
  ```
- **No root Cargo.toml** — each crate must be built/tested from its own directory.

## Build & Test

```bash
# Validate everything. Use this before finalizing any significant code change.
make fmt all

# Test individual crates
cd rticx-core && cargo test
cd rticx-spsc && cargo test
cd rticx-async && cargo test
cd compilation-passes/rticx-sw-pass && cargo test
cd compilation-passes/rticx-async-pass && cargo test
cd compilation-passes/rticx-auto-assign && cargo test -- --test-threads=1
cd compilation-passes/rticx-deadline-pass && cargo test

# Build distribution examples
cd distributions/rticx-cortex-m/example-apps/armv7m-app
cargo build --example hello_rtic
cargo build --example async_ping_pong --features async

cd distributions/rticx-riscv/examples/esp32c3-examples
cargo build --example hello_rtic
cargo build --example async_prio0 --features async

# Run cortex-m & riscv slic examples under QEMU
make qemu
```

## Available Skills

Load these with the `skill` tool for detailed reference on specific tasks:

| Skill | Purpose |
|-------|---------|
| `rticx-backend` | Full trait signatures, pipeline order, InfoBus API, syntax attributes |
| `rticx-create-pass` | How to write a new compilation pass (references wiki) |
| `rticx-create-distribution` | How to create a new distribution (references wiki) |


*Last oriented: 2026-08-12*
