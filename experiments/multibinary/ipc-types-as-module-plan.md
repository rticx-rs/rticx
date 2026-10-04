# Plan: `ipc-types` crate → generated `ipc_types` module

> **Status:** proposed, not started.
> **Scope:** the `experiments/multibinary` workspace only (it is a standalone
> cargo workspace; the root workspace and `COMPATIBILITY.md` are unaffected).

This is the working plan for turning the generated `ipc-types` **crate** into a
generated **`ipc_types` module** that the cross-binary pass re-emits into each
application's `#[app]` module.

It is written to be loaded over several fresh sessions. Work **phase by phase**;
each phase lists its files and an acceptance check. Update the checkboxes and
the "Progress log" at the bottom as you go.

---

## 1. Goal

Today `cargo xbin sync` generates a crate `<project-root>/ipc-types/`
(`Cargo.toml` + `src/lib.rs`), each app depends on it via
`ipc-types = { path = "../ipc-types" }`, and the generated `src/lib.rs` does
`unsafe impl rticx_xbin_rt::CrossCoreMessage for X {}` (so the crate depends on
`rticx-xbin-rt`).

Target design:

1. `cargo xbin sync` writes a single file
   `<project-root>/target/rticx-xbin/ipc_types.rs` (next to `system.json`).
   No `Cargo.toml`, no `rticx-xbin-rt` dependency, no checked-in generated crate.
2. The cross-binary pass injects, into each application's `#[app]` module:
   - the `unsafe trait CrossCoreMessage` definition (the same pattern as
     `RticSwTask` from the software pass and `RticIdleTask` from the core pass);
   - a `pub mod ipc_types { … }` whose body is the **re-emitted tokens** of the
     generated file.
3. User code keeps writing `ipc_types::TypeX` *inside the app module*.
4. The runtime `rticx-xbin-rt` no longer defines `CrossCoreMessage`; its
   `Fifo`/`Producer`/`Consumer` bounds stay `T: Copy + 'static` (already the case).
5. `cargo rticx-expand` prints the fully expanded `ipc_types` module, because
   the pass splices the tokens (an `include!` would not be expanded by the tool).

## 2. Locked decisions

| Decision | Choice |
|---|---|
| Trait location | Injected by `rticx-xbin-pass` into the `#[app]` module; **deleted** from `rticx-xbin-rt`. |
| File → module | Pass **re-emits the file's tokens** (not `include!`/`#[path]`), so `cargo rticx-expand` shows them. |
| Generated file | `<project-root>/target/rticx-xbin/ipc_types.rs`. |
| Producer bounds | Keep `T: Copy + 'static` in the runtime; do **not** reintroduce a `CrossCoreMessage` bound. |

### External tasks

A cross receiver's task **struct** is declared inside `#[app]` (that is how
RTICX external tasks work — see `wiki/User-Guide-Syntax.md` §"External tasks").
Its `impl RticSwTask` may live in an external module. There it writes:

```rust
use crate::app::ipc_types;             // then: type SpawnInput = ipc_types::EncryptReq;
// or fully qualified: type SpawnInput = crate::app::ipc_types::EncryptReq;
```

If a shorter path is wanted, a single `pub use app::ipc_types;` in `main.rs`
gives external modules `crate::ipc_types::EncryptReq`. The framework does **not**
emit a crate-root alias: the `#[app]` attribute only owns the module, so that
would need per-distro macro surgery.

### Verified Rust semantics (do not re-litigate)

- A child module does **not** see parent items by bare name. The generated
  module must start with `use super::CrossCoreMessage;` (or use `super::…`).
- `include!` of a file with inner attributes (`#![…]`) is a hard error
  (`an inner attribute is not permitted in this context`). The generated file
  must contain **no inner attributes**.
- A wrapper `pub mod ipc_types { use super::T; include!(…) }` with
  `unsafe impl T for X {}` compiles.

## 3. Current state map (orientation)

