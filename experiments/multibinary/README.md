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

## Build & test

```bash
cd experiments/multibinary
cargo build
cargo test

# try the driver skeleton (`cargo xbin`)
cargo install --path crates/rticx-xbin-driver
cargo xbin --help
```
