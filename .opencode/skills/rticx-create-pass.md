# Create a Compilation Pass

Prerequisite: run `git submodule update --init` from the repo root to fetch
the wiki.

## Primary reference

Read `wiki/Distributor-Guide-Writing-Compilation-Passes.md` for the full
procedure: trait implementation, `RticAttr` attribute parsing, InfoBus usage,
testing, and pass-specific backend traits.

For project architecture and design decisions context, see
`wiki/Distributor-Guide-Architecture.md`.

## Quick start: How to add a new compilation pass 

1. Create a new rust crate and pull rticx-core as a dependency.
2. Implement `rticx_core::RticPass` trait.
3. RTICX App attributes can be parsed with `rticx_core::parse_utils::RticAttr`. Strip every argument your pass consumes from the `args` token stream before returning from `run_pass` (`RticAttr::args_tokens`), and strip your pass-only task-attribute keys before re-emitting task attributes to the core pass.
4. Keep the compilation pass hardware agnostic by abstracting the hardware specific details behind a backend trait which will be implemented by a distribution (See `rticx_sw_pass::SwPassBackend` for an example)
5. Always create a README.md at the root of the crate containing:
  - What the compilation pass is about
  - Specify if this pass is for single core or both singe and multicore targets 
  - Example of high-level/user-application syntax
  - Example of expanded lower level syntax. If too complicated, write a detailed description where step by step you explain the syntax expansion gradually and preferably with examples that are easy to grasp.
  - If the pass provides a backend trait for hardware bindings, specify any necessary information that a distribution develop needs to know.
  - List any necessary dependencies, types, functions and constants that a distribution must re-export.
  - Specify what information this pass publishes to the `rticx_core::InfoBus` and what other information it expects if any.
  - List any required compilation passes that need to be bound between this pass and core-pass provided by rticx-core.
  - What major version of rticx-core this pass has been tested with (Syntax compatibility with core-pass).

