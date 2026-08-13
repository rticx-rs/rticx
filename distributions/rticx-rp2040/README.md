# rticx-rp2040

[RTICX](https://github.com/rticx-rs/rticx) multicore distribution for the Raspberry Pi Pico / RP2040 dual-core Cortex-M0+.

## Notes
- This crate uses `rp2040-hal` crate pinned at version "0.10.2". 
- Enabling `async` features will enabled the `critical-section-impl` feature of `rp2040-hal`, and you must remove/disable any other critical section backend


## Features

- `async` -- enable async/await software tasks
- `swtasks` -- enable core-local and cross-core software tasks
- `autoassign` -- enable automatic core assignment to tasks (inferred from shared resources usage)

## Documentation

Full user guide is available in the [project wiki](https://github.com/rticx-rs/rticx/wiki/User-Guide).

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
