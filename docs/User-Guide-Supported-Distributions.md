# Supported Distributions

This page lists the reference distributions maintained in this repository. New distributions for other hardware targets are developed as out-of-tree crates.

| Distribution | Path | Target | Features | Notes |
|--------------|------|--------|----------|-------|
| `rp2040-rtic` | `distributions/rp2040-rtic/` | Raspberry Pi Pico / RP2040 dual-core Cortex-M0+ | `autoassign`, `swtasks` | Single binary; starts core 1 from `post_init`. |
| `stm32-renode-rtic` | `distributions/stm32-renode-rtic/` | Renode-simulated multicore STM32F1C3-like | `multibin` | Multi-binary build: compile each core separately with `RUSTFLAGS='--cfg core="N"'`. |
| `rtic-hippo` | `distributions/rtic-hippo/` | Single-core RISC-V Hippomenes MCU | `deadline-pass` | Uses threshold-based (`mintthresh`) locking. |
| `atalanta-rtic` | `distributions/atalanta-rtic/` | Single-core RISC-V Atalanta MCU | `deadline-pass` | |
| `distribution-template` | `distributions/distribution-template/` | Reference / template | — | Conceptual starting point for new distributions; not expected to compile. |

## Feature descriptions

- `swtasks` — enables the `rtic-sw-pass` software tasks pass (dispatchers, message queues, `spawn`, `spawn_from`).
- `autoassign` — enables the `rtic-auto-assign` pass for automatic `core = N` assignment.
- `deadline-pass` — enables the `rtic-deadline-pass` deadlines-to-priorities conversion.
- `multibin` — enables multi-binary output via `#[cfg(core = "N")]` guards.

## Feature flags per distribution

### `rp2040-rtic`

```toml
[features]
autoassign = ["rtic-macro/autoassign"]
swtasks = ["rtic-macro/swtasks"]
```

These features are enabled by default for the example dev-dependencies.

### `stm32-renode-rtic`

This distribution is always built with `multibin` support. Each core is compiled separately:

```bash
RUSTFLAGS='--cfg core="0"' cargo build --example ping_pong
RUSTFLAGS='--cfg core="1"' cargo build --example ping_pong
```

### `rtic-hippo` and `atalanta-rtic`

These single-core RISC-V distributions enable the `deadline-pass` feature to support `deadline = D` attributes in tasks.

## Creating a new distribution

If you want to support new hardware, you create a new distribution crate outside of this repository. The [Distributor Guide](Distributor-Guide) explains how to implement the required backend traits and register compilation passes.
