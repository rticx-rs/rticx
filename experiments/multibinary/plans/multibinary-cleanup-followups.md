# Multibinary release-cleanup follow-ups

## Goal

Finish the release cleanup of the `experiments/multibinary` workspace that was
started in `crates/rticx-xbin-pass`:

1. remove the duplicated integration-test scaffolding from
   `crates/rticx-xbin-pass/tests/`;
2. hoist the triplicated `tempfile` dev-dependency into
   `[workspace.dependencies]`;
3. remove the remaining planning/milestone references from the **user-facing**
   docs `docs/architecture.md` and `docs/user-guide.md` (Phase 3);
4. unify the duplicated hex parsers in `rticx-xbin-proto` (Phase 4);
5. return an iterator from `FieldType::named_types` (Phase 5);
6. simplify the cycle check in `IpcTypes::visit` (Phase 6).

## Context

`experiments/multibinary/` is a **standalone cargo workspace** (its own
`[workspace]`, root `Cargo.toml` at `experiments/multibinary/Cargo.toml`). It is
separate from the root `rtic-mc-experiments` workspace.

What is already done (previous session, uncommitted at the time of writing):

- All milestone (`M5.5`, `M6.5-T3`, `M6.9-T4`, `M7-T2`, …), plan-section
  (`Plan §4/§8`, `plan §7.3`) and design-decision (`D1`–`D5`) references were
  removed from `crates/rticx-xbin-pass/src/` and
  `crates/rticx-xbin-pass/tests/`, with the prose rewritten where the reference
  was load-bearing.
- `rticx-xbin-pass` source was simplified: one `generate_fifo_view` helper,
  shared `item_name`, one `crate::error` constructor, no `Option`/`expect`
  fingerprint dance in `run_pass`, single env read in `from_env`.
- Verified green: `cargo fmt -p rticx-xbin-pass -- --check`,
  `cargo clippy -p rticx-xbin-pass --all-targets`,
  `cargo test -p rticx-xbin-pass`,
  `cargo check --workspace --exclude rticx-stm32h7`.

`crates/rticx-xbin-proto` was cleaned in the same spirit (this session,
uncommitted):

- all milestone/plan references were removed from its `src/` and `tests/`
  (`M1-T1`, `M5.5`, `M6.9-T2/T4/T5`, `M7-T2`, `M0-T2/T3/T4`, `§6.1/§7.2`, …),
  with the prose reworded where the tag was load-bearing;
- `codegen.rs` now uses the shared `Hash64` and the `FieldType` `Display` impl
  instead of local `fnv1a64`/`render_field_type` copies, and `layout.rs` cycle
  detection dropped the parallel `visiting` set;
- verified green: `cargo fmt -p rticx-xbin-proto -- --check`,
  `cargo clippy -p rticx-xbin-proto --all-targets`,
  `cargo test -p rticx-xbin-proto`.

The remaining milestone references live in:

| File | `M#` | `D#` | `§` | Treatment |
|---|---|---|---|---|
| `docs/architecture.md` | 116 | 8 | 1 | clean (user-facing) |
| `docs/user-guide.md` | 32 | 3 | 3 | clean (user-facing) |


Note: the `D1`–`D5` tags found in `docs/*.md` are currently **undefined in the
tree** — the document that introduced them is not present in git history either
(searched `plans/` and history). Treat them as dangling and drop them from the
user-facing docs; see Phase 3.

Other workspace crates (`crates/rticx-xbin-driver`, `crates/mock/*`,
`rticx-stm32h7/*`, fixture apps) still carry milestone references too.

### Commands used in this workspace

```bash
cd experiments/multibinary

# the crate under test
cargo test -p rticx-xbin-pass
cargo clippy -p rticx-xbin-pass --all-targets
cargo fmt -p rticx-xbin-pass -- --check

# workspace-wide (rticx-stm32h7 cannot be built for the host; it needs exactly
# one of the `cm7`/`cm4` features, so exclude it)
cargo check --workspace --exclude rticx-stm32h7
cargo test -p rticx-xbin-proto -p rticx-xbin-driver
```

`rticx-xbin-pass` integration tests spawn real `cargo` builds of throwaway
projects, so the first `cargo test -p rticx-xbin-pass` after a change is slow
(compile of fixtures); subsequent runs are cached.

## Definition of done

- [ ] `cargo test -p rticx-xbin-pass` passes with the shared test-support
      module in place and the duplicated helpers deleted.
- [ ] `cargo clippy -p rticx-xbin-pass --all-targets` and
      `cargo fmt -p rticx-xbin-pass -- --check` are clean.
- [ ] `tempfile` is declared once in `[workspace.dependencies]` and consumed
      via `tempfile = { workspace = true }` in the three crates.
