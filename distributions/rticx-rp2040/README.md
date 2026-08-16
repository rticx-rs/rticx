# rticx-rp2040

[RTICX](https://github.com/rticx-rs/rticx) multicore distribution for the Raspberry Pi Pico / RP2040 dual-core Cortex-M0+.

## Notes
- This crate uses `rp2040-hal` crate pinned at version "0.10.2". 
- Enabling `async` features will enable the `critical-section-impl` feature of `rp2040-hal`, and you must remove/disable any other critical section backend


## Features

- `async` -- enable async/await software tasks
- `swtasks` -- enable core-local and cross-core software tasks
- `autoassign` -- enable automatic core assignment to tasks (inferred from shared resources usage)

## Documentation

Full user guide is available in the [project wiki](https://github.com/rticx-rs/rticx/wiki/User-Guide) including multicore syntax.

## Core 1 stack size

By default, core 1 gets a stack of **4096** 32-bit words (16 KiB). You can change
this with the `core1_stack = N` `#[app]` argument, where `N` is the stack size in
**32-bit words** (not bytes). The argument is only accepted when `cores = 2`;
if omitted, the default of `4096` is used.

``` rust
// Give core 1 a 32 KiB stack (8192 * 4 bytes)
#[rticx_rp2040::app(device = rp2040_hal::pac, cores = 2, core1_stack = 8192)]
mod my_app {
    // ...
}
```

## Examples
You can find examples in `example-apps`. 

To build the examples:
``` bash
cd examples-apps
make all 
# or individually
make build-hello
```

To run the examples on a RP pico, you need to have `probe-rs` installed (cargo install prob-rs-tools) use

```bash
make run-<example-name>
#e.g
make run-hello
```

## License

MIT
