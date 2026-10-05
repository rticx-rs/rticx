# Cross-binary interface fingerprint (`interface_hash`) — implementation plan

**Status:** planned, not started.
**Scope:** `experiments/multibinary/` only (pass, proto, driver, fixtures, docs).
No root-workspace crate changes, so **no RTICX generation bump**.
**Prerequisites:** `plans/dispatchers-sharing-plan.md` and
`plans/topology-single-source-plan.md` are executed **first** (see §3).
**Target:** remove the whole-source freshness hash that red-squiggles every edit
under rust-analyzer, and replace it with a compact fingerprint over only the
cross-binary-sensitive interface of an application.

This document is split into **four implementation sessions (phases)**. Each phase
is self-contained and leaves the workspace green: it lists the failing tests to
write first (TDD), the files to touch, the acceptance commands and the commit
boundary. Load one phase per fresh session, do its checklist, tick the progress
log, commit, move on.

> **How to resume:** load this file, read the [progress log](#progress-log),
> find the first task whose status is `[ ]`, implement it, tick it, add a one-line
> note (and the commit hash when committed). Keep the workspace green at every
> **phase** boundary.

---

## 1. Motivation

Today the per-application freshness hash (`app_source_hash`, `crates/rticx-xbin-pass/src/lib.rs:694`)
canonicalizes **the entire `#[app]` module token tree plus the `#[app]` arguments**
into a `String` and FNV-hashes it. It is:

- **computed on every macro expansion** (`lib.rs:331`), *before* the module is
  parsed, cloning the whole `ItemMod` and materializing a canonical string
  proportional to the source size;
- **recorded** in `<target>.xbin.json` as `source_hash` (`lib.rs:570`,
  `manifest.rs:54`), merged into `system.json` (`merge.rs:540`, `system.rs:237`)
  and folded into `canonical_topology_text` (`system.rs:146`);
- **checked** in phase 2 by `check_source_hash` (`codegen.rs:1397`, called
  `lib.rs:414`), hard-erroring when the recomputed hash differs.

Because `system_from_env` (`lib.rs:469`) discovers `rticx.toml` from
`CARGO_MANIFEST_DIR`, rust-analyzer runs the pass in codegen mode and recomputes
this hash on **every** expansion. Editing a task body (or a doc comment, which is
an attribute token) changes the hash and produces a hard error/red squiggle even
though nothing cross-binary changed.

**Goal.** The hash must depend only on the parts of an application that the
driver merges into the project topology:

- the resolved project topology the application participates in
  (`cores` / `core_ids` / `external_cores`, resolved from `rticx.toml`);
- the **cross-binary dispatcher entries** the xbin pass steals from
  `dispatchers` (order + IRQ identity);
- the **cross-binary receiver declarations**: `name`, `priority`, `capacity`,
  `core`, `spawn_by`, `input_type`;
- the IDL canonical layout (`ipc-types.toml`, via `layout_hash`).

Everything else — task bodies, comments, local sw/async task declarations and
their priorities, `shared` lists, `device`/`pacs`, surplus (local) dispatcher
entries, formatting — must change freely without a freshness error.

**Non-goals.** No runtime, layout, allocation or dispatch-chain change. No change
to the dispatcher-sharing or topology-single-source models. No project-level IPC
pinning.

## 2. Locked decisions (from the design review)

| Topic | Decision |
|---|---|
| Hash strategy | **Narrow** the per-app hash to an interface fingerprint; keep an early "run `cargo xbin sync`" error, stop tripping on unrelated code. |
| Fingerprint contents | Resolved topology (`cores`, `core_ids`, `external_cores`) + the **stolen cross-dispatcher prefix** per local core + receiver declarations (`name`, `priority`, `capacity`, `core`, `spawn_by`, `input_type`). Nothing else. |
| Dispatchers | Only the entries xbin steals matter. Surplus/local entries and their IRQ identity do **not** enter the fingerprint. (If, when implementing, isolating the prefix is awkward, including the whole `dispatchers` list is an accepted simplification — but prefer the prefix.) |
| Local task priorities | **Excluded.** Phase 2 already re-validates `(target core, priority)` collisions from the current source (`priority.rs` / `validate_priority_lines`). |
| `shared`, `device`/`pacs` | **Excluded.** Recomputed in phase 2 from the current source; no cross-binary layout effect. |
| `rticx.toml` | Handled by `topology-single-source-plan.md` (`check_application` + `include_str!(rticx.toml)` anchor). This plan does not re-hash it. |
| IDL freshness | **Add a check.** In codegen mode the pass parses `ipc-types.toml`, recomputes the layout hash and hard-errors on a mismatch with `system.json`. An `include_str!(ipc-types.toml)` anchor makes editing the IDL re-expand the macro (the existing anchor only tracks the generated `ipc_types.rs`). |
| JSON field name | Rename `source_hash` → `interface_hash` in `AppManifest` and `AppEntry`. |
| Schema versions | `MANIFEST_SCHEMA_VERSION` **3 → 4**, `SYSTEM_SCHEMA_VERSION` **2 → 3**. (`RTICX_GENERATION` stays `0.2`.) |
| Computation site | Both cases (phase 1 metadata, phase 2 codegen) compute the fingerprint from the already-parsed data, after the cross-dispatcher steal; identical inputs → identical fingerprint. |
| Compiler-independence | The fingerprint is built from parsed, canonicalized data, so the whole-module `normalize_semicolons`/`write_canonical` hack is deleted. `write_canonical` is kept only to render dispatcher `Path`s host-independently. |
| Safety net | Phase 2 keeps its targeted checks: `check_application` (topology), `resolve_receiver` (per field), owned-view-task-without-declaration, dispatcher count. The fingerprint is an early clear-message guard, not the only guard. |

## 3. Assumed starting state (prerequisites)

This plan is written against the tree **after** `dispatchers-sharing-plan.md`
and `topology-single-source-plan.md` have landed. Before starting Phase 1,
verify all of the following; if any is false, stop and finish the prerequisite
first.

- [ ] `XbinPass::run_pass` no longer knows `ipc_dispatchers`; it parses
      `dispatchers`, steals the first `K = lines(c).len()` entries per local core
      into `extensions.cross_dispatchers`, and re-emits the remainder into the
      args for `rticx-sw-pass`/`rticx-async-pass` (`parse.rs::steal_dispatchers`).
- [ ] The pass resolves `cores` / `core_ids` / `external_cores` from `rticx.toml`
      (new `crates/rticx-xbin-pass/src/topology.rs`, `AppTopology`,
      `resolve_app_topology`), and `check_application` compares the resolved
      topology against the synced `system.json`; a `include_str!(rticx.toml)`
      dep-info anchor is emitted.
- [ ] `#[app(core_ids)]` / `#[app(external_cores)]` are hard errors pointing at
      `rticx.toml`.
- [ ] `MANIFEST_SCHEMA_VERSION == 3` (topology plan, Session B); `AppManifest` no
      longer carries `cores`/`core_ids`/`external_cores`; `AppEntry` in
      `system.json` still does.

Quick check:

```bash
cd experiments/multibinary
rg -n "ipc_dispatchers" crates/rticx-xbin-pass/src        # expect none
rg -n "MANIFEST_SCHEMA_VERSION" crates/rticx-xbin-proto/src/manifest.rs
rg -n "fn steal_dispatchers|AppTopology|resolve_app_topology" crates/rticx-xbin-pass/src
```

If only `dispatchers-sharing-plan` landed but not `topology-single-source-plan`
(or vice versa), the deltas are small and noted inline; the fingerprint always
consumes whatever the pre-existing `AppExtensions` already exposes
(`extensions.core_ids`, `extensions.external_cores`, and either
`extensions.cross_dispatchers` or `extensions.ipc_dispatchers`).

---

## 4. Design

### 4.1 The fingerprint

```
xbin-interface v1
cores=<u32>
core_ids=[g0, g1, ...]
external_cores=[g...]
dispatcher core=<local> entry=<canonical path>
...
receiver name=<ident> priority=<u16> capacity=<usize> core=<local> spawn_by=<global> input=<type>
...
```

- One `dispatcher` line per stolen entry, in the order the pass bound them
  (ascending `(source, priority)` per local core, cores in order). The `Path` is
  rendered with the existing `write_canonical` so punctuation `Spacing` cannot
  change the hash (`Path::to_token_stream()`), exactly like the current
  `path_key`.
- One `receiver` line per declaration; `ModuleDecls.receivers` is already sorted
  by name, so the rendering is deterministic.
- The leading `v1` tag lets a future format change be detected explicitly; it is
  belt-and-braces on top of the schema-version gate.

Explicitly **not** in the text: module items, task bodies, doc/`//` comments,
local `#[sw_task]`/`#[async_task]` declarations, `shared`, `device`/`pacs`,
surplus dispatcher entries, `#[app]` key order/formatting.

### 4.2 Where it is computed

Replace the unconditional `app_source_hash(&args, &app_mod)` at `lib.rs:331`
with a call after the module has been parsed (and the dispatchers stolen):

```rust
// run_pass, after parse_app_args + parse_module + steal_dispatchers:
let interface_hash = matches!((&self.meta, &self.system), (Some(_), _) | (_, Some(_)))
    .then(|| interface_hash(&extensions, &decls));
```

- `meta` mode passes it to `build_manifest` (today `source_hash`); `system` mode
  passes it to `check_interface_hash`.
- `XbinPass::disabled()` never uses it → no parsing/hash work at all.
- `pub(crate) fn interface_hash(extensions: &AppExtensions, decls: &ModuleDecls) -> Hash64`
  builds the canonical text and returns `Hash64::of(text.as_bytes())`.
- Keep a public wrapper so tests/tooling can build a view that matches a given
  source, exactly like `app_source_hash` today (see §4.4 tests):
  ```rust
  /// Parses `(args, app_mod)` with the given topology and returns the
  /// fingerprint `run_pass` would compute.
  pub fn app_interface_hash(
      args: &TokenStream,
      app_mod: &ItemMod,
      topology: Option<&AppTopology>,
  ) -> syn::Result<Hash64>;
  ```
  Implement it by factoring the parse sequence
  (`parse_app_args` → `parse_module` → `steal_dispatchers`) into one helper used
  by both `run_pass` and the wrapper; if that refactor is too invasive, the
  wrapper may re-run the three calls on clones.
- Delete `normalize_semicolons`, `is_block_like`, and the `visit-mut` syn feature
  (the only user is `normalize_semicolons`; see
  `crates/rticx-xbin-pass/Cargo.toml:19-21`). Keep `write_canonical` for the
  dispatcher paths.

### 4.3 IDL freshness check

Today editing `ipc-types.toml` after a sync is invisible to a plain
`cargo build`: the synced `ipc_types.rs`, `system.json` and the receiver's
`input_type` stay mutually consistent, so nothing fires until the next
`cargo xbin sync`.

Add, in codegen mode, next to the interface check:

```rust
// Reason about the project root the topology resolver already found.
let idl_path = project_root.join(IDL_FILE);            // "ipc-types.toml"
let idl = if idl_path.is_file() { parse_idl_file(&idl_path)? } else { IpcTypes::empty() };
check_idl_hash(&view, layout_hash(&idl)?)?;            // compare Hash64::new(live) with view.layout_hash
```

- New `pub const IDL_FILE: &str = "ipc-types.toml";` in `rticx-xbin-proto`
  (single owner); have the driver's `IDL_MANIFEST` (`driver/src/project.rs:9`)
  re-export it so the two can never disagree (as `IPC_TYPES_FILE` already does).
- `check_idl_hash` error:
  > ``the IDL `ipc-types.toml` changed since the last `cargo xbin sync` (the system view records layout hash …, the current IDL hashes to …); run `cargo xbin sync` ``
- **Dep-info anchor.** Emit, when the file exists,
  `const __RTICX_XBIN_IDL: &str = include_str!(<abs ipc-types.toml>);`
  in `generate_freshness_items` (reuse `include_str_path_lit`). Without it, a
  cargo-cached build would not re-run the macro after a pure IDL edit and the
  check would never fire.
- The comparison uses `IpcTypes::empty()` when the file is absent, so the chosen
  sentinel matches `system_view`'s (`alloc.rs:40` computes `layout_hash` from the
  parsed/empty IDL, not `Hash64::ZERO`).

