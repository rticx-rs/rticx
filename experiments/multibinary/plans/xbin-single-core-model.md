# Simplify the cross-binary extension to one core per binary

## Goal

Replace the multi-core-shaped project model of the experimental `rticx-xbin`
extension with a single-core-per-binary model: `rticx.toml` declares one
`core_id` per `[[application]]`, `#[app]` may not declare `cores`, and
`external_cores` is gone — the cross-binary edges are inferred entirely from
each receiver's `spawn_by`.

## Context

The extension is a standalone Cargo workspace at `experiments/multibinary/`
(its own `[workspace]`, deliberately outside the root workspace). **All
commands below run from `experiments/multibinary/`.**

Crates touched:

| Crate | Path |
|---|---|
| `rticx-xbin-proto` | `crates/rticx-xbin-proto/` |
| `rticx-xbin-pass` | `crates/rticx-xbin-pass/` |
| `rticx-xbin-driver` | `crates/rticx-xbin-driver/` |
| `xbin-mock-distro` | `crates/mock/xbin-mock-distro/` |
| `xbin-mock-capability` | `crates/mock/xbin-mock-capability/` |
| `metadata-macro` (fixture) | `crates/mock/fixtures/metadata/metadata-macro/` |
| `rticx-stm32h7-macro` | `rticx-stm32h7/rticx-stm32h7-macro/` |

Test commands (run each from `experiments/multibinary/`):

```bash
cargo test -p rticx-xbin-proto          # proto unit + integration tests
cargo test -p rticx-xbin-pass           # pass unit + integration tests
cargo test -p rticx-xbin-driver         # driver tests (builds the fixtures via cargo)
cargo test -p xbin-mock-distro          # mock distribution unit tests
cargo fmt --all                         # rustfmt the whole experimental workspace
cargo check -p rticx-stm32h7-macro -p rticx-stm32h7-bindings
```

`cargo test -p <crate>` builds only that crate's dependency graph, so the
workspace does **not** have to compile as a whole for a scoped phase to be
green. `crates/mock/xbin-mock-distro` and `crates/mock/fixtures/**` are part of
the experimental workspace (fixtures are standalone sub-workspaces), so they
block a workspace-wide `cargo test` until Phase 3.

### Confirmed design decisions (do not re-derive)

1. `rticx.toml` `schema` stays `1`. Each `[[application]]` has a **required**
   scalar `core_id = N` (globally unique). The old array key `core_ids` and the
   removed `external_cores` are hard errors with migration wording (same style
   as the already-removed `[ipc.regions]`).
2. Full scalar model: single `core_id` flows through `Application`,
   `AppTopology`, `AppEntry`; `CoreEntry` loses `local_index`.
3. `#[app]` in a cross-binary application may **not** declare `cores`,
   `core_id`, `core_ids` or `external_cores` (all four are errors). Internally
   the pass injects `core_ids = [core_id]` (for `rticx-core`/sw-pass) and does
   **not** inject `cores` (defaults to 1).
4. `dispatchers` is a **flat** list only; the nested one-array-per-core shape is
   an error.
5. A cross receiver may **not** declare `core` (always local core 0; the pass
   injects `core = 0` on the generated `#[task]`).
6. `#[sw_task(spawn_by = G)]` **always** names a global producer core id ⇒ a
   cross-binary receiver. A `#[sw_task]` with no `spawn_by` is a plain local
   software task. `spawn_by == this binary's own core_id` is a **hard error**
   ("it is a core-local task; drop `spawn_by`").
7. The producer declares nothing. The driver already infers each producer app
   from `task.spawner_core == app.core_id`; removing `external_cores` deletes
   only the two-sided *visibility* checks.
8. Schema bumps: `system.json` 3 → 4, application manifest 5 → 6.

### Gotchas

- Pass integration tests (`crates/rticx-xbin-pass/tests/*.rs`) embed their own
  `system.json` / `rticx.toml` string constants and build temp projects; they do
  **not** read the checked-in fixtures. They must be rewritten to the new
  schema. Most of them re-`seal()` the view (`with_interface_hashes`,
  `system_with`), so their embedded `topology_hash` is recomputed; the few that
  write the const verbatim (`write_system`, `init_hooks.rs`,
  `receiver_compile.rs`) need their `topology_hash` regenerated — the pass's
  `load_system` error prints both the stored and the computed value
  ("its `topology_hash` (0x..) does not match its contents (0x..)"), so iterate
  until green.
- Driver tests serialize access to shared checked-in fixtures with
  `common::fixture_lock()` (see `crates/rticx-xbin-driver/tests/common/mod.rs`).