| Area | File | What to touch |
|---|---|---|
| Generator | `crates/rticx-xbin-proto/src/codegen.rs` | `generate_crate`, `GeneratedCrate`, `GeneratedFile`, `GENERATED_CRATE_NAME`, `RtDependency`, `CodegenOptions`, `generate_cargo_toml` |
| Exports/docs | `crates/rticx-xbin-proto/src/lib.rs` | re-exports + module docs |
| Driver | `crates/rticx-xbin-driver/src/commands.rs` | `sync_with_output`, `IpcTypesOutcome`, `rt_dependency`, `relative_path`, `canonicalize_allow_missing` |
| Driver consts | `crates/rticx-xbin-driver/src/project.rs` | add `IPC_TYPES_FILE` |
| Pass codegen | `crates/rticx-xbin-pass/src/codegen.rs` | `generate_receiver_items` (assertion), `input_type`, docs; add module+trait emitter |
| Pass entry | `crates/rticx-xbin-pass/src/lib.rs` | `run_pass` codegen branch; docs |
| Runtime | `crates/rticx-xbin-rt/src/lib.rs` | delete `CrossCoreMessage` |
| Fixtures | `crates/mock/fixtures/{three-app,e2e,metadata}/ipc-types/` | delete; update app `Cargo.toml`s + comments |
| STM32H7 demo | `rticx-stm32h7/examples-apps/ipc-types/`, app `Cargo.toml`s, `.gitignore` | delete; update |
| Docs | `docs/architecture.md`, `docs/user-guide.md`, `multibinary-multicore-plan.md`, `rticx-stm32h7/rticx-stm32h7-macro/README.md` | wording |

Relevant pass symbols:

- `crates/rticx-xbin-pass/src/codegen.rs`
  - `XbinPassBackend::ipc_types_path()` (default `ipc_types`),
  - `generate_receiver_items(...)` — emits
    `fn __rticx_xbin_assert_cross_core_message<T: #rt_path::CrossCoreMessage>()`,
  - `input_type(task, backend)` — `#ipc_types_path::#type_ident`,
  - `generate_freshness_items(view, path)` — already emits an absolute
    `include_str!` anchor; reuse the same absolute-path helper for the new file.
- `crates/rticx-xbin-pass/src/lib.rs`
  - `run_pass` loads `system.path` in codegen mode via `system_from_env()` and
    `load_system`; the module file is `system.path.parent().join(IPC_TYPES_FILE)`.

---

## 4. Phases

### Phase 1 — Generator: single `ipc_types.rs` (`rticx-xbin-proto`)

**Files:** `crates/rticx-xbin-proto/src/codegen.rs`, `src/lib.rs`, `tests/*`.

- [ ] Replace `generate_crate` with
      `pub fn generate_module(idl: &IpcTypes) -> Result<String, CodegenError>`
      returning the file contents.
- [ ] Delete `CodegenOptions`, `RtDependency`, `DEFAULT_RT_PATH`,
      `generate_cargo_toml`, `GeneratedFile`; keep `canonical_layout_text`,
      `layout_hash`, `fnv1a64`, `ConstRegistry`.
- [ ] Decide the fate of `GeneratedCrate`/`CrateStatus`/`FileStatus`:
      keep a minimal single-file API, e.g.
      `pub fn write_if_changed(path: &Path, contents: &str) -> io::Result<FileChange>`
      (reuse `FileChange::{Created,Updated,Unchanged}`), or keep a tiny
      `GeneratedFile { path, contents }` wrapper. Keep change detection so an
      unchanged IDL is a no-op.
- [ ] Add `pub const IPC_TYPES_FILE: &str = "ipc_types.rs";` (re-export from
      `lib.rs`).
- [ ] `HEADER`: drop `#![no_std]` and `#![allow(non_camel_case_types, non_snake_case)]`
      (no inner attributes in the module body; the wrapper carries the outer
      `#[allow]`).
- [ ] Emit `unsafe impl CrossCoreMessage for {name} {}` (bare name). Everything
      else (repr(C) structs, `SIZE_*`/`ALIGN_*`/`OFF_*`, `LAYOUT_HASH`,
      `const _: () = { … }` assertions) is unchanged.