### 4.4 Rename + schemas

| Item | Before | After |
|---|---|---|
| `AppManifest` field | `source_hash` | `interface_hash` |
| `AppEntry` field | `source_hash` | `interface_hash` |
| `canonical_topology_text` line | `source_hash=` | `interface_hash=` |
| `MANIFEST_SCHEMA_VERSION` | 3 | 4 |
| `SYSTEM_SCHEMA_VERSION` | 2 | 3 |
| `codegen::check_source_hash` | `check_source_hash` | `check_interface_hash` |
| error text | "changed since the last `cargo xbin sync` (the system view records source hash …)" | "interface changed since the last `cargo xbin sync` (the system view records interface hash …, the source hashes to …)" |

---

## 5. Phases

### Phase 1 — rename `source_hash` → `interface_hash` and bump schemas (behavior unchanged)

**Goal:** mechanical rename + schema bump, with the *existing* whole-source
computation untouched. Keeps Phase 2 focused on the semantic change and makes its
diff reviewable.

**Files**

- `crates/rticx-xbin-proto/src/manifest.rs` (`AppManifest.source_hash` → `interface_hash`,
  `MANIFEST_SCHEMA_VERSION = 4`, module doc).
- `crates/rticx-xbin-proto/src/system.rs` (`AppEntry.source_hash` → `interface_hash`,
  `SYSTEM_SCHEMA_VERSION = 3`, `canonical_topology_text`).
