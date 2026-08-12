# Create a Distribution

Prerequisite: run `git submodule update --init` from the repo root to fetch
the wiki.

## Primary reference

Read `wiki/Distributor-Guide-Writing-Distributions.md` for the full
procedure: crate layout, CorePassBackend implementation, RticMacroBuilder
assembly, and feature gating.

For trait signatures and pipeline order, load the `rticx-backend` skill.
For architecture context and design decision, see `wiki/Distributor-Guide-Architecture.md`.

## Quick start

- Copy `distributions/distribution-template/` as your starting point.
- Two crates are needed: `<distro>/` (library, exports RTICX macro, utilities and any other necessary re-exports) and `<distro>/rticx-macro/` (proc-macro).
- Implement the hardware specific bindings for the core pass `rticx_core::CorePassBackend` for the target hardware architecture
- OPTIONAL: Implement the hardware specific bindings for software pass and async pass to enable support for software task and async/await syntax.
  - bind the software and async passes using `.bind_pre_core_pass()` function the `rticx_core::RticMacroBuilder`
- OPTIONAL: Bind other passes using `rticx_core::RticMacroBuilder` and implement any other necessary bindings.
- Export the proc-macro in the library crate
- Re-export any necessary dependencies required by the core-pass or any other compilation passes.
- For ARM Cortex-m targets refer to `rticx-cortex-m` as an example
- For RISCV refer to `rticx-riscv` as an example
- For single binary multicore targets refer to `rticx-rp2040` as an example
- Each compilation pass should be features gates so that users can choose to opt-in/out of those syntax extensions and speedup the build system.