- [ ] Update `src/lib.rs` docs (the `codegen` bullet and the "generated crate
      depends on `rticx-xbin-rt`" note) and the re-exports.
- [ ] Tests:
  - [ ] `tests/generate.rs`: golden is now one file; drop Cargo.toml and
        `rt_dependency_path_and_version_forms` tests; keep determinism,
        `layout_hash_is_stable`, canonical-text and collision tests; adapt
        `write_if_changed_*` to one file.
  - [ ] `tests/fixtures/golden_lib.rs` → `tests/fixtures/golden_ipc_types.rs`
        (drop inner attrs, bare `CrossCoreMessage`); delete `golden_cargo.toml`.
  - [ ] `tests/generate_compile.rs`: compile a wrapper crate with **no**
        `rticx-xbin-rt` dependency:
        `#![no_std]` + `pub unsafe trait CrossCoreMessage: Copy + 'static {}` +
        `#[allow(non_camel_case_types, non_snake_case)] pub mod ipc_types { use super::CrossCoreMessage; include!(...) }`
        (or inline the generated text); keep host + ≥2 embedded targets.

**Acceptance:** `cargo test -p rticx-xbin-proto` passes; generated text contains
no `rticx_xbin_rt` and no inner attributes.

### Phase 2 — Runtime: remove `CrossCoreMessage` (`rticx-xbin-rt`)

**Files:** `crates/rticx-xbin-rt/src/lib.rs`, `tests/fifo.rs`,
`crates/mock/rticx-xbin-mock/tests/backend.rs`.

- [ ] Delete `pub unsafe trait CrossCoreMessage` and the doc bullet referencing
      it (the module doc says "implemented only by the generated `ipc-types`").
- [ ] Fix `tests/fifo.rs` and the mock backend test: drop the marker impls or
      define a local `trait CrossCoreMessage` where the test's intent needs it
      (`Fifo`/`Producer`/`Consumer` only need `Copy + 'static`).
- [ ] Grep the workspace to confirm no remaining reference:
      `rg 'CrossCoreMessage' experiments/multibinary/crates/rticx-xbin-rt`.

**Acceptance:** `cargo test -p rticx-xbin-rt` passes; no `CrossCoreMessage`
symbol in the runtime.

### Phase 3 — Driver: write the module file (`rticx-xbin-driver`)

**Files:** `crates/rticx-xbin-driver/src/commands.rs`, `src/project.rs`,
`src/lib.rs`, `tests/*`.

- [ ] Add/re-export `IPC_TYPES_FILE` (from `rticx-xbin-proto`); keep
      `SYSTEM_FILE` in `project.rs`.
- [ ] `sync_with_output`: when the project has an `ipc-types.toml`, call
      `generate_module(idl)` and write it to `output_dir.join(IPC_TYPES_FILE)`
      with change detection. Store the file path in `IpcTypesOutcome.path`.
- [ ] Delete `rt_dependency`, `relative_path`, `canonicalize_allow_missing`, and
      the `CodegenOptions`/`RtDependency`/`GENERATED_CRATE_NAME` imports.
- [ ] `src/lib.rs`: reporting text stays the same shape
      (`created`/`updated`/`up to date`).
- [ ] Tests (`tests/driver.rs`, `tests/cli.rs`, `tests/fixture.rs`,
      `tests/e2e.rs`, `tests/three_app.rs`, `tests/negative.rs`): expect
      `target/rticx-xbin/ipc_types.rs`, no `Cargo.toml`, no checked-in crate;
      update `GENERATED_CRATE_NAME` usages.

**Acceptance:** `cargo test -p rticx-xbin-driver` passes (except fixture tests
that need Phases 4–5; sequence accordingly or land Phases 3–5 together).

### Phase 4 — Pass: inject trait + re-emit module (`rticx-xbin-pass`)

**Files:** `crates/rticx-xbin-pass/src/codegen.rs`, `src/lib.rs`, `tests/*`.

- [ ] Add `generate_ipc_types_items(system_path: &Path, view: &SystemView) -> syn::Result<Vec<Item>>`:
  - return `vec![]` when `view.types` is empty (project has no IDL);
  - `let file = system_path.parent().join(rticx_xbin_proto::IPC_TYPES_FILE);`
  - if `view.types` is non-empty and `file` is missing → hard error
    ("run `cargo xbin sync`");
  - `let parsed = syn::parse_file(&std::fs::read_to_string(&file)?)?;` and drop
    any `parsed.attrs` defensively;
  - emit the trait:
    `pub unsafe trait CrossCoreMessage: Copy + 'static {}`;
  - emit the module:
    ```
    #[allow(non_camel_case_types, non_snake_case, unused_imports)]
    pub mod ipc_types {
        use super::CrossCoreMessage;
        <parsed.items…>
    }
    ```
  - emit the freshness anchor (absolute path):
    `const _: &str = include_str!(#abs_file);`
- [ ] Module name: derive from `backend.ipc_types_path()`'s last segment (default
      `ipc_types`); document that a multi-segment path is unsupported for the
      injected module.
- [ ] (`run_pass`, codegen branch) extend `items` with
      `generate_ipc_types_items(&system.path, &view)?` and append.
- [ ] Change the receiver assertion to bare `CrossCoreMessage`:
      `fn __rticx_xbin_assert_cross_core_message<T: CrossCoreMessage>() {}`.
- [ ] Optional: reject a user-defined `CrossCoreMessage`/`ipc_types` in the app
      module with a clear error.
