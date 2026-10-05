# Single-source project topology (`rticx.toml`)

**Status:** planned, not started.
**Scope:** `experiments/multibinary/` only. No root-workspace crate changes, so **no
RTICX generation bump** (`rticx-core`, `rticx-sw-pass`, `rticx-async-pass` are
untouched; single-binary `#[app(core_ids = …)]` keeps working there).
**Last updated:** 2026-10-06.

This document is split into **three implementation sessions**. Each session is
self-contained: it lists the files to touch, the exact behaviour, the tests to
migrate and the acceptance criteria. Work one session at a time and update the
[progress log](#progress-log) when a task finishes. If a session is still too
large, split it at the checkpoint marked in it.

> **How to resume:** load this file, read the [progress log](#progress-log),
> find the first unchecked task, implement it, tick it, add a one-line note (and
> the commit hash when committed). Keep the workspace green at every **session**
> boundary; individual tasks inside a session may leave tests momentarily red.

---

## 1. Motivation

Project topology is currently declared **twice** and cross-checked in three
places:

| Fact | `rticx.toml` | `#[app(...)]` | Reconciled in |
|---|---|---|---|
| local → global core mapping | `core_ids` (authoritative) | `core_ids` (native to `rticx-core`) | `merge.rs` `CoreIdsMismatch` |
| local core count | derived (`core_ids.len()`) | `cores` (default 1) | `merge.rs` `CoreCountMismatch` |
| visible foreign cores | — | `external_cores` (pass-owned) | `merge.rs` visibility checks |

The user-facing result is redundant and error-prone: a user can write a
`core_ids` mapping in `#[app]` that disagrees with `rticx.toml`, and
`external_cores` exists in exactly one place only by accident.

**Goal:** `rticx.toml` is the single user-facing source of topology. `#[app]`
keeps only what is genuinely per-binary syntax (`device`, `ipc_dispatchers`,
`dispatchers`, …). The cross-binary pass reads `rticx.toml`, derives `cores`
from `core_ids`, and **injects `cores` + `core_ids` into the token stream it
hands to the core pass**, so `rticx-core` needs no change.

---

## 2. Locked decisions

| Topic | Decision |
|---|---|
| `rticx.toml` key | `external_cores` (plural), optional, per `[[application]]` |
| `core_ids` | stays **required** on every `[[application]]` |
| Manifest | Session B drops `cores` / `core_ids` / `external_cores` from `AppManifest`; `MANIFEST_SCHEMA_VERSION` → **3** |
| Topology authority | `rticx.toml` (`ProjectConfig`); the manifest only reports source-discovered facts (receivers, types, capability pools) |
| Topology resolution | the pass walks up from `CARGO_MANIFEST_DIR` to the nearest `rticx.toml`; **no new env var** |
| `#[app(core_ids)]` | hard error pointing at `rticx.toml` |
| `#[app(external_cores)]` | hard error pointing at `rticx.toml` |
| `#[app(cores = N)]` | **optional for now**; if present it must equal `core_ids.len()`, otherwise a hard error with the "single-binary multicore" clarification |
| No project found | identity mapping over the declared `cores` (default 1); keeps a standalone xbin distribution usable |
| Project found but app absent | hard error |
| `system.json` `AppEntry.core_ids` / `external_cores` | **kept** (synced snapshot; the phase-2 pass and `check_application` use them) |
| `rticx.toml` `schema` | stays `1` (the new key is optional/additive) |

---

## 3. New user-facing shape

```toml
# rticx.toml
schema = 1

[[application]]
package = "app-m7"
target = { kind = "bin", name = "m7" }
core_ids = [0]
external_cores = [1]

[[application]]
package = "app-m4"
target = { kind = "bin", name = "m4" }
core_ids = [1]
external_cores = [0]
```

```rust
// app-m4/src/main.rs — no `cores`, `core_ids` or `external_cores`
#[app(device = mock_pac, ipc_dispatchers = [XBIN_IPC_LINE_0])]
mod app { /* … */ }
```

After the cross-binary pass runs, the core pass receives:

```rust
#[app(device = mock_pac, ipc_dispatchers = [XBIN_IPC_LINE_0],
      cores = 1, core_ids = [1])]
```

`spawn_by` semantics are **unchanged**: resolved in the global namespace iff the
application's `external_cores` is non-empty (now read from `rticx.toml`), local
index otherwise. `core` on a task stays a local index.

---

## 4. Mechanics

### 4.1 Topology resolution (pass side)

New module `crates/rticx-xbin-pass/src/topology.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppTopology { core_ids: Vec<u32>, external_cores: Vec<u32> }

impl AppTopology {
    pub fn new(core_ids: impl Into<Vec<u32>>, external_cores: impl Into<Vec<u32>>) -> Self;
    pub fn identity(cores: u32) -> Self;      // core_ids = 0..cores, external = []
    pub fn cores(&self) -> u32;               // core_ids.len() as u32
    pub fn core_ids(&self) -> &[u32];
    pub fn external_cores(&self) -> &[u32];
}

pub struct ResolvedTopology { pub topology: AppTopology, pub manifest_path: PathBuf }

/// Nearest `rticx.toml` above `CARGO_MANIFEST_DIR` (reuses the existing
/// `project_root_from_env` walk-up). `Ok(None)` when no project is found;
/// `Err` when a project exists but `package` is not listed.
pub fn resolve_app_topology(package: &str, target: Option<&str>)
    -> syn::Result<Option<ResolvedTopology>>;
```

The pass picks its source at construction:

| Constructor | Topology source |
|---|---|
| `XbinPass::from_env()` (real distributions) | `Project` — resolve at `run_pass` |
| `XbinPass::disabled()` | `FromArgs` — identity over the `#[app(cores)]` argument (default 1) |
| `XbinPass::with_manifest(...)` / `with_system(...)` (tests) | `FromArgs`, overridable with `.with_topology(AppTopology)` |

Add `pub fn with_topology(mut self, topology: AppTopology) -> Self`.

Rationale for a `Fixed` override: the pass's own tests must be able to drive a
non-identity mapping without writing a `rticx.toml` into the crate directory.

### 4.2 Injection

`parse_app_args(args, topology: Option<&AppTopology>)` in
`crates/rticx-xbin-pass/src/parse.rs` becomes:

1. parse the raw arguments into an `RticAttr`;
2. **reject** `core_ids` / `external_cores` (exact messages in §5);
3. read the optional `cores`; if present and `!= core_ids.len()`, fail (§5);
4. resolve the topology (`Fixed`/`Project`, else `AppTopology::identity(cores)`);
5. **insert** `cores = <len>` and `core_ids = [..]` into the attribute;
6. call `AppArgs::parse(attr.args_tokens())` to obtain `pacs` and let
   `rticx-core` validate `device`/`core_ids` (it already does);
7. parse `ipc_dispatchers` with the resolved `cores` (unchanged);
8. return `AppExtensions { cores, core_ids, external_cores, pacs, ipc_dispatchers }`
   and the injected argument tokens.

`parse_module`, `record_local_task`, `parse_receiver`, `validate_ipc_dispatchers`
keep using `extensions.*`; only their source changes.

### 4.3 Freshness and staleness

- `check_application` (`codegen.rs`) still compares the **resolved** topology
  (current `rticx.toml`) with the **synced** `system.json` snapshot, so editing
  `rticx.toml` without re-syncing still fails with "run `cargo xbin sync`".
- When the topology came from a project file, emit
  `const __RTICX_XBIN_PROJECT_TOML: &str = include_str!(<abs path>);` next to
  the existing system-view anchor, so rustc dep-info tracks topology edits on
  the plain-`cargo build` path.

### 4.4 Driver

`sync` / `build` keep working unchanged: cargo sets `CARGO_MANIFEST_DIR`, the
pass walks up. No new env var. `verify`'s plain-build path also works.

### 4.5 In-tree distro macros

`crates/mock/xbin-mock-distro` and
`crates/mock/fixtures/metadata/metadata-macro` currently parse `core_ids` out of
the `#[app]` args to build their backend. Both switch to
`rticx_xbin_pass::resolve_app_topology(CARGO_PKG_NAME, CARGO_BIN_NAME)`.

`rticx-stm32h7-macro` needs no behavioural change (its `physical_core` ignores
the local index); only its doc comments and the example sources change.

---

## 5. New error messages (exact)

`#[app(core_ids = …)]`:

> `` `core_ids` moved to `rticx.toml`: declare the local core index to global core id mapping in this application's `[[application]]` entry and remove `core_ids` from `#[app]` ``

`#[app(external_cores = …)]`:

> `` `external_cores` moved to `rticx.toml`: declare the foreign cores this application can see in its `[[application]]` entry and remove `external_cores` from `#[app]` ``

`#[app(cores = N)]` mismatch:

> `` `{package}` declares `cores = {n}`, but rticx.toml maps {m} core(s) via `core_ids`. In a multi-binary project `cores` is derived from `core_ids`; declare `cores` only when this application itself runs on more than one core at once (single-binary multicore, uncommon in a multi-binary project). Remove the argument or reconcile it with rticx.toml ``

Project not found but the app uses an xbin distribution → identity fallback (no
error). Project found but the package is absent:

> `` the application `{package}` is not listed in the nearest `rticx.toml` (`{path}`); add an `[[application]]` entry for it ``

---

## 6. Explicitly out of scope

- No `cores` deprecation/removal decision yet — it stays optional with an
  error on mismatch (the user will revisit it later).
- No root-workspace / published-crate change, no COMPATIBILITY generation bump.
- No change to `spawn_by` semantics, task `core` (local index), `ipc_dispatchers`
  shape, priority planning, IPC pools, or the doorbell transport.
- No project-level IPC pinning (still distro-owned, M6.9).

---

## 7. Implementation sessions

### Session A — topology moves into `rticx.toml` (manifest unchanged)

**Checkpoint A1 (additive, workspace stays green):** tasks A1 + A2 below are
purely additive; stop, run the test suite, commit, then continue with A3+.

#### A1. `rticx-xbin-proto`: `external_cores` in `[[application]]`
Files: `crates/rticx-xbin-proto/src/project.rs`,
`crates/rticx-xbin-proto/tests/project.rs`.

- Add `external_cores: Vec<u32>` to `Application` + `external_cores()` accessor.
- `parse_application`: accept the optional `external_cores` key (u32 array);
  reject duplicates; reject ids already in this application's `core_ids`
  (message names `application[i].external_cores`).
- `to_toml_string`: render `external_cores = [..]` after `core_ids` when
  non-empty (keeps the round-trip property).
- Update the module doc comment / schema example.
- Tests: presence/absence, duplicate, overlap with `core_ids`, canonical
  round-trip, unknown-key list text.

**Acceptance A1:** `cargo test -p rticx-xbin-proto` green.

#### A2. Pass topology resolver
Files: `crates/rticx-xbin-pass/src/topology.rs` (new),
`crates/rticx-xbin-pass/src/lib.rs` (declare the module + re-exports).
`project_root_from_env` / `PROJECT_MANIFEST` currently live in `lib.rs`; make
them `pub(crate)` or move them into `topology.rs` so the resolver can reuse the
walk-up.

- Implement `AppTopology`, `ResolvedTopology`, `resolve_app_topology` (§4.1).
- Re-export `AppTopology` from the crate root (distro macros use it).
- Unit tests: identity, cores(), resolution against a tempdir project
  (nearest wins), missing app error, `Ok(None)` without a project.

**Acceptance A2:** `cargo test -p rticx-xbin-pass` green (nothing wired yet).

#### A3. Pass parsing and injection
Files: `crates/rticx-xbin-pass/src/parse.rs`,
`crates/rticx-xbin-pass/src/lib.rs`.

- Change `parse_app_args` to `(args, topology: Option<&AppTopology>)` per §4.2.
- Add the three new errors (§5).
- `XbinPass` gains a `TopologySource` (`Project` / `Fixed` / `FromArgs`) and
  `.with_topology(...)`; `from_env` = `Project`, `disabled`/`with_manifest`/
  `with_system` = `FromArgs`.
- `from_env` stores `CARGO_PKG_NAME`/`CARGO_BIN_NAME` for resolution; resolve in
  `run_pass` and feed `parse_app_args`.
- Keep writing `cores`/`core_ids`/`external_cores` into the manifest in this
  session (schema 2, echoed from the resolved topology), so the proto/driver
  layer stays untouched and green.
- Update `#[cfg(test)]` tests in `lib.rs` / `parse.rs` (the old
  `core_ids = [7]` length-mismatch test is obsolete; assert the new migration
  errors instead).

#### A4. Pass freshness anchor
Files: `crates/rticx-xbin-pass/src/codegen.rs`.

- Emit the `rticx.toml` `include_str!` anchor when topology came from a project
  file (§4.3).
- Keep `check_application`; ensure its message still reads "run `cargo xbin sync`".

#### A5. Distro macros
Files: `crates/mock/xbin-mock-distro/src/lib.rs`,
`crates/mock/fixtures/metadata/metadata-macro/src/lib.rs`,
`rticx-stm32h7/rticx-stm32h7-macro/src/lib.rs` (docs only).

- Replace the `core_ids(&args)` helpers with `resolve_app_topology(...)`.
- `MockDistroBackend`/`MetadataCapabilityBackend` keep their `core_ids` field,
  populated from the resolved topology.
- The injected `__rticx_xbin_backend(<core>)` helper uses
  `topology.core_ids()[0]`.
- `MockDistroBackend`'s unit tests are unaffected.

#### A6. Fixtures and examples
Files:
- `crates/mock/fixtures/{metadata,e2e,three-app}/rticx.toml`
- `crates/mock/fixtures/{metadata,e2e,three-app}/app-*/src/main.rs`
- `rticx-stm32h7/examples-apps/rticx.toml`
- `rticx-stm32h7/examples-apps/{app-cm7,app-cm4}/src/main.rs`

- Add `external_cores` to every `[[application]]` (e2e: `[1]`/`[0]`;
  three-app: `[1,2]`/`[0,1]`/`[0,2]`; metadata: `[1]`/`[0]`; h7: `[1]`/`[0]`).
- Remove `cores`, `core_ids` and `external_cores` from every `#[app(...)]`
  (keep `ipc_dispatchers`).

#### A7. Pass/integration test migration
Files (all under `crates/rticx-xbin-pass/`): `tests/syntax.rs`,
`tests/ipc_dispatchers.rs`, `tests/metadata.rs`, `tests/core_ids.rs`,
`tests/codegen.rs`, `tests/priority_lines.rs`, `tests/receiver_compile.rs`,
`tests/init_hooks.rs`, `tests/e2e_runtime.rs`. (`src/priority.rs`'s unit tests
build `AppEntry` values directly and are expected to need no change.)

- Replace `cores = N, core_ids = […], external_cores = […]` argument fixtures
  with `.with_topology(AppTopology::new(core_ids, external_cores))` and drop
  those keys from the args.
- `tests/core_ids.rs` keeps its intent (non-identity mapping reaches the runtime
  core checks) but supplies the mapping via `.with_topology(...)`.
- Move the old `external_cores` duplicate/overlap negative tests to
  `proto/tests/project.rs`; add pass tests for the two "moved to rticx.toml"
  errors and for the `cores` mismatch error.

**Acceptance A:**
```bash
cd experiments/multibinary
cargo test -p rticx-xbin-proto -p rticx-xbin-pass
cargo test -p rticx-xbin-driver          # schema 2 still echoed; fixtures green
# optional, exercises the real driver path:
cargo install --path crates/rticx-xbin-driver
(cd crates/mock/fixtures/e2e && cargo xbin build --verify-elf)
(cd crates/mock/fixtures/three-app && cargo xbin build)
(cd rticx-stm32h7/examples-apps && cargo xbin build --verify-elf)
```
No fixture contains `core_ids`/`external_cores` in `#[app]`.

---

### Session B — manifest schema 3 (drop the echo)

#### B1. `AppManifest` schema 3
Files: `crates/rticx-xbin-proto/src/manifest.rs`,
`crates/rticx-xbin-pass/src/lib.rs` (`build_manifest`).

- `MANIFEST_SCHEMA_VERSION = 3`.
- Remove `AppManifest.cores`, `.core_ids`, `.external_cores` and their docs.
- `build_manifest` stops writing them.

#### B2. Merge uses `ProjectConfig`
Files: `crates/rticx-xbin-proto/src/merge.rs`,
`crates/rticx-xbin-proto/src/error.rs`.

- Core-id / core-count consistency: compare against `ProjectConfig` only; drop
  the `manifest.cores` / `manifest.core_ids` checks.
- External-cores existence loop reads `application.external_cores()`.
- `SpawnerCoreNotVisible` / `TargetCoreNotVisible` read the receiver's and the
  producer application's config `external_cores`.
- `AppEntry.core_ids` / `.external_cores` are copied from config.
- Remove `MergeError::CoreCountMismatch`, `CoreIdsMismatch`, `OwnExternalCore`
  (the parser now owns duplicates/disjointness).

#### B3. Update tests to schema 3
Files:
- `crates/rticx-xbin-proto/tests/{merge,alloc,system}.rs`
- `crates/rticx-xbin-driver/tests/{fixture,negative,e2e,three_app,verify}.rs`
- `crates/rticx-xbin-pass/tests/metadata.rs` (manifest assertions)
- `crates/rticx-xbin-driver/src/visualize.rs` (test fixture, if needed)

- Remove assertions on manifest `cores`/`core_ids`/`external_cores`; assert the
  same facts through `merged`/`system` instead.
- `tests/negative.rs`: the old manifest-vs-`rticx.toml` mismatch cases cannot be
  manufactured any more. Replace with:
  (a) `#[app(core_ids)]` / `#[app(external_cores)]` rejected at `sync`;
  (b) editing `rticx.toml` after `sync` then building → `check_application`
  mismatch ("run `cargo xbin sync`").
  Follow the pattern already in `tests/fixture.rs::sync_reports_a_failing_cargo_check`.

**Acceptance B:**
```bash
cd experiments/multibinary
cargo test
```
whole experimental workspace green (default members), plus the three fixture
`cargo xbin build` commands from Session A.

---

### Session C — documentation and final verification

#### C1. Reference documents
- `docs/multibinary-multicore-plan.md`:
  - §1/§4 diagram: `#[app(core_ids, external_cores)]` → `rticx.toml`;
  - §2 locked-decision rows (task syntax / new `external_cores` rule);
  - §5.2 module table (`project.rs`, `parse.rs`, new `topology.rs`);
  - §6.1 (`rticx.toml` schema), §6.4 (`#[app]` additions), §6.5 (`spawn_by`);
  - §7.1 phase-1 description (topology from `rticx.toml`);
  - §15 glossary (local/global core id, `external_cores`).
- `docs/user-guide.md`: §3 (`rticx.toml` schema table + example), §5/§6 syntax
  tables and error table, remove `#[app(core_ids)]`.
- `docs/architecture.md`: metadata/phase description.
- `README.md`: only if it references `#[app(core_ids)]`.

#### C2. Final verification
```bash
cd experiments/multibinary
cargo fmt --all -- --check
cargo test
cargo clippy --all-targets

# fixtures through the real driver
cargo install --path crates/rticx-xbin-driver
for f in crates/mock/fixtures/metadata crates/mock/fixtures/e2e crates/mock/fixtures/three-app; do
  (cd "$f" && cargo xbin sync && cargo xbin build)
done

# in-tree STM32H7 acceptance distribution
make -C rticx-stm32h7 all
```
Optional (needs Renode ≥ 1.16.1): `make -C rticx-stm32h7 renode`.

**Acceptance C:** all of the above green; no `#[app(...)]` in the tree mentions
`core_ids` or `external_cores`; `rg 'core_ids\s*=' experiments/multibinary --glob '*.rs'`
only matches the injected/parse/test-internal sites.

---

## 8. Test migration inventory

| Area | Files |
|---|---|
| Project parser | `crates/rticx-xbin-proto/tests/project.rs` |
| Merge/alloc/system | `crates/rticx-xbin-proto/tests/{merge,alloc,system}.rs` |
| Pass unit tests | `crates/rticx-xbin-pass/src/{lib,parse}.rs` |
| Pass integration | `crates/rticx-xbin-pass/tests/{syntax,ipc_dispatchers,metadata,core_ids,codegen,priority_lines,receiver_compile,init_hooks,e2e_runtime}.rs` |
| Driver | `crates/rticx-xbin-driver/tests/{fixture,negative,e2e,three_app,verify,driver,cli}.rs`, `src/visualize.rs` |
| Mock distro | `crates/mock/xbin-mock-distro/src/lib.rs` (backend tests unaffected), `crates/mock/fixtures/metadata/metadata-macro/src/lib.rs` |

---

## 9. Risks and notes

| Risk | Mitigation |
|---|---|
| Proc macro reads a project file at expansion time; rustc does not track it | `include_str!` anchor (§4.3); `cargo xbin build` cleans + syncs |
| Walk-up could find an unrelated `rticx.toml` | Every fixture/session has its own; document "nearest wins" |
| Standalone xbin distribution with no project silently becomes single-core | Accepted; documented. A wrong `cores` still errors on mismatch |
| `rticx-xbin-proto` public types change (`AppManifest`, `MergeError`) | Extension-internal today; relevant only at M7-T3 extraction |
| Existing `#[app(core_ids)]` users break | Extension is unreleased/experimental; hard error names the migration |
| `cores` remains a possible source of confusion | Kept optional per decision; error message explains single-binary multicore semantics |

---

## 10. Progress log

Status legend: `[ ]` not started · `[~]` in progress · `[x]` done · `[-]` dropped.

| # | Session | Task | Status | Notes / commit |
|---|---|---|---|---|
| A1 | A | proto `external_cores` in `[[application]]` | [ ] | |
| A2 | A | pass `topology.rs` resolver + `AppTopology` | [ ] | checkpoint A1 green before starting |
| A3 | A | `parse_app_args` injection + new errors + `TopologySource` | [ ] | |
| A4 | A | `rticx.toml` freshness anchor + `check_application` | [ ] | |
| A5 | A | mock distro + metadata-macro resolve topology | [ ] | |
| A6 | A | fixtures/examples move topology to `rticx.toml` | [ ] | |
| A7 | A | migrate pass/unit tests | [ ] | |
| A* | A | session acceptance + commit | [ ] | |
| B1 | B | `AppManifest` schema 3 (drop echo fields) | [ ] | |
| B2 | B | `merge.rs` reads `ProjectConfig`; drop dead errors | [ ] | |
| B3 | B | migrate proto/driver/pass tests to schema 3 | [ ] | |
| B* | B | session acceptance + commit | [ ] | |
| C1 | C | docs sweep (`plan`/`user-guide`/`architecture`) | [ ] | |
| C2 | C | final verification (workspace + fixtures + h7) | [ ] | |

### Session notes

- _(add entries here as sessions complete: date, what landed, anything deferred)_