- `crates/rticx-xbin-proto/src/merge.rs:540`.
- `crates/rticx-xbin-proto/src/hash.rs` (module doc: `source_hash` → `interface_hash`).
- `crates/rticx-xbin-pass/src/lib.rs` (`build_manifest` field write).
- `crates/rticx-xbin-pass/src/codegen.rs` (`check_source_hash` → `check_interface_hash`
  and its doc; the freshness-anchor doc comments).
- All test fixtures with a literal `"source_hash":` or `.source_hash`
  (`crates/rticx-xbin-pass/tests/{codegen,init_hooks,receiver_compile,priority_lines,e2e_runtime,metadata}.rs`,
  `crates/rticx-xbin-driver/tests/*` as applicable) — `rg -n "source_hash" crates` is the
  checklist.

**TDD steps**

1. Write a failing test (proto or pass) asserting
   `MANIFEST_SCHEMA_VERSION == 4`, `SYSTEM_SCHEMA_VERSION == 3`, and that a
   produced manifest/`system.json` contains `"interface_hash"` and not
   `"source_hash"`. Run → red.
2. Rename the fields/constants/messages and migrate every fixture.
3. `rg -n "source_hash" crates` must only match the (new) `app_source_hash`
   function name and its doc — those are renamed in Phase 2.

