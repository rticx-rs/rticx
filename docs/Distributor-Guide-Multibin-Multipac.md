# Multibin and Multipac

This page covers the `multibin` and `multipac` features of `rtic-core`, which are used by distributions that produce multiple binaries from a single RTIC source or that need a different PAC path per core.

## `multibin`

The `multibin` feature is used when the target produces multiple binaries from one RTIC application source, typically one binary per core. The expanded output is guarded with `#[cfg(core = "N")]` so that each binary contains only the code for its core.

### Enabling the feature

Enable the feature on `rtic-core` in your distribution's macro crate:

```toml
[dependencies]
rtic-core = { path = "...", features = ["multibin"] }
```

### Building a multibin application

Compile each core separately using `RUSTFLAGS` to set the `core` cfg flag:

```bash
cd distributions/stm32-renode-rtic
RUSTFLAGS='--cfg core="0"' cargo build --example ping_pong
RUSTFLAGS='--cfg core="1"' cargo build --example ping_pong
```

### Shared memory macro

When `multibin` is enabled, `CorePassBackend::multibin_shared_macro_path()` must return the path to the shared-memory macro used to place data in shared RAM. For example:

```rust
fn multibin_shared_macro_path() -> syn::Path {
    syn::parse_quote!(rtic::export::microamp::shared)
}
```

This macro is typically provided by `microamp_experimental` or a similar shared-memory support crate.

## `multipac`

The `multipac` feature is used when each core has its own PAC path. Without this feature, the `device` argument in `#[app(...)]` takes a single path. With `multipac`, it takes a list of paths:

```rust
#[rtic::app(device = [pac0, pac1], cores = 2)]
mod my_app { ... }
```

Each core then uses the PAC path at the corresponding index.

### Enabling the feature

```toml
[dependencies]
rtic-core = { path = "...", features = ["multipac"] }
```

### Backend handling

The backend can use the core index to select the correct PAC path when generating code that references device peripherals or interrupts.

## Using both features together

Multi-binary multicore systems often need both `multibin` and `multipac`. Enable both features in the macro crate and ensure the backend provides the shared-memory macro path.

## Out-of-tree tooling

The `microamp_experimental` directory in this repository provides experimental μAMP tooling for assembling multi-binary images. New distributions can reuse this tooling or provide their own build scripts.

## Summary

| Feature | Use case |
|---------|----------|
| `multibin` | One RTIC source, multiple per-core binaries. |
| `multipac` | Each core uses a different PAC path. |

For a concrete `multibin` example, see `stm32-renode-rtic`.
