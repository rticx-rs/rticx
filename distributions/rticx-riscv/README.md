# rticx-riscv

RTICX distribution for single-core RISC-V targets. Supports three mutually
exclusive backends, each providing the low-level interrupt controller bindings,
resource locking, and software-task infrastructure.

## Supported targets

| Feature   | Target                              | Interrupt controller                        | Locking mechanism                          |
|-----------|-------------------------------------|---------------------------------------------|--------------------------------------------|
| `slic`    | Generic RISC-V (CLINT/PLIC/ECLIC)  | [riscv-slic](https://crates.io/crates/riscv-slic) software-level controller | Threshold via SLIC `lock()` / `run()`      |
| `esp32c3` | Espressif ESP32-C3 (RV32IMC)       | `INTERRUPT_CORE0::cpu_int_thresh`           | Threshold register (+ CS at ceiling = 15)  |
| `esp32c6` | Espressif ESP32-C6 (RV32IMAC)      | `PLIC_MX::mxint_thresh`                    | Threshold register (+ CS at ceiling = 15)  |

**Exactly one** of these features must be selected at build time. Selecting none
or more than one produces a compile error.

## Features

| Feature    | Default | Description                                             |
|------------|---------|---------------------------------------------------------|
| `swtasks`  | yes     | Enable the software-tasks compilation pass (`rticx-sw-pass`). Provides `spawn()` / `spawn_dispatch()` APIs. Disable with `default-features = false` for a hardware-task-only distribution. |
| `slic`     | no      | Generic RISC-V target using the SLIC interrupt controller abstraction. Requires the user to call `riscv_slic::codegen!()` in their crate to generate the interrupt vector. |
| `esp32c3`  | no      | Espressif ESP32-C3. Uses `FROM_CPU_INTR{0..3}` as dispatcher software interrupts. |
| `esp32c6`  | no      | Espressif ESP32-C6 (machine-mode). Uses `FROM_CPU_INTR{0..3}` as dispatcher software interrupts. |

## Compilation passes/ Syntax features

The distribution binds the following compilation passes into the macro pipeline:

| Pass | Crate | When | Purpose |
|------|-------|------|---------|
| **Core pass** | `rticx-core` | always | Parses the `#[app]` module, computes SRP ceilings, generates resource proxies, init/idle wrappers, and task interrupt handlers. |
| **Software-tasks pass** | `rticx-sw-pass` | `swtasks` enabled | Transforms `#[sw_task]` items into `#[task]` items bound to dispatcher interrupts. Generates `spawn()` APIs, ready queues, and dispatcher handler bodies. |


## Usage

### Generic SLIC target

```toml
[dependencies]
rticx-riscv = { version = "0.1", default-features = false, features = ["slic", "swtasks"] }
riscv-slic = "0.2"
```

```rust
#[rticx_riscv::app(device = my_pac)]
mod app {
    #[sw_task(priority = 1)]
    #[shared]
    struct MyShared { /* ... */ }

    #[init]
    fn init(_cx: init::Context) -> Shared { /* ... */ }

    #[sw_task]
    fn worker(_cx: worker::Context) { /* ... */ }
}

// Must be called in the user crate to generate the SLIC interrupt vector:
riscv_slic::codegen!(
    slic = riscv_slic,
    pac = my_pac,
    swi = [SoftwareInterrupt0]
);
```

### ESP32-C3

```toml
[dependencies]
rticx-riscv = { version = "0.1", default-features = false, features = ["esp32c3", "swtasks"] }
```

```rust
#[rticx_riscv::app(device = esp32c3)]
mod app {
    #[sw_task(priority = 2)]
    #[shared]
    struct Shared { counter: u32 }

    #[init]
    fn init(_cx: init::Context) -> Shared { Shared { counter: 0 } }

    #[sw_task]
    fn worker(_cx: worker::Context) { /* ... */ }
}
```

### ESP32-C6

```toml
[dependencies]
rticx-riscv = { version = "0.1", default-features = false, features = ["esp32c6", "swtasks"] }
```

Same structure as ESP32-C3; swap the feature and `device` argument.

## License

MIT
