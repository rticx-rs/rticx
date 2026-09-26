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
| `rticx-xbin-proto` | IDL parser, canonical layout engine, JSON schema + validation |
| `rticx-xbin-pass` | `RticPass` implementation (metadata mode + codegen mode) |
| `rticx-xbin-rt` | Atomic cross-core SPSC queue, marker trait, ready/epoch helpers |
| `rticx-xbin-driver` | `cargo-xbin` subcommand (`sync`, `build`) |
| `rticx-xbin-mock` | Mock distro/backend for host tests |

## Documentation

- [docs/](docs/README.md) — user guide outline, architecture outline and
  PlantUML sources (rendered manually).

## Build & test

```bash
cd experiments/multibinary
cargo build
cargo test

# try the driver skeleton (`cargo xbin`)
cargo install --path crates/rticx-xbin-driver
cargo xbin --help
```
