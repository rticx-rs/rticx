# RTICX multi-binary multicore extension (experimental)

Standalone workspace for the multi-binary / heterogeneous multi-core extension of RTICX.

## Example Multi-binary multicore project
- TODO: See STM32H7 RTICX distribution examples

## Documentation

- [User guide](docs/user-guide.md) 
- [Architecture](docs/architecture.md)

## Crates

| Crate | Purpose |
|---|---|
| `rticx-xbin-rt` | Atomic cross-core SPSC FIFO and the ready-queue re-export |
| `rticx-xbin-proto` | IDL parser, canonical layout engine, `ipc_types` module generator, merge + validation, JSON schemas, canonical FIFO image |
| `rticx-xbin-pass` | `RticPass` capturing Multi-binary cross-core tasks syntax; injects the `CrossCoreMessage` trait and the `ipc_types` module |
| `rticx-xbin-driver` | `cargo-xbin` subcommand (`sync`, `build [--verify-elf]`, `verify`) |
| `rticx-xbin-mock` | Mock distro/backend for host tests |
| `xbin-mock-capability` | Fixture IPC capability table shared by the mock distributions |
| `xbin-mock-distro`, `xbin-mock-runtime` | Mock `#[app]` distribution and runtime used by the fixtures |

## Build & test

```bash
cd experiments/multibinary
cargo build
cargo test

# try the driver skeleton (`cargo xbin`)
cargo install --path crates/rticx-xbin-driver
cargo xbin --help
```