**Acceptance**

```bash
cd experiments/multibinary
cargo test -p rticx-xbin-proto -p rticx-xbin-pass -p rticx-xbin-driver
```

**Commit:** `xbin: rename the freshness hash to interface_hash (schema 3/4)`

---

### Phase 2 — narrow the hash to the xbin interface fingerprint

**Goal:** the fingerprint no longer changes on bodies, comments, local tasks,
`shared`, `device`/`pacs` or surplus dispatchers; it still changes on every
cross-binary-sensitive field. This is the fix for the rust-analyzer squiggles.

**Files**

- `crates/rticx-xbin-pass/src/lib.rs` — new `interface_hash`, public
  `app_interface_hash`, delete `normalize_semicolons`/`is_block_like`, compute
  after parse, only when meta/system mode is active.
- `crates/rticx-xbin-pass/src/codegen.rs` — `check_interface_hash` message/doc.
- `crates/rticx-xbin-pass/Cargo.toml` — drop the `visit-mut` syn feature.
- `crates/rticx-xbin-pass/src/parse.rs` — expose what the fingerprint needs via
  `AppExtensions`/`ModuleDecls` (already public to the crate).

**TDD steps (pass crate tests, `crates/rticx-xbin-pass/tests/codegen.rs` unless noted)**

Write these failing tests first:

