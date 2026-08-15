# rticx-expand

A cargo subcommand that expands RTICX applications into **plain, compilable
Rust** for debugging, inspection, and security vetting.

RTICX applications are written with attributes (`#[<distro>::app]`, `#[task]`,
`#[sw_task]`, …) that expand into a large amount of generated code before
rustc ever sees it. `cargo rticx-expand` shows you that generated code and,
optionally, replaces the attribute syntax in your source file with it so that
GDB and other tools work directly on the code that actually runs on the
device.

| Audience | Use case |
|----------|----------|
| Application developers | Debug compile errors hidden inside macro expansions |
| Application developers | Step-debug runtime behavior with GDB |
| Application developers | Security and correctness vetting of generated code |
| Distribution / pass maintainers | Inspect how each compilation pass transforms the module |

The full user-facing documentation lives in the
[wiki](https://github.com/rticx-rs/rticx/wiki/Debugging-and-Inspection).

## Install

```bash
cargo install rticx-expand
```

This installs the `cargo-rticx-expand` binary, invocable as
`cargo rticx-expand …`.

## Quick start

Run from your application crate (the one whose `Cargo.toml` depends on an
RTICX distribution):

```bash
# Print the complete expanded file to stdout (like `cargo expand`)
cargo rticx-expand --example hello_rtic --features swtasks

# Save it as a compilable file
cargo rticx-expand --example hello_rtic --features swtasks > expanded.rs

# Replace the #[…::app] module in your source file with the expansion
cargo rticx-expand --example hello_rtic --features swtasks --merge

# Put your original source back
cargo rticx-expand restore
```

The printed output is the **complete file**: the expanded module spliced into
your original source, so everything around it (`#![no_std]`, `#![no_main]`,
imports, statics, and any code after the module) is preserved and the result stays executable.

## Use cases

### 1. Debugging compile errors hidden in macro expansions

When an RTICX application fails to compile, rustc reports the error inside the
generated code, far away from the attributes that produced it — or, worse,
only shows an error at the attribute site with an ambiguous message. Expand
the application to see the real code and the real error location:

```bash
cargo rticx-expand --example hello_rtic --features swtasks > expanded.rs
```

The expansion is written even when `cargo check` fails (for example because of
an error in a task body), so you can inspect exactly what the pipeline
produced. If the macro itself panics, no expansion can be produced and your
sources are left untouched — the tool reports the cargo output in that case.

### 2. Step-debugging with GDB

GDB operates on compiled code, not attributes. To set breakpoints on the
generated entry point, dispatchers, interrupt handlers, or resource locks,
merge the expansion into your source:

```bash
cargo rticx-expand --example hello_rtic --features swtasks --merge
```

The `#[…::app]` module is replaced by the expanded code, your original is kept
as `<file>.old`, and the result is a plain Rust file. Build, flash, and debug
it as usual; `cargo rticx-expand restore` brings the original sources back
when you are done.

### 3. Security and correctness vetting

The expanded file shows *everything* the framework generates — entry point,
interrupt vectors, SRP ceilings and locks, dispatcher queues, resource
proxies, and the exact code paths that run on the device. Nothing is hidden
behind macro expansion, so the code can be audited line by line, diffed
against previous releases, or fed to static analysis tools.

## For distribution and pass maintainers

Snapshot the module after **every** pipeline stage — each compilation pass and
the core pass — into a directory of your choice:

```bash
cargo rticx-expand --example hello_rtic --features swtasks --expand-passes target/passes
```

The directory receives one file per stage, named so lexical order equals
pipeline order:

```
target/passes/00_original.rs        # the module exactly as the user wrote it
target/passes/01_SoftwareTasks.rs   # after the software tasks pass
target/passes/02_core.rs            # after the core pass (the final expansion)
```

Diff consecutive snapshots to see exactly what each stage changed — this is
the fastest way to debug your own compilation passes or verify that a
distribution expands the syntax you expect:

```bash
diff -u target/passes/00_original.rs target/passes/01_SoftwareTasks.rs
diff -u target/passes/01_SoftwareTasks.rs target/passes/02_core.rs
# or compare the whole chain side by side:
meld target/passes
```

When a pass fails, the tool writes `NN_<Pass>_input.rs` with the exact module
state the failing pass received; when core parsing fails,
`NN_post_passes.rs` holds the state after all passes. Snapshots are beautified
and formatted for readable diffs, and stale snapshots from previous runs are
removed on each invocation.

## How it works

`rticx-core` drives the expansion through environment variables, which the
tool sets for you (and which you can also set manually when invoking
`cargo check` directly):

| Variable | Effect |
|----------|--------|
| `RTICX_EXPAND` | Set to any value to enable expansion writing |
| `RTICX_EXPAND_PATH` | Optional directory override (default: `<target>/rticx-expand`) |
| `RTICX_EXPAND_PASS_DIR` | Optional directory for ordered per-stage snapshots |

The output directory and file names are derived automatically: the macro
detects which source file invoked it and names the files after that file
(`main_expanded.rs`, `hello_rtic_expanded.rs`, …).

## License

MIT
