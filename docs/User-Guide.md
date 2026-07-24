# User Guide

This guide is for application developers who want to write real-time Rust applications using an existing RTIC distribution.

## What is RTIC?

RTIC is a Rust framework for building event-driven, real-time embedded applications. It uses a set of attributes (`#[rtic::app]`, `#[task]`, `#[shared]`, etc.) to generate code that implements the Stack Resource Policy (SRP) for safe resource sharing between tasks and interrupts.

## The modular rewrite

In this rewrite, the framework is split into three layers:

1. **`rtic-core`** — parses the RTIC syntax, runs resource-ceiling analysis, and generates code for tasks, resources, `init`, and `idle`.
2. **Compilation passes** — independent crates that extend RTIC syntax (e.g., software tasks, automatic core assignment, deadlines-to-priorities).
3. **Distributions** — target-specific crates that bind the core and selected passes to a particular microcontroller, exposing the final `#[rtic::app]` macro.

As an application author, you pick a distribution, write an RTIC application using the supported syntax, and let the macro do the rest.

## Core concepts

- **Tasks** — units of work that can be triggered by interrupts or by other tasks.
- **Shared resources** — data protected by the SRP ceiling protocol.
- **Priorities** — RTIC assigns ceilings based on task priorities so that higher-priority tasks can preempt safely.
- **Init** — the startup task that runs once and returns the initial shared resources.
- **Idle** — the loop that runs when no task is active.

## Next steps

- [Getting Started](User-Guide-Getting-Started) — build your first RTIC application.
- [Syntax Reference](User-Guide-Syntax) — learn the RTIC attributes supported by the core and software-task passes.
- [Supported Distributions](User-Guide-Supported-Distributions) — pick the right distribution for your target hardware.
