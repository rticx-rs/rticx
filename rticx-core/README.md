# rticx-core

Core procedural macro logic for the [RTICX](https://github.com/rticx-rs/rticx) real-time concurrency framework.

Provides parsing, Stack Resource Policy (SRP) ceiling analysis, and code generation for tasks, resources, `init`, and `idle`. Exposes a [`RticMacroBuilder`](https://github.com/rticx-rs/rticx/wiki) API that lets distribution crates chain compilation passes before or after the core pass.

## License

MIT