- [ ] `cargo check --workspace --exclude rticx-stm32h7` and
      `cargo test -p rticx-xbin-proto -p rticx-xbin-driver` pass.
- [ ] No `M#`/`D#`/plan-section reference remains in
      `docs/architecture.md` and `docs/user-guide.md`; intra-doc section links
      still resolve.
- [ ] The `rticx-xbin-proto` hex parsing is shared, `FieldType::named_types`
      returns an iterator, and `IpcTypes::visit` computes the cycle start once;
      `cargo test -p rticx-xbin-proto` passes with clippy/fmt clean.

---

## Phase 1 — Shared test-support module for `rticx-xbin-pass`

**Objective:** move the byte-identical integration-test scaffolding into
`crates/rticx-xbin-pass/tests/support/mod.rs` without changing test behavior.

**Background:** every file in `crates/rticx-xbin-pass/tests/` is a separate
test crate. A subdirectory (`tests/support/mod.rs`) is *not* a test target, so it
can be shared by declaring `mod support;` in each test file. Add
`#![allow(dead_code)]` at the top of `support/mod.rs`, because each test crate
only uses a subset of the helpers and would otherwise emit `dead_code` warnings.

Only move helpers that are genuinely identical or trivially unifiable. The mock
backends are **not** identical (they differ in the mock accessor name
`__mock_xbin_backend` vs `__rticx_xbin_backend`, whether they implement
`configure_shared_memory`/`ipc_base_override`/`custom_interrupt_path`, and the
router ident) — leave them in their files.

**Files:**
- `crates/rticx-xbin-pass/tests/support/mod.rs` — new, holds the shared helpers.
- `crates/rticx-xbin-pass/tests/codegen.rs` — delete local `ENV_LOCK`/`EnvGuard`.
- `crates/rticx-xbin-pass/tests/metadata.rs` — delete local `ENV_LOCK` and the
  function-local `EnvGuard`.
- `crates/rticx-xbin-pass/tests/receiver_compile.rs` — delete local `cargo()`
  and the rt/mock path resolution inside `write_project`.
- `crates/rticx-xbin-pass/tests/init_hooks.rs` — delete local `cargo()` and the
  rt/mock path resolution inside `write_project`.
- `crates/rticx-xbin-pass/tests/e2e_runtime.rs` — delete local `cargo()`.

**Helpers to move (in this order):**

1. **`ENV_LOCK` + `EnvGuard`.** `codegen.rs` has
   `EnvGuard::{set_all, unset_all}` (stores `Vec<(&'static str, Option<OsString>)>`);
   `metadata.rs` has a function-local `EnvGuard::{set, set_all}` (String-based).
   Unify into one type in `support/mod.rs` with `set(name, value)`,
   `set_all(&[(&str, &OsStr)])` and `unset_all(&[&str])`, keeping the existing
   `// SAFETY: the process environment is guarded by ENV_LOCK` comments and the
   save/restore-on-`Drop` behavior. Both files then use
   `support::{EnvGuard, ENV_LOCK}`.
2. **`cargo() -> PathBuf`.** Identical in `receiver_compile.rs`,
   `init_hooks.rs`, `e2e_runtime.rs`:
   ```rust
   std::env::var_os("CARGO").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("cargo"))
   ```
3. **rt/mock workspace paths.** `receiver_compile.rs::write_project` and
   `init_hooks.rs::write_project` both compute the canonicalized
   `../rticx-xbin-rt` and `../mock/rticx-xbin-mock` paths from
   `env!("CARGO_MANIFEST_DIR")`. Factor into
   `support::runtime_and_mock_paths() -> (PathBuf, PathBuf)`. Keep the two
   `write_project` functions (their package names and generated `main.rs`/`lib.rs`
   contents differ); only share the path resolution.

**Steps (test-first, refactor style):**
1. Baseline: run `cargo test -p rticx-xbin-pass`; expect all green. Record the
   pass count. (This is the regression guard for the whole phase; there is no
   new behavior to test.)
2. Add `tests/support/mod.rs` with `#![allow(dead_code)]` and the `EnvGuard` /
   `ENV_LOCK` / `cargo()` helpers; add `mod support;` + imports to the five test
   files. Delete the moved copies. Run `cargo test -p rticx-xbin-pass`; expect
   GREEN with the same pass count.
3. Add `runtime_and_mock_paths()`; rewrite the two `write_project` bodies to use
   it. Run `cargo test -p rticx-xbin-pass`; expect GREEN.
4. Refactor pass: `cargo clippy -p rticx-xbin-pass --all-targets` (no new
   warnings) and `cargo fmt -p rticx-xbin-pass -- --check`.

**Exit criteria / verification:**
- [ ] `cargo test -p rticx-xbin-pass` passes with the same number of tests as
      the baseline.
- [ ] `grep -rn "struct EnvGuard" crates/rticx-xbin-pass/tests` matches only
      `support/mod.rs`.
