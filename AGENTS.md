# AGENTS.md — RTICX

This repository is the **core** of the RTICX modular rewrite: one cargo
workspace (root `Cargo.toml`) with the core crates, compilation passes,
tooling, and the in-tree reference distribution `rticx-cortex-m`.

The `rticx-riscv` and `rticx-rp2040` distributions live **out-of-tree** in
their own repositories:

| Distribution | Repository |
|---|---|
| `rticx-riscv` | https://github.com/rticx-rs/rticx-riscv |
| `rticx-rp2040` | https://github.com/rticx-rs/rticx-rp2040 |

## Architecture

Three layers:

- **`rticx-core`** — proc-macro core: parsing, SRP ceiling analysis, codegen, and a `RticMacroBuilder` API that chains compilation passes.
- **Compilation passes** — pure syntax-to-syntax transforms (e.g. software tasks, async tasks, deadline→priority, auto-assign). They implement `RticPass`.
- **Distributions** — target-specific crates providing a `#[<distro>::app]` macro. Each distribution implements backend traits, registers passes, and exposes target-specific runtimes. Only `rticx-cortex-m` lives in this repo.

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

### Tooling

| Tool | Path | What it does |
|------|------|---------------|
| `rticx-expand` | `tools/rticx-expand/` | `cargo rticx-expand` subcommand: prints the complete expanded source (user code around the `#[…::app]` module preserved) to stdout like `cargo expand` (`--merge` splices it into the source file, `restore` reverts), `--expand-passes <dir>` snapshots the module after every pass + the core pass for diffing |

### Distributions

| Distribution | Location | Target |
|--------------|----------|--------|
| `rticx-cortex-m` | `distributions/rticx-cortex-m/` (this repo) | Single-core Cortex-M (BASEPRI or source-masking) |
| `rticx-riscv` | `rticx-rs/rticx-riscv` (own repo) | Single-core RISC-V (SLIC, ESP32-C3, ESP32-C6) |
| `rticx-rp2040` | `rticx-rs/rticx-rp2040` (own repo) | Dual-core Cortex-M0+ (RP2040) |

## Feature Flags

| Crate | Feature | Effect |
|-------|---------|--------|
| `rticx-cortex-m` | `armv6m` | Source-masking lock (default: BASEPRI) |
| `rticx-riscv` (own repo) | `slic` / `esp32c3` / `esp32c6` | Mutually exclusive target selectors |
| `rticx-rp2040` (own repo) | `autoassign` | auto core assignment |
| <all distros> | `async` | Enables async/await software tasks |
| <all distros> | `swtasks` | Enables lightweight software tasks |

Feature propagation pattern: distro crate feature -> forwards to `*-macro` crate feature → macro crate enables `dep:<pass>` via `#[cfg(feature = "...")]`.

## Conventions

- **Attribute args** are parsed via `rticx_core::parse_utils::RticAttr` (typed accessors + supported-key checks). Unknown `#[app]` args produce warnings in the generated code; unknown task-level args are compile errors. Passes must strip the args they consume before returning from `run_pass` — see wiki "Writing Compilation Passes".
- **API compatibility + versioning**: `COMPATIBILITY.md` at the repo root is the compatibility contract (frozen backend/pass trait surface, additive-only rules, shared version generation, cross-repo coordination with the out-of-tree distributions). Read it before touching backend/pass APIs.
- **Distro-smoke**: the `distro-smoke` CI job (advisory, `continue-on-error`) compiles the out-of-tree distro repos against this checkout via `[patch.crates-io]`. Red is expected during a generation bump until the distro repos opt in.

## GOTCHAS

- **`build-std` is required** for `riscv32imc-unknown-none-elf` in the riscv repo's esp32c3 examples. It lives in that repo's `examples/esp32c3-examples/.cargo/config.toml`:
  ```toml
  [unstable]
  build-std = ["alloc", "core"]
  ```
- **`rticx-cortex-m` does not compile for the host target** (BASEPRI path is armv7-m only); it is exercised via its own `Makefile` with real targets (`make -C distributions/rticx-cortex-m ...`).

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

# rticx-expand tool
cd tools/rticx-expand && cargo test
cargo install --path tools/rticx-expand  # enables `cargo rticx-expand`

# Build the in-tree cortex-m distribution examples
cd distributions/rticx-cortex-m/examples-apps
cargo build --example hello_rtic
cargo build --example async_ping_pong --features async
cargo build --target thumbv6m-none-eabi --example hello_rtic

# Run cortex-m examples under QEMU
make qemu

# Out-of-tree distributions: build/test them in their own repositories
git clone https://github.com/rticx-rs/rticx-riscv
cd rticx-riscv && make all          # fmt-check, clippy (target matrix), QEMU examples

git clone https://github.com/rticx-rs/rticx-rp2040
cd rticx-rp2040 && make all         # fmt-check, clippy, example builds (thumbv6m)
```

## Available Skills

Load these with the `skill` tool for detailed reference on specific tasks:

| Skill | Purpose |
|-------|---------|
| `rticx-backend` | Full trait signatures, pipeline order, InfoBus API, syntax attributes |
| `rticx-create-pass` | How to write a new compilation pass (references wiki) |
| `rticx-create-distribution` | How to create a new distribution (references wiki) |
| `rticv2-to-rticx-migration` | Comprehensive guide and reference for porting RTIC v2 code to RTICX |


*Last oriented: 2026-08-18*
