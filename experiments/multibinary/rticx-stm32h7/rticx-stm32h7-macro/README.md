# rticx-stm32h7-macro

Proc-macro crate of the [RTICX STM32H7 (Cortex-M7 + Cortex-M4)](https://github.com/rticx-rs/rticx)
distribution. It implements the `#[app]` attribute macro by assembling the core
pass, the cross-binary pass (`rticx-xbin-pass`) and the software-tasks pass.

This is an internal dependency of `rticx-stm32h7`; depend on that crate
instead.

The macro is target-gated: exactly one of the `cm7` / `cm4` features selects the
physical core the binary runs on.

## Generated code

`#[app]` also injects the shared IDL types. `cargo xbin sync` writes
`target/rticx-xbin/ipc_types.rs`, and the macro re-emits its tokens into the
application module as `pub mod ipc_types`, next to an
`unsafe trait CrossCoreMessage: Copy + 'static` that every generated type
implements — the same injection pattern as `RticSwTask` and `RticIdleTask`.
Application code therefore writes `ipc_types::EncryptReq` inside `#[app]` with
no crate dependency; an external module reaches the types through
`crate::app::ipc_types`.

## License

MIT