- [ ] `grep -rn "fn cargo()" crates/rticx-xbin-pass/tests` matches only
      `support/mod.rs`.
- [ ] `cargo clippy -p rticx-xbin-pass --all-targets` and
      `cargo fmt -p rticx-xbin-pass -- --check` are clean.

---

## Phase 2 — Hoist `tempfile` into `[workspace.dependencies]`

**Objective:** declare `tempfile` once in the workspace and consume it from the
three crates that use it.

**Files:**
- `Cargo.toml` (workspace root of `experiments/multibinary`) — add
  `tempfile = "3.27.0"` under `[workspace.dependencies]`, in the
  `# --- external ---` block next to `thiserror`/`toml`.
- `crates/rticx-xbin-driver/Cargo.toml` — `tempfile = "3.27.0"` →
  `tempfile = { workspace = true }`.
- `crates/rticx-xbin-pass/Cargo.toml` — same.
- `crates/rticx-xbin-proto/Cargo.toml` — same.

All three are `[dev-dependencies]` (verified), so the workspace entry is enough;
`workspace = true` is valid from a `[dev-dependencies]` table.

**Steps (config refactor; the existing suites are the regression guard):**
1. Baseline: `cargo test -p rticx-xbin-proto -p rticx-xbin-driver -p rticx-xbin-pass`;
   expect GREEN.
2. Add the workspace entry and switch the three manifests.
3. Run `cargo check --workspace --exclude rticx-stm32h7` and the baseline test
   command again; expect GREEN. `Cargo.lock` should be unchanged (same
   requirement); if it changes, stop and investigate.

**Exit criteria / verification:**
- [ ] `grep -rn 'tempfile' crates/*/Cargo.toml` shows only
      `tempfile = { workspace = true }`.
- [ ] `cargo check --workspace --exclude rticx-stm32h7` passes.
- [ ] `cargo test -p rticx-xbin-proto -p rticx-xbin-driver -p rticx-xbin-pass`
      passes.
- [ ] `git diff Cargo.lock` is empty.

---

## Phase 3 — Remove planning references from docs; retire the plan document

**Objective:** the user-facing docs describe the shipped system without
milestone/decision tags, and the internal plan document no longer ships as-is.

**Files:**
- `docs/architecture.md`
- `docs/user-guide.md`

**What "planning reference" means here:** milestone ids (`M5.5`, `M6.9-T4`,
`M7-T2`, …), design-decision ids (`D1`–`D5`), and references to the plan
document's own sections (`plan §7.2`, `[plan §10](...)`). Keep genuine
cross-document section links between the two docs, but replace the bare `§N`
link text with a descriptive label (e.g. `[plan §10](…)` → `[the
distribution/backend contract](…)`).

**Steps:**
1. Baseline: `grep -rnoE '\bM[0-9]+(\.[0-9]+)?(-T[0-9]+)?|\bD[0-9]\b|§[0-9.]+' docs plans`
   and record the counts (table above).
2. `docs/architecture.md`: strip the parenthetical `M#`/`D#` tags and reword the
   sentence-level references (the same treatment already applied to
   `rticx-xbin-pass`). Fix the one `plan §` link. Verify every markdown link
   target (`#...` anchor) still exists.
3. `docs/user-guide.md`: same. Pay attention to the three `§` links
   (`[architecture §7]`, `[architecture §8]`, `[plan §10]`) — keep the links,
   make the text descriptive.
4. Re-run the grep from step 1; expect zero `M#`/`D#` in the two docs (the plan
   doc may be gone entirely).
5. Markdown link check: `grep -rnoE '\]\([^)]*#[^)]*\)' docs` and confirm each
   target anchor exists in the referenced file.

**Exit criteria / verification:**
- [ ] `grep -rnoE '\bM[0-9]+(\.[0-9]+)?(-T[0-9]+)?|\bD[0-9]\b' docs/architecture.md docs/user-guide.md`
      returns nothing.
- [ ] Every intra-doc `#anchor` link in `docs/*.md` resolves (no dead links).
- [ ] The plan-document decision is applied and noted in the progress log below.
- [ ] No code build is affected (docs-only), but re-run
      `cargo test -p rticx-xbin-pass` only if Phase 1/2 changes are in the same
      tree.

## Phase 4 — Unify the hex parsers in `rticx-xbin-proto`

**Objective:** one implementation of the optional `0x`/`0X` prefix strip and
hex-digit validation, shared by `Hash64::from_hex` and the `system.json`
`hex_u32` serde helper, so the two cannot drift.

**Background:** `hash.rs::Hash64::from_hex` parses a `u64` (1..=16 digits) with
`HashParseError`; `system.rs::hex_u32::parse` parses a `u32` (1..=8 digits) with
a plain `String` error for serde. Both now duplicate the same
prefix-strip + digit-count + `is_ascii_hexdigit` logic with separately worded
messages. Milestone references are already removed from this crate, and
`codegen.rs`/`layout.rs` duplications were collapsed in the same pass.