1. `interface_hash_ignores_task_bodies_and_doc_comments` — two `#[app]` modules
   with identical receiver declarations but different `exec` bodies and doc
   comments hash equal.
2. `interface_hash_ignores_local_task_declarations` — adding/changing a local
   `#[sw_task]` (different priority) leaves the hash unchanged.
3. `interface_hash_ignores_shared_and_device` — changing a receiver's `shared`
   list and the `device`/`pacs` leave the hash unchanged.
4. `interface_hash_changes_on_receiver_field` — a table test that changing each
   of `name`, `priority`, `capacity`, `core`, `spawn_by`, `input_type` changes
   the hash.
5. `interface_hash_changes_on_stolen_dispatcher_and_ignores_surplus` — moving an
   IRQ in the stolen cross prefix changes the hash; changing/adding a surplus
   local dispatcher entry does not.
6. `interface_hash_changes_on_topology` — a different `core_ids` /
   `external_cores` changes the hash.
7. `interface_hash_ignores_token_spacing` — keep the spirit of the old
   `source_hash_ignores_token_spacing` test, now over dispatcher paths
   (`Punct` `Spacing::Alone` vs `Joint`).
8. Replace the old `source_hash_ignores_ra_synthetic_statement_semicolons` test:
   the semicolon normalization is deleted, so delete the test and rely on (1)
   (a synthetic statement semicolon lives in a body, which is ignored).

Then implement the fingerprint and delete the normalizer. Migrate the helpers in
`tests/codegen.rs` (`with_source_hashes` → `with_interface_hashes`) and all patch
call sites to the new public wrapper.

**Driver regression tests** — add `crates/rticx-xbin-driver/tests/freshness.rs`
over a copy of `crates/mock/fixtures/e2e` (reuse the `fixture_copy` helper from
`negative.rs`, or factor it into a shared test module):

1. `plain_build_accepts_a_body_only_edit` — sync, edit a receiver task body
   (and add a doc comment), plain `cargo build` **succeeds**. This is the user's
   original complaint; it must pass.
2. `plain_build_rejects_a_source_interface_edit` — sync, change a receiver
   `priority` (without re-syncing), plain build **fails** with
   "interface changed since the last `cargo xbin sync`".
3. Update `crates/rticx-xbin-driver/tests/negative.rs::plain_build_rejects_a_missing_or_stale_sync`:
   its current "source changed" edit injects `const __XBIN_NEGATIVE: u32 = 1;`,
   which is **not** interface-relevant any more. Change the injected edit to a
   receiver interface change and update the asserted message; move the
   body-edit positive case to `freshness.rs`.

**Acceptance**

```bash
cd experiments/multibinary
cargo test -p rticx-xbin-pass
cargo test -p rticx-xbin-driver
```

**Commit:** `xbin: hash only the cross-binary interface for freshness`

---

### Phase 3 — IDL (`ipc-types.toml`) freshness check

