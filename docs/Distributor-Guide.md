# Distributor Guide

This guide is for developers who want to port RTIC to new hardware, create a new distribution, or write a new compilation pass.

It assumes you are familiar with the user-facing syntax described in the [User Guide](User-Guide) and [Syntax Reference](User-Guide-Syntax).

## What is a distributor?

A distributor crate:

1. Implements the low-level, target-specific backend traits defined by `rtic-core` (and optionally by passes such as `rtic-sw-pass`).
2. Registers the compilation passes it wants to use via `RticMacroBuilder`.
3. Exposes the final `#[rtic::app]` proc macro.

The result is a self-contained RTIC distribution that application developers can depend on.

## Important scope note

This repository maintains the core framework and a set of reference distributions. New distributions for additional hardware targets are expected to live in their own crates and repositories. They are not merged into this repository.

## Guide sections

- [Architecture](Distributor-Guide-Architecture) — how the core, passes, and distributions fit together.
- [Writing Compilation Passes](Distributor-Guide-Writing-Compilation-Passes) — implementing the `RticPass` trait.
- [Writing Distributions](Distributor-Guide-Writing-Distributions) — implementing backends and exposing `#[rtic::app]`.
- [Multibin and Multipac](Distributor-Guide-Multibin-Multipac) — building multi-binary and multi-PAC applications.

## Core concepts

- `RticMacroBuilder` — the API used by every distribution to assemble the macro pipeline.
- `CorePassBackend` — the target-specific interface for the core code generation phase.
- `RticPass` — the interface for syntax-transformation passes.
- Backend extension traits — passes such as `rtic-sw-pass` may define their own backend traits (e.g., `SwPassBackend`) for target-specific details.

Start with [Architecture](Distributor-Guide-Architecture) for the big picture.