**Files:**
- `crates/rticx-xbin-proto/src/hash.rs` — add the shared helper; use it from
  `from_hex`.
- `crates/rticx-xbin-proto/src/system.rs` — use it from `hex_u32::parse`.

**Steps (refactor; the existing suites are the regression guard):**
1. Baseline: `cargo test -p rticx-xbin-proto`; expect GREEN.
2. Add a crate-private helper (for example
   `hash::strip_hex_digits(text: &str, max_digits: usize) -> Option<&str>`)
   that strips the optional prefix and returns `None` unless the remainder is
   non-empty, no longer than `max_digits` and all ASCII hex digits.
3. Rewrite `Hash64::from_hex` and `hex_u32::parse` to call it, keeping each
   caller's own error type and exact message wording.
4. `cargo test -p rticx-xbin-proto`; expect GREEN with the same pass count.

**Exit criteria / verification:**
- [ ] `hash.rs` and `system.rs` both call the shared helper; no duplicated
      prefix/digit logic remains.
- [ ] `tests/hash.rs` and `tests/system.rs` pass **unchanged** (the exact error
      strings are preserved).
- [ ] `cargo clippy -p rticx-xbin-proto --all-targets` and
      `cargo fmt -p rticx-xbin-proto -- --check` are clean.

---

## Phase 5 — `FieldType::named_types` returns an iterator

**Objective:** stop building a `Vec<&str>` on every call; return a lazy iterator
that recurses through arrays.

**Background:** `idl.rs::FieldType::named_types` currently allocates a fresh
`Vec` per call, and `IpcTypes::validate` calls it in two loops (the existence
check and the cycle DFS). The collection is only ever iterated.

**Files:**
- `crates/rticx-xbin-proto/src/idl.rs` — change the signature and body; update
  the two call sites in `validate`/`visit`.

**Steps (refactor; the existing suites are the regression guard):**
1. Baseline: `cargo test -p rticx-xbin-proto`; expect GREEN.
2. Change `named_types` to return `impl Iterator<Item = &str> + '_` (or a small
   `enum`-based iterator if the recursive array arm forces boxing). The array
   arm should `flat_map` back into `named_types`; the primitive arm must yield
   nothing.
3. Update the two call sites to iterate the returned iterator directly.
4. `cargo test -p rticx-xbin-proto`; expect GREEN. Confirm the cyclic-IDL and
   unknown-reference rejection tests still pass.

**Exit criteria / verification:**
- [ ] `FieldType::named_types` no longer returns a `Vec`.
- [ ] `cargo clippy -p rticx-xbin-proto --all-targets` and
      `cargo fmt -p rticx-xbin-proto -- --check` are clean.

---

## Phase 6 — Simplify the cycle check in `IpcTypes::visit`

**Objective:** align `idl.rs` cycle detection with the already-simplified
`layout.rs::Computer::named_layout`.

**Background:** `layout.rs` now finds the cycle start with a single
`stack.iter().position(...)`; `idl.rs::visit` still pre-scans with a separate
`position(...).unwrap_or(0)`. A `Visit::Active` state already implies the name
is on the stack, so the pre-scan and defensive fallback are redundant.

**Files:**
- `crates/rticx-xbin-proto/src/idl.rs` — the `Some(Visit::Active)` arm.

**Steps (refactor; the existing suites are the regression guard):**
1. Baseline: `cargo test -p rticx-xbin-proto`; expect GREEN.
2. Replace the pre-scan plus `unwrap_or(0)` with the `layout.rs` shape: a
   direct `find`/`position` on the stack to get the cycle start, then build the
   chain from there.
3. `cargo test -p rticx-xbin-proto`; expect GREEN.

**Exit criteria / verification:**
- [ ] `IpcTypes::visit` no longer computes the cycle start twice and has no
      `unwrap_or(0)`.
- [ ] The cyclic-message-reference test still passes.
- [ ] `cargo clippy -p rticx-xbin-proto --all-targets` and
      `cargo fmt -p rticx-xbin-proto -- --check` are clean.

---

## Progress log

| Phase | Status | Notes |
| ----- | ------ | ----- |
| 1 — Shared test-support module | TODO | |
| 2 — Hoist `tempfile` | TODO | |
| 3 — Docs/plan cleanup | TODO | Blocked on the plan-document decision |
| 4 — Unify the hex parsers | TODO | `crates/rticx-xbin-proto` |
| 5 — `named_types` iterator | TODO | `crates/rticx-xbin-proto` |
| 6 — Simplify `IpcTypes::visit` cycle check | TODO | `crates/rticx-xbin-proto` |