- Pass tests that mutate the process environment use `support::ENV_LOCK`
  (`crates/rticx-xbin-pass/tests/support/mod.rs`).
- `rticx-xbin-pass` is bound by `xbin-mock-distro`, `metadata-macro` and
  `rticx-stm32h7-macro`; all three consume its public API (`AppTopology`,
  `resolve_app_topology`, `XbinPassBackend`). `xbin-mock-capability` only uses
  `IpcPool`/`PhysicalCore`/`PoolId`, which do not change.

## Definition of done

- `rticx.toml` uses `core_id = N`; `core_ids` and `external_cores` fail with
  actionable migration errors.
- No `#[app]` in the extension accepts `cores` / `core_id` / `core_ids` /
  `external_cores`; `dispatchers` is flat; receivers have no `core`.
- A receiver's `spawn_by` alone determines its producer; nothing declares
  `external_cores`.
- `AppTopology`, `AppEntry`, `CoreEntry`, `AppExtensions`, `AppManifest`,
  `ReceiverDecl`, `CoreCapability` are single-core shaped.
- All of these pass from `experiments/multibinary/`:

  ```bash
  cargo fmt --all -- --check
  cargo test -p rticx-xbin-proto
  cargo test -p rticx-xbin-pass
  cargo test -p rticx-xbin-driver
  cargo test -p xbin-mock-distro
  cargo test                                   # workspace default-members
  cargo check -p rticx-stm32h7-macro -p rticx-stm32h7-bindings
  ```

- `docs/user-guide.md` and `docs/architecture.md` describe the single-core
  model (no `core_ids`/`external_cores` as user-facing keys) and their error
  tables match the code.

## Out of scope

- Any change to the root workspace (`rticx-core`, `rticx-sw-pass`,
  `rticx-cortex-m`, …). The extension only uses their frozen surfaces.
- Rewriting the runtime (`rticx-xbin-rt`, `rticx-xbin-mock`) — the transport is
  unaffected by topology shape.
- The STM32H7 distributions' behaviour beyond their `examples-apps/rticx.toml`.

---

## Phase 1 — `rticx-xbin-proto`: scalar `core_id` + schema bump

**Objective:** `rticx.toml` parses `core_id`, the system view and manifests
carry single-core app entries, and merge infers the producer from `spawn_by`.

**Files:**
- `crates/rticx-xbin-proto/src/project.rs` — `Application` becomes
  `{ package, target, core_id: u32 }`; parse a required `core_id`; reject
  `core_ids` / `external_cores` with migration errors; update the unknown-key
  list; `validate()` rejects duplicate `core_id`; `to_toml_string()` renders
  `core_id = N`; `ProjectConfig::core_ids()` iterates each app's single id.
