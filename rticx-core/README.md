# rticx-core

This crate:
- Provides the core procedural macro logic for the [RTICX](https://github.com/rticx-rs/rticx) real-time concurrency framework.
    - Syntax that captures the SRP (Stack Resource Policy) Tasks and Resources model. 
    - Performs Ceiling analysis, and code generation for hardware-bound tasks, shared resources, locks, `init`, `post_init`, and `idle`
    - Provides the foundation for single-binary multicore targets support
- Exposes a [`RticMacroBuilder`](https://github.com/rticx-rs/rticx/wiki) API that lets distribution crates chain compilation passes to the core pass.
- Exposes a [`InfoBus`](https://github.com/rticx-rs/rticx/wiki) API that lets distributions and compilation passes subscribe and publish information throughout the proc-macro expansion pipeline.


## Documentation
For more information about the RTICX syntax and instructions for creating distributions and compilation passes refer to the [`RTICX Wiki`](https://github.com/rticx-rs/rticx/wiki)

## Semantic Versioning
- Any backwards incompatible application syntax changes requires a bump to major version.
- Any refactoring and/or logical bug fixes requires a bump to the minor version.

## License

MIT
