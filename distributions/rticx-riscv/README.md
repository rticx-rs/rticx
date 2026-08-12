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
| `async`  | no     | Enable Async/Await software-tasks |
| `swtasks`  | yes     | Enable the non-async software-tasks compilation pass (`rticx-sw-pass`). Disable with `default-features = false` for a hardware-task-only distribution. |
| `slic` + `mecall-backend` or `clint-backend`    | no      | Generic RISC-V target using the SLIC interrupt controller abstraction. |
| `esp32c3`  | no      | Espressif ESP32-C3. Uses `FROM_CPU_INTR{0..3}` as dispatcher software interrupts. |
| `esp32c6`  | no      | Espressif ESP32-C6 (machine-mode). Uses `FROM_CPU_INTR{0..3}` as dispatcher software interrupts. |


## Running examples in QEMU
```bash
make examples

# or individually
make examples-slic
make examples-esp
```

## License

MIT