- [ ] Update module docs (`lib.rs` lines ~34–37, 88–89; `codegen.rs` header).
- [ ] Tests:
  - [ ] `tests/codegen.rs`: assertion expectation becomes bare
        `CrossCoreMessage`; add a test that the trait + `ipc_types` module are
        emitted and the file is missing → error.
  - [ ] `tests/init_hooks.rs`, `tests/receiver_compile.rs`,
        `tests/e2e_runtime.rs`: replace the hand-written `ipc-types` crate with a
        hand-written `target/rticx-xbin/ipc_types.rs` next to their
        `system.json`; use `app::ipc_types::X` instead of crate-root
        `ipc_types::X`; drop the `ipc-types` dependency from the generated
        `Cargo.toml`s.
  - [ ] `tests/ipc_dispatchers.rs`, `tests/priority_lines.rs`,
        `tests/syntax.rs`, `tests/metadata.rs`: only the `ipc_types::` type
        strings remain valid (no change unless they build crates).

**Acceptance:** `cargo test -p rticx-xbin-pass` passes; `cargo rticx-expand` on
a synced app prints the trait and the full `ipc_types` module.

### Phase 5 — Fixtures & in-tree distro

- [ ] Delete `crates/mock/fixtures/three-app/ipc-types/`,
      `crates/mock/fixtures/e2e/ipc-types/`,
      `crates/mock/fixtures/metadata/ipc-types/`,
      `rticx-stm32h7/examples-apps/ipc-types/`.
- [ ] Remove `ipc-types = { path = "../ipc-types" }` from every app
      `Cargo.toml` (`three-app/app-{m7,m5,m4}`, `e2e/app-*`,
      `metadata/app-*`, `rticx-stm32h7/examples-apps/app-{cm7,cm4}`).
- [ ] Regenerate/trim the fixture `Cargo.lock` files (drop the `ipc-types`
      package). Fixtures: `crates/mock/fixtures/{e2e,three-app}/Cargo.lock` and
      `rticx-stm32h7/examples-apps/Cargo.lock`.
- [ ] Update comments/manifests that describe a checked-in generated crate
      (workspace headers, `rticx-stm32h7/.gitignore`, app READMEs).
- [ ] `metadata-macro`: metadata mode never injects; leave functional, fix the
      misleading `rt_path` if convenient.

**Acceptance:** the fixture builds driven by the driver succeed; no
`ipc-types/` directories remain.

### Phase 6 — Docs

- [ ] `docs/architecture.md` (crate table, `cargo xbin sync` section, "crate
      generator" wording).
- [ ] `docs/user-guide.md` (`ipc-types.toml` section, the "generated crate"
      paragraph, task syntax examples, reference list).
- [ ] `multibinary-multicore-plan.md` §6.3 ("Generated `ipc-types` crate"), §6.5
      (remove "add `ipc-types` as a dependency"), §7/§8.
- [ ] `rticx-stm32h7/rticx-stm32h7-macro/README.md`.
- [ ] Document: injected `CrossCoreMessage`, the `target/rticx-xbin/ipc_types.rs`
      location, the external-task import pattern.

### Phase 7 — Validation

- [ ] `cd experiments/multibinary && cargo test` for each touched crate.
- [ ] `cargo fmt` (and clippy) in `experiments/multibinary`.
- [ ] Driver end-to-end fixture tests (`three_app.rs`, `e2e.rs`) build all apps.
- [ ] `cargo xbin sync` twice on a fixture: second run reports `up to date` and
      produces byte-identical `system.json` + `ipc_types.rs`.
- [ ] `cargo rticx-expand` on an app shows the expanded module.
- [ ] STM32H7 demo builds (`make` in the demo, or the driver build); run under
      Renode/QEMU if available.

---

## 5. Risks & edge cases

- **Scope:** `ipc_types` is nameable only inside `#[app]` and its descendants.
  External modules use `crate::app::ipc_types::…`. Documented.
- **Phase 1:** metadata mode halts before parsing, so an unresolved `ipc_types`
  in the `cargo xbin sync` `cargo check` is irrelevant (unchanged).
- **Pass ordering:** confirm the software pass and core pass preserve the
  injected `pub mod ipc_types` item (they already preserve generated xbin items;
  add a targeted test).
- **Freshness:** the pass reads the file at expansion and *re-emits* it, so rustc
  does not track the file by itself. The `include_str!` anchor is required.
- **Missing file:** clear "run `cargo xbin sync`" error when `view.types` is
  non-empty but `ipc_types.rs` is absent.
- **Name collisions:** a user item named `CrossCoreMessage` or `ipc_types` in the
  app module; detect or let rustc error.
- **`cargo rticx-expand` without a synced view** still errors, as today.

---

## 6. Progress log

> Append a dated line per session, noting phase + status + any deviation.

- (not started)