- `crates/rticx-xbin-proto/src/error.rs` — remove
  `MergeError::{TargetCoreNotVisible, SpawnerCoreNotVisible,
  ReceiverCoreOutOfRange}`; keep `UnknownCore` (its `context` becomes e.g.
  `` `spawn_by` of receiver `T` ``) and `OwnSpawnerCore` (message: the
  receiver's `spawn_by` names its own core / a core-local task).
- `crates/rticx-xbin-proto/src/manifest.rs` — `ReceiverDecl` drops `core`;
  `AppManifest.capabilities: Vec<CoreCapability>` becomes a single
  `capability: Option<CoreCapability>`; `CoreCapability` drops `local_core`;
  bump `MANIFEST_SCHEMA_VERSION` 5 → 6; update docs.
- `crates/rticx-xbin-proto/src/merge.rs` — build the core table from
  `application.core_id()`; drop the `external_cores` existence loop and the two
  visibility checks; `receiver_core = application.core_id()`; a receiver's
  `spawn_by` must exist in the owner map and differ from its own core
  (`OwnSpawnerCore` otherwise); `match_pools` reads each app's single
  `capability` and uses `application.core_id()` as its global id; build
  `AppEntry { core_id, .. }`.
- `crates/rticx-xbin-proto/src/system.rs` — `AppEntry` becomes
  `{ package, target, interface_hash, core_id: u32 }` (drop `core_ids` and
  `external_cores`); `CoreEntry` drops `local_index`; bump
  `SYSTEM_SCHEMA_VERSION` 3 → 4; update `canonical_topology_text()`.
- `crates/rticx-xbin-proto/src/lib.rs` — update re-exports if any removed item
  was public.
- Tests: `crates/rticx-xbin-proto/tests/project.rs`, `merge.rs`, `alloc.rs`,
  `system.rs`, `manifest.rs`.

**Steps (TDD):**
1. In `tests/project.rs`, rewrite the fixtures to `core_id = N` and add/port
   assertions for: `core_id` required; `core_ids` rejected with the migration
   message; `external_cores` rejected with the removal message; duplicate
   `core_id` rejected; canonical round-trip. Run
   `cargo test -p rticx-xbin-proto --test project`; expect RED (compile error
   against the old `Application` API).
2. Implement `project.rs` + `error.rs`; re-run `--test project`; expect GREEN.
3. In `tests/system.rs` and `tests/manifest.rs`, assert the new field shapes
   (`AppEntry.core_id` scalar, no `external_cores`, `CoreEntry` without
   `local_index`, single manifest `capability`, `ReceiverDecl` without `core`)
   and `SYSTEM_SCHEMA_VERSION == 4` / `MANIFEST_SCHEMA_VERSION == 6`. Run
   `cargo test -p rticx-xbin-proto`; expect RED.
4. Implement `system.rs` + `manifest.rs`; expect the shape tests GREEN.
5. In `tests/merge.rs` and `tests/alloc.rs`, rewrite fixtures to `core_id`, and
   replace the visibility tests with: producer inferred from `spawn_by`;
   `OwnSpawnerCore` when `spawn_by == own core_id`; `UnknownCore` when
   `spawn_by` names an undeclared core. Run `cargo test -p rticx-xbin-proto`;
   expect RED on merge, then implement `merge.rs`; expect GREEN.
6. Refactor `merge.rs` comments/module docs to describe inference instead of
   visibility; keep tests GREEN.

**Exit criteria / verification (from `experiments/multibinary/`):**
- [ ] `cargo test -p rticx-xbin-proto` passes
- [ ] `cargo fmt --all`
- [ ] `grep -rn "external_cores" crates/rticx-xbin-proto/src` shows only the
      migration-error wording, no data-model field
- [ ] Note in the progress log that `rticx-xbin-pass`/`driver` do not compile
      yet (expected; Phases 2–3)

---

## Phase 2 — `rticx-xbin-pass`: single-core topology, syntax, codegen

**Objective:** the pass rejects multi-core `#[app]` syntax, classifies
`spawn_by` globally, and generates code from a single `core_id` with a flat
dispatcher list.

**Files:**
- `crates/rticx-xbin-pass/src/topology.rs` — `AppTopology { core_id: u32 }` with
  `new(core_id)`, `core_id()`; replace `identity(cores)` with a standalone
  `core_id = 0` constructor; `resolve_app_topology` reads `application.core_id()`.
- `crates/rticx-xbin-pass/src/parse.rs` — `AppExtensions { core_id, pac:
  syn::Path, dispatchers: Vec<Path>, cross_dispatchers: Vec<Path> }`; reject
  `cores`, `core_id`, `core_ids`, `external_cores` in `#[app]`; inject only
  `core_ids = [core_id]`; `parse_dispatchers` returns a flat `Vec<Path>` and
  errors on the nested shape; `steal_dispatchers` operates on one flat list;
  `parse_module` classifies any `spawn_by` as a global cross-binary receiver and
  hard-errors when it equals `extensions.core_id`; `parse_receiver` rejects a
  `core` key and builds `ReceiverDecl` without `core`; `inject_receiver_tasks`
  emits `core = 0`.
- `crates/rticx-xbin-pass/src/lib.rs` — `TopologySource::FromArgs` becomes the
  standalone single-core source; `interface_hash` hashes the single `core_id`
  and the flat dispatcher list (drop `cores`/`core_ids`/`external_cores`);
  `build_manifest` emits a single `capability`.
- `crates/rticx-xbin-pass/src/codegen.rs` — sender filter
  `task.spawner_core == application.core_id`; `HookPlan { core_id,
  initializes_fifos: bool }`; `generate_init_hooks` handles one core;
  `LineGroup` drops `local_core` and indexes `extensions.cross_dispatchers`
  flat; `local_pend_fn_ident` always returns `__rticx_local_irq_pend`;
  `interrupt_path` uses `extensions.pac`; `resolve_receiver` compares
  `task.receiver_core == application.core_id` and drops the local-core check.
- `crates/rticx-xbin-pass/src/priority.rs` — ownership via
  `task.receiver_core == application.core_id`.
- Tests: `src/*` unit tests and `tests/{syntax,core_ids,dispatchers,metadata,
  codegen,receiver_compile,init_hooks,priority_lines,e2e_runtime}.rs`,
  `tests/support/mod.rs` if helpers change.

**Steps (TDD):**
1. In `tests/syntax.rs` and `tests/core_ids.rs`, rewrite the topology constants
   to `core_id` and add tests: `#[app(cores = N)]` rejected; `#[app(core_id =
   N)]` / `#[app(core_ids = [..])]` / `#[app(external_cores = [..])]` rejected
   with the migration messages; any `spawn_by` is a cross receiver;
   `spawn_by == own core_id` is a hard error; a no-`spawn_by` task stays local;
   a receiver `core` key is rejected. Run
   `cargo test -p rticx-xbin-pass --test syntax --test core_ids`; expect RED.
2. Implement `topology.rs` + `parse.rs`; re-run those two test binaries; expect
   GREEN.
3. In `tests/dispatchers.rs`, assert the flat `dispatchers` list is stolen from
   its front and the nested shape errors. Run
   `cargo test -p rticx-xbin-pass --test dispatchers`; expect RED, then GREEN
   after the `steal_dispatchers`/`parse_dispatchers` change.
4. In `tests/metadata.rs` and `tests/codegen.rs`, rewrite the embedded
   `system.json`/`rticx.toml` constants to schema 4 (scalar `core_id`, no
   `external_cores`, `CoreEntry` without `local_index`, flat dispatchers) and
   update the manifest assertions (`capability` singular, `ReceiverDecl`
   without `core`). Run `cargo test -p rticx-xbin-pass --test metadata --test
   codegen`; expect RED. Implement `lib.rs` + `codegen.rs`; expect GREEN.
   Regenerate verbatim-embedded `topology_hash` values from the pass's
   stale-hash error (see Gotchas).
5. Repeat step 4's rewrite for `tests/receiver_compile.rs`,
   `tests/init_hooks.rs`, `tests/priority_lines.rs` and
   `tests/e2e_runtime.rs`; run each `--test` and make it GREEN.
6. Refactor: delete now-unused `SpawnByNamespace`, per-core dispatcher loops and
   local-core plumbing; keep every test GREEN.

**Exit criteria / verification (from `experiments/multibinary/`):**
- [ ] `cargo test -p rticx-xbin-pass` passes (unit + all integration tests)
- [ ] `cargo fmt --all`
- [ ] `grep -rn "external_cores\|core_ids\[" crates/rticx-xbin-pass/src` shows
      only migration-error wording / the injected `core_ids` key, no per-core
      vectors
- [ ] Note in the progress log that the workspace still does not build
      workspace-wide (mock distro/fixtures/driver pending, Phase 3)

---

## Phase 3 — Driver, distributions and fixtures, end to end

**Objective:** the driver and the mock/H7 distributions use the scalar model,
the checked-in fixture projects declare `core_id`, and the driver's end-to-end
tests build every fixture.

**Files:**
- `crates/mock/xbin-mock-distro/src/lib.rs` — `MockDistroBackend { core_id }`;
  `physical(0)` returns `core_id`; `app_topology` falls back to
  `AppTopology::new(0)`; drop `declared_cores`; the injected backend helper uses
  `topology.core_id()`.
- `crates/mock/fixtures/metadata/metadata-macro/src/lib.rs` — same scalar
  adaptation (`MetadataCapabilityBackend { core_id }`, drop `declared_cores`).
- `crates/mock/fixtures/e2e/rticx.toml`, `crates/mock/fixtures/three-app/rticx.toml`,
  `crates/mock/fixtures/metadata/rticx.toml` — `core_id = N`, drop
  `external_cores`.
- `rticx-stm32h7/examples-apps/rticx.toml` — `core_id = N`, drop
  `external_cores`.
- `crates/rticx-xbin-driver/src/visualize.rs` (+
  `crates/rticx-xbin-driver/src/visualize/app.js`) — drop `CoreEntry.local_index`
  from `meta`, `CoreCard` and the JS `"local " + local_index` label.
- `crates/rticx-xbin-driver/src/elf.rs` — construct `AppEntry`/`CoreEntry` with
  the new fields.
- Driver tests: `crates/rticx-xbin-driver/tests/{fixture,negative,e2e,three_app,
  verify,freshness,driver,cli}.rs`.

**Steps (TDD):**
1. Update the three mock fixtures' `rticx.toml` and the H7 example
   `rticx.toml` to `core_id`, then run
   `cargo test -p rticx-xbin-driver --test fixture --test negative`; expect RED
   (mock distro / driver source do not compile against the new APIs).
2. Implement `xbin-mock-distro/src/lib.rs` and
   `metadata-macro/src/lib.rs`; run `cargo test -p xbin-mock-distro`; expect
   GREEN (update its `physical_core_maps_...` unit test to a single `core_id`).
3. Port `visualize.rs`/`app.js` and `elf.rs`; run
   `cargo build -p rticx-xbin-driver`; expect it to compile.
4. In `tests/fixture.rs`, drop the `task.core == 0` assertion, port the
   `app.core_id` expectations, and keep the manifest/capability assertions
   (`producer.receivers.is_empty()`, single `capability`). Run
   `cargo test -p rticx-xbin-driver --test fixture`; expect RED, then GREEN.
5. In `tests/negative.rs`, replace the `#[app(core_ids)]` /
   `#[app(external_cores)]` cases with the new `#[app(cores)]` /
   `#[app(core_id)]` / `#[app(core_ids)]` / `#[app(external_cores)]` messages
   and the `rticx.toml` `core_ids` / `external_cores` migration messages;
   remove the two-sided-visibility case; add a `spawn_by == own core_id`
   core-local-task case. Run `cargo test -p rticx-xbin-driver --test negative`;
   expect RED, then GREEN.
6. Port `tests/e2e.rs`, `tests/three_app.rs`, `tests/verify.rs`,
   `tests/freshness.rs`, `tests/driver.rs`, `tests/cli.rs` to the new schema
   (app entries, embedded views, `topology_hash` regeneration as in Phase 2).
   Run `cargo test -p rticx-xbin-driver`; expect GREEN.
7. Refactor: remove dead multi-core helpers/comments surfaced by the scalar
   model; keep tests GREEN.

**Exit criteria / verification (from `experiments/multibinary/`):**
- [ ] `cargo test -p rticx-xbin-driver` passes (sync/build/verify over the
      `e2e`, `three-app` and `metadata` fixtures)
- [ ] `cargo test -p xbin-mock-distro` passes
- [ ] `cargo fmt --all`
- [ ] `cargo check -p rticx-stm32h7-macro -p rticx-stm32h7-bindings`
- [ ] `grep -rn "external_cores\|core_ids" crates/mock/fixtures rticx-stm32h7/examples-apps --include=*.toml`
      returns nothing

---

## Phase 4 — Docs and full-workspace validation

**Objective:** the guides describe the single-core model, and the whole
experimental workspace is green.

**Files:**
- `experiments/multibinary/docs/user-guide.md` — section 4 (topology table:
  `core_id` required; drop `external_cores`; the example `rticx.toml`), section
  6 (`spawn_by` resolution: global always, `spawn_by == own core_id` error;
  receiver `core` removed), the `#[app]` arguments table (`cores` rejected;
  flat `dispatchers`), and the "Common errors" tables (drop the
  `external_cores` rows; add the `core_ids`/`external_cores` migration rows and
  the core-local-`spawn_by` row).
- `experiments/multibinary/docs/architecture.md` — sections 5 (crate map:
  `rticx.toml` parse), 6 (sync steps + `system.json` shape/version), 7
  (codegen), 8 (interface fingerprint), 9/10 (§`core_ids`/`external_cores`
  references).

**Steps:**
1. Edits are non-executable; verification is a grep sweep plus the full test
   run. First sweep:
   `grep -rn "external_cores" docs/` and `grep -rn "core_ids" docs/` and update
   every hit to the new model, leaving only migration-error quotes.
2. Regenerate the `system.json` shape description (schema 4, `apps[].core_id`,
   no `external_cores`, `cores[]` without `local_index`) and the manifest
   description (schema 6, single `capability`, `ReceiverDecl` without `core`).
3. Fix the error tables to match the real `error.rs` / `parse.rs` /
   `project.rs` messages.

**Exit criteria / verification (from `experiments/multibinary/`):**
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test -p rticx-xbin-proto` passes
- [ ] `cargo test -p rticx-xbin-pass` passes
- [ ] `cargo test -p rticx-xbin-driver` passes
- [ ] `cargo test -p xbin-mock-distro` passes
- [ ] `cargo test` (workspace default-members) passes
- [ ] `cargo check -p rticx-stm32h7-macro -p rticx-stm32h7-bindings` passes
- [ ] `grep -rn "external_cores" docs/` shows only migration-error wording;
      `rticx.toml` examples in `docs/` use `core_id`

---

## Progress log

| Phase | Status | Notes |
| ----- | ------ | ----- |
| 1 — proto scalar model + schema bump | TODO | expected: pass/driver temporarily uncompilable |
| 2 — pass single-core topology/syntax/codegen | TODO | largest phase |
| 3 — driver + distributions + fixtures | TODO | unblocks workspace-wide build |
| 4 — docs + full-workspace validation | TODO | |