**Goal:** editing the IDL after a sync is a hard error on a plain build, and the
macro actually re-runs when the IDL changes.

**Files**

- `crates/rticx-xbin-proto/src/lib.rs` — `pub const IDL_FILE: &str = "ipc-types.toml";`
  (and re-export); update `crates/rticx-xbin-driver/src/project.rs:9` to
  re-export it instead of defining its own literal.
- `crates/rticx-xbin-pass/src/codegen.rs` — `check_idl_hash` + the
  `__RTICX_XBIN_IDL` anchor in `generate_freshness_items`.
- `crates/rticx-xbin-pass/src/lib.rs` — wire the check into the codegen branch,
  using the project root the topology resolver already found.

**TDD steps**

1. `crates/rticx-xbin-pass/tests/codegen.rs`: unit tests for `check_idl_hash` —
   matching layout hash passes; a different `view.layout_hash` errors with the
   exact message; the no-`ipc-types.toml` case compares against
   `IpcTypes::empty()`.
2. `crates/rticx-xbin-pass/tests/codegen.rs`: a snapshot test that codegen
   emits `__RTICX_XBIN_IDL` with `include_str!` when the IDL exists, and emits
   nothing when it does not.
3. `crates/rticx-xbin-driver/tests/freshness.rs`:
   `plain_build_rejects_an_edited_idl` — sync, edit `ipc-types.toml` (e.g. add a
   field or change `u32` → `u16`), plain build **fails** with
   "the IDL `ipc-types.toml` changed since the last `cargo xbin sync`" and
   "run `cargo xbin sync`". (This also proves the anchor fires, because a cached
   build would otherwise not re-expand.)

**Acceptance**

```bash
cd experiments/multibinary
cargo test -p rticx-xbin-proto -p rticx-xbin-pass -p rticx-xbin-driver
```

**Commit:** `xbin: reject a stale IDL on a plain cargo build`

---

### Phase 4 — documentation and final verification

**Goal:** all reference docs describe the interface fingerprint and the IDL
check; full workspace + fixtures green.

**Files**

- `plans/multibinary-multicore-plan.md` — §7.1 (phase-1 manifest fields),
  §7.3 (freshness/staleness), §14 (risk rows on staleness) and the glossary
  where `source_hash` is named; add the IDL freshness rule.
- `docs/architecture.md` — lines around 52, 88, 135–141, 187, 204–207, 417, 441:
  replace "source hash / source changed" with the interface fingerprint and the
  IDL check.
- `docs/user-guide.md` — lines around 509 (`m7.xbin.json` comment), 644–647
  (freshness narrative), 905–911 (error table): document what does and does not
  trigger a re-sync, and the new IDL error row.
- `crates/rticx-xbin-proto/src/hash.rs` and the `manifest.rs`/`system.rs`
  field docs (if not already swept in Phase 1/2).

**Steps**

1. `rg -n "source_hash|source hash|whole-source|normalize_semicolons" docs plans crates` —
   every remaining match is either a deliberate historical note or a bug.
2. Update the reference plan's status line to record this change.
3. Run the full verification below.

**Acceptance**

```bash
cd experiments/multibinary
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test

# real driver path (needs `cargo install --path crates/rticx-xbin-driver` or `cargo xbin`)
for f in crates/mock/fixtures/metadata crates/mock/fixtures/e2e crates/mock/fixtures/three-app; do
  (cd "$f" && cargo xbin sync && cargo xbin build)
done

# in-tree STM32H7 distribution
make -C rticx-stm32h7 all
```

Manual end-to-end sanity check (the actual acceptance for this work):

```bash
cd crates/mock/fixtures/e2e
cargo xbin sync
# 1. body-only edit: append a line/comment inside a receiver's exec -> plain build must pass
cargo build --package app-m4 --bin m4
# 2. interface edit: change a receiver `priority` -> plain build must fail, naming cargo xbin sync
# 3. IDL edit: change a field type in ipc-types.toml -> plain build must fail,
#    naming the IDL and cargo xbin sync
```

**Commit:** `xbin: document the interface fingerprint and IDL freshness`

---

## 6. Progress log

Status legend: `[ ]` not started · `[~]` in progress · `[x]` done · `[-]` dropped.

| # | Phase | Task | Status | Notes / commit |
|---|---|---|---|---|
| P0 | — | Verify prerequisites (§3) | [ ] | stop if not landed |
| P1-T1 | 1 | Failing schema/rename test | [ ] | |
| P1-T2 | 1 | Rename `source_hash` → `interface_hash`, bump schemas, migrate fixtures | [ ] | |
| P1-* | 1 | Phase acceptance + commit | [ ] | |
| P2-T1 | 2 | Failing fingerprint-scope tests | [ ] | bodies/comments/local/shared/device |
| P2-T2 | 2 | Failing fingerprint-sensitivity tests | [ ] | receiver fields/dispatchers/topology |
| P2-T3 | 2 | Implement `interface_hash`, delete normalizer, drop `visit-mut` | [ ] | |
| P2-T4 | 2 | Migrate `with_interface_hashes` + patch call sites | [ ] | |
| P2-T5 | 2 | Driver `freshness.rs`: body-edit passes, interface-edit fails | [ ] | key acceptance |
| P2-T6 | 2 | Update `negative.rs` source-changed case | [ ] | |
| P2-* | 2 | Phase acceptance + commit | [ ] | |
| P3-T1 | 3 | `check_idl_hash` unit tests + snapshot | [ ] | |
| P3-T2 | 3 | Implement IDL check + `include_str!` anchor + `IDL_FILE` | [ ] | |
| P3-T3 | 3 | Driver `plain_build_rejects_an_edited_idl` | [ ] | proves the anchor fires |
| P3-* | 3 | Phase acceptance + commit | [ ] | |
| P4-T1 | 4 | Docs sweep (`plan`/`architecture`/`user-guide`/proto docs) | [ ] | |
| P4-T2 | 4 | Full verification + manual matrix | [ ] | |
| P4-* | 4 | Phase acceptance + commit | [ ] | |

### Session notes

- _(add entries here as phases complete: date, what landed, deviations, deferred items)_

---

## 7. Risks and notes

| Risk | Mitigation |
|---|---|
| The IDL check re-parses `ipc-types.toml` on every expansion | The file is small; the check only runs in codegen mode and only when a project root exists. |
| A cargo-cached build does not re-run the macro after an IDL edit | Emit `include_str!(ipc-types.toml)`; the `plain_build_rejects_an_edited_idl` test fails without it. |
| Fingerprint misses a still-relevant change | Phase 2 keeps `check_application`, `resolve_receiver`, the owned-task check and the dispatcher count as the safety net; the fingerprint only adds an early, precise message. |
| Cross-plan coupling (`AppTopology`, `cross_dispatchers`) | §3 lists the exact assumed APIs; the fingerprint consumes whatever `AppExtensions` already exposes, so a missing prerequisite degrades to a one-line delta rather than a rewrite. |
| `system.json` is rewritten on every `sync` even when unchanged | Optional follow-up: switch the `system.json` write to `write_if_changed` (as `ipc_types.rs` already does) so an unrelated edit produces a byte-identical view and stable mtime. Not required for this plan. |
| Stale `system.json` from the old full-source hash during development | Schema bumps (`3/4` and `2/3`) plus the topology hash force a re-sync; the documented error already says "run `cargo xbin sync`". |

## 8. Out of scope

- Async cross-binary tasks; runtime/layout/allocation changes.
- Any change to the dispatcher-sharing or topology-single-source models.
- Project-level IPC pinning.
- A `cargo xbin`/rust-analyzer integration beyond the fingerprint and anchors
  (e.g. auto-syncing the IDE).
