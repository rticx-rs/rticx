# xbin dispatcher stealing — implementation plan

**Goal:** delete the pass-owned `ipc_dispatchers` `#[app]` argument and make the
cross-binary (xbin) pass **steal the front of the regular `dispatchers` list**,
leaving the remainder for `rticx-sw-pass` / `rticx-async-pass`.

**Scope:** `experiments/multibinary/` only (the pass, the mock distribution,
the fixtures, the STM32H7 examples) plus the three documents under
`experiments/multibinary/docs/`. No root-workspace crate changes.

**Explicit non-goal (decided):** the xbin pass does **not** try to compute the
local (sw/async) dispatcher demand. If the combined list is too short, the
xbin pass only reports its own missing cross-binary line; the shortfall for
local tasks is reported later by `rticx-sw-pass`/`rticx-async-pass`, **with a
wrong "found" count**. That inaccuracy is accepted for now and will be
addressed in the future.

---

## How to use this document

The work is split into **three self-contained phases**. Each phase is meant to
be loaded into a fresh session; pick the next phase whose status is `TODO`,
do its checklist, run its acceptance commands, then update the progress log
below before committing.

Phase 1 leaves the repository consistent (the workspace builds and the pass
tests pass); the fixtures are only updated in Phase 2, and because they are
**separate workspaces** that are not compiled by `cargo test` of the
`experiments/multibinary` workspace, Phase 1 stays green without them.

---

## Progress log

| Phase | Status | Scope | Notes |
|---|---|---|---|
| 1 | DONE | pass internals + pass test suite | `crates/rticx-xbin-pass` only. `cargo fmt --check` clean; `cargo test -p rticx-xbin-pass` green (120 tests, 0 failed). `ipc_dispatchers` no longer consumed; `tests/ipc_dispatchers.rs` → `tests/dispatchers.rs`. |
| 2 | DONE | mock distro + fixtures + H7 examples + driver tests | depends on Phase 1. `cargo test` and `cargo test -p rticx-xbin-driver` green; `make -C rticx-stm32h7 fmt-check build examples` exit 0 (both apps verified). Deviation: driver `tests/negative.rs` inline sources needed `ipc_dispatchers`→`dispatchers` (plan said no change; no test asserted the old error message). |
| 3 | DONE | docs (`user-guide.md`, `architecture.md`, `multibinary-multicore-plan.md`) | depends on Phase 1/2 behaviour being final. Acceptance grep clean (text); the doc lives at `plans/multibinary-multicore-plan.md`, not `docs/` (plan path was stale) — updated there. `rticx-stm32h7/README.md` needed no change. |

Status values: `TODO`, `IN PROGRESS`, `DONE`. Add a short note (branch/PR,
deviations) in the last column when you finish a phase.

---

## Background: today's behaviour

Two disjoint, pass-owned arguments:

| Argument | Owner | Shape | Assigned to |
|---|---|---|---|
| `ipc_dispatchers` | xbin pass | per local core | one entry per **distinct `(source, priority)` cross line**, ascending |
| `dispatchers` | `rticx-sw-pass` / `rticx-async-pass` | per local core (flat array only when `cores == 1`) | one entry per **distinct priority group**, ascending priority; extras allowed |

Relevant facts established while researching this change:

- The xbin pass is bound **before** sw/async and the core pass
  (`rticx-stm32h7-macro`, `xbin-mock-distro`, and both in-tree test harnesses),
  so it can rewrite `dispatchers` before sw/async read it.
- `rticx-sw-pass`'s `assign_dispatchers`
  (`compilation-passes/rticx-sw-pass/src/common/analyze.rs`) requires
  `len >= n_priority_groups` and consumes entries in ascending-priority order;
  extra entries are ignored.
- `rticx-sw-pass` consumes `dispatchers` from the args
  (`SoftwarePass::run_pass` removes the key); `rticx-async-pass` does the same.
- The current xbin `validate_ipc_dispatchers` requires **exactly** one entry per
  cross line (too few *and* too many are errors) and rejects duplicates and
  overlap with `dispatchers`.
- The mock distribution builds its `__XbinInterrupt` enum variants from
  `ipc_dispatchers` (`crates/mock/xbin-mock-distro/src/lib.rs`,
  `ipc_dispatcher_variants`), so it must switch to `dispatchers`.
- The STM32H7 reserved-HSEM check (`rticx-stm32h7-macro`) iterates all generated
  task `binds`, so it needs no change.

## New behaviour

For each local core `c`:

1. `lines(c)` = the distinct `(spawn_by, priority)` pairs of the application's
   cross-binary receivers on `c`, sorted ascending (existing logic).
2. `K = lines(c).len()`.
3. The xbin pass takes the **first `K`** entries of `dispatchers[c]` and binds
   them to the cross lines in `lines(c)` order.
4. The **remaining** entries (`dispatchers[c][K..]`) are re-emitted into the
   `#[app]` args for `rticx-sw-pass`/`rticx-async-pass`, which assign them to
   local priority groups exactly as today.

Convention to document: **cross-binary lines come first, then the local task
dispatchers.**

Error handling:

- If `dispatchers[c]` has **fewer than `K`** entries, the xbin pass errors,
  naming the missing cross line (it cannot generate a `binds` without an
  interrupt). This replaces the old "pool too short" error and still points at
  `dispatchers`.
- Duplicate entries **within the stolen prefix** are rejected (two cross lines
  sharing one IRQ), as today.
- The old "too many entries" error is **removed**: surplus entries are local
  dispatchers.
- A shortfall for local tasks is left to sw/async (their message will report a
  wrong `found` count — accepted).
- `ipc_dispatchers` is deleted as if it never existed: an application still
  declaring it keeps the key in its args and surfaces the core pass's normal
  unknown-`#[app]`-argument diagnostic.

---

## Phase 1 — pass internals + pass test suite

**Deliverable:** `crates/rticx-xbin-pass` no longer knows `ipc_dispatchers`,
steals from `dispatchers`, and its whole test suite passes.

### Implementation checklist

- [ ] `crates/rticx-xbin-pass/src/parse.rs`
  - [ ] Delete `IPC_DISPATCHERS_ARG`, `parse_ipc_dispatchers`,
        `ipc_dispatcher_path`, `check_ipc_dispatchers`,
        `validate_ipc_dispatchers`, `parse_dispatcher_cores`.
  - [ ] Keep `DISPATCHERS_ARG`; update its doc comment.
  - [ ] `AppExtensions`: replace `ipc_dispatchers: Vec<Vec<Path>>` with
        - `dispatchers: Vec<Vec<Path>>` — the parsed input, per local core;
        - `cross_dispatchers: Vec<Vec<Path>>` — the stolen prefix, per local
          core (consumed by `codegen.rs`).
  - [ ] Add `parse_dispatchers(expr, cores) -> syn::Result<Vec<Vec<Path>>>`
        mirroring `rticx-sw-pass`'s shape rules
        (`compilation-passes/rticx-sw-pass/src/common/parse.rs`): an array
        required; flat paths only when `cores == 1`; nested arrays otherwise;
        the number of top-level lists must equal `cores`; an empty array is
        allowed. Mark with `TODO(extract)` (xbin cannot depend on sw-pass's
        internal helpers at build time — it only has sw-pass as a dev-dep).
        Use the same messages as sw-pass where practical.
  - [ ] `parse_app_args` returns `(AppExtensions, RticAttr)` (instead of
        `(AppExtensions, TokenStream)`), so `run_pass` can rewrite
        `dispatchers` *after* the receivers are parsed. Parse `dispatchers`
        into `extensions.dispatchers` but **do not consume the key**.
  - [ ] Add `steal_dispatchers(extensions, decls, attr) -> syn::Result<()>`:
        - compute `lines(c)` (the existing sorted+deduped `(spawn_by, priority)`
          logic from `validate_ipc_dispatchers`);
        - for each core: if `dispatchers[c].len() < K`, error naming the first
          missing line `(source, priority)` and telling the user to put one
          interrupt per cross line at the front of `dispatchers`;
        - reject duplicates inside the stolen prefix;
        - set `extensions.cross_dispatchers[c] = dispatchers[c][..K]`;
        - rewrite `attr`'s `dispatchers` to the remainder
          (`dispatchers[c][K..]`), preserving the parsed shape (flat when
          `cores == 1`, nested otherwise); if the key was absent, leave it
          absent; if the remainder is empty, emit an empty array of the right
          shape.
  - [ ] Update the module-level `//!` docs (drop `ipc_dispatchers`, describe
        the steal and the "cross lines first" convention).
  - [ ] Unit tests in `mod tests`: delete the `ipc_dispatchers` shape/overlap
        tests; add `parse_dispatchers` shape tests and `steal_dispatchers`
        tests (prefix stolen, remainder preserved, too-few error, duplicate
        error, same-`(source, priority)` tasks share one entry).
- [ ] `crates/rticx-xbin-pass/src/lib.rs`
  - [ ] Drop the `validate_ipc_dispatchers` import/call; call
        `steal_dispatchers(&mut extensions, &decls, &mut attr)` after
        `parse_module`, then return `attr.args_tokens()`.
  - [ ] Update the module docs: `dispatchers` is now shared (xbin consumes the
        front), `ipc_dispatchers` is gone.
- [ ] `crates/rticx-xbin-pass/src/codegen.rs`
  - [ ] `generate_receiver_items`: use `extensions.cross_dispatchers` instead
        of `extensions.ipc_dispatchers` (the `line_irq_indices` mapping stays).
  - [ ] Update the comments that mention `ipc_dispatchers` and
        `validate_ipc_dispatchers` (lines ~491, ~635).

### Test checklist (`crates/rticx-xbin-pass/tests/`)

- [ ] Rename `ipc_dispatchers.rs` → `dispatchers.rs` and rewrite it:
  - [ ] flat (one core) and nested (multi-core) steal: cross prefix is consumed
        from `dispatchers`, the remainder stays in the returned args;
  - [ ] `ipc_dispatchers` is no longer consumed/stripped (it stays in the args);
  - [ ] too-few entries error names the missing cross line;
  - [ ] surplus entries are **not** an error (they are local dispatchers);
  - [ ] duplicate stolen entries are rejected;
  - [ ] pipeline test (xbin + sw-pass): the cross line dispatcher binds the
        first entry, the local software dispatcher binds the next one; the
        local pass never sees the stolen entry.
- [ ] `tests/codegen.rs`: replace every `ipc_dispatchers = ...` with the
        equivalent `dispatchers = ...` (cross lines first); update or drop
        assertions for the removed "extra/duplicate/overlap" errors; keep the
        generated-dispatcher assertions (lines ~236, 248, 830, 968, 1036,
        1295, 1539, 1550).
- [ ] `tests/syntax.rs` (lines ~80, 92), `tests/priority_lines.rs` (~244),
        `tests/receiver_compile.rs` (~330), `tests/e2e_runtime.rs` (~38, ~222,
        ~473, ~481, ~683), `tests/metadata.rs` (~223, ~268, ~304, ~325, ~460,
        ~505): same mechanical replacement. In `e2e_runtime.rs` the receiver
        modules define `__XbinInterrupt` by hand, so the enum staying unchanged
        is expected.
- [ ] `tests/core_ids.rs` already passes `dispatchers = [[IRQ0], [IRQ1,
        IRQ2]]`; confirm it still passes (the multi-core shape is now parsed by
        xbin too) and that its assertions are unaffected.
- [ ] `tests/init_hooks.rs`: confirm it uses no dispatchers argument (no change
        expected).

### Acceptance — Phase 1

```bash
cd experiments/multibinary
cargo fmt -p rticx-xbin-pass
cargo test -p rticx-xbin-pass
```

All pass-crate tests green. (Fixtures still use `ipc_dispatchers`; they are
separate workspaces and are handled in Phase 2.)

---

## Phase 2 — mock distro, fixtures, H7 examples, driver tests

**Deliverable:** every fixture and the STM32H7 demo use a single `dispatchers`
list; `cargo test` of the workspace (including the driver, which builds the
fixtures) is green.

### Implementation checklist

- [ ] `crates/mock/xbin-mock-distro/src/lib.rs`
  - [ ] `inject_interrupt_support` / `ipc_dispatcher_variants`: collect the
        enum variants from `dispatchers` instead of `ipc_dispatchers`; rename
        the helper accordingly and update the doc comments. (The enum covers
        both the stolen cross entries and the leftover local entries.)
- [ ] Fixtures (mechanical: `ipc_dispatchers = [..]` → `dispatchers = [..]`,
      cross lines first; update the surrounding comments):
  - [ ] `crates/mock/fixtures/e2e/app-m4/src/main.rs`
  - [ ] `crates/mock/fixtures/three-app/app-m4/src/main.rs` (4 cross lines)
  - [ ] `crates/mock/fixtures/three-app/app-m5/src/main.rs`
  - [ ] `crates/mock/fixtures/three-app/app-m7/src/main.rs`
  - [ ] `crates/mock/fixtures/metadata/app-m4/src/main.rs`
  - [ ] `crates/mock/fixtures/three-app/Cargo.toml` comment block
- [ ] STM32H7:
  - [ ] `rticx-stm32h7/examples-apps/app-cm4/src/main.rs`
  - [ ] `rticx-stm32h7/examples-apps/app-cm7/src/main.rs`
  - [ ] `rticx-stm32h7/rticx-stm32h7-macro/src/lib.rs` doc comment (line ~78)
- [ ] Driver tests: confirm `crates/rticx-xbin-driver/tests/*` need no code
      change (they drive `cargo xbin`, which now emits the new fixtures); if a
      test asserts on an `ipc_dispatchers`-era error message, update it.

### Acceptance — Phase 2

```bash
cd experiments/multibinary
cargo test                      # default-members, includes the driver tests
cargo test -p rticx-xbin-driver # builds the e2e/three-app/metadata fixtures
```

And, if the Cortex-M targets and `cargo-xbin` are installed:

```bash
cd experiments/multibinary/rticx-stm32h7
make fmt-check build examples
make renode                     # optional, 4 s under Renode
```

---

## Phase 3 — documentation

**Deliverable:** no stale `ipc_dispatchers` references in the docs; the shared
`dispatchers` convention is documented, including the accepted inaccuracy of
the local-shortfall error.

### Checklist

- [ ] `docs/user-guide.md`
  - [ ] §5 "Declaring cross-binary tasks": the `#[app]` argument table
        (line ~297) and the worked example (lines ~193, ~226, ~396) — replace
        `ipc_dispatchers` with `dispatchers` and state the convention (cross
        lines first, then local task dispatchers; leftovers go to the
        software/async pass).
  - [ ] Multi-source variant (line ~583): same substitution, keep the
        ascending `(source, priority)` order note.
  - [ ] "Common errors" (line ~855) and the troubleshooting row (~920, ~731):
        describe the new "not enough entries" error and the known confusing
        local shortfall.
  - [ ] The owner table (line ~176) and argument table (line ~332).
- [ ] `docs/architecture.md`
  - [ ] §2 crate table (line ~52), §3/§4 dispatch description (line ~157),
        §7 (line ~316) and any `ipc_dispatchers` mention: replace with the
        shared-`dispatchers` model.
- [ ] `docs/multibinary-multicore-plan.md` (the reference/plan document)
  - [ ] §2 locked decisions (line ~385) and the dispatch chain (line ~157).
  - [ ] §5 implementation map (`parse.rs` row, line ~204) and the test list
        (line ~234).
  - [ ] §8 phase-2 receiver side (lines ~589, ~590) and §11 testing (line
        ~732).
  - [ ] §12 docs (lines ~797, ~805) and §13 M6.5/M6.9 task text where
        `ipc_dispatchers` is named.
  - [ ] §14 risks (lines ~1053, ~1065): rewrite the `ipc_dispatchers` count
        risk as the shared-pool ordering/`found`-count note.
  - [ ] §15 glossary (lines ~1102, ~1105): redefine the dispatcher pool and the
        line dispatcher.
- [ ] `rticx-stm32h7/README.md`: check the HSEM/doorbell paragraph (~line 109)
      for `ipc_dispatchers` wording (grep shows none, verify while there).

### Acceptance — Phase 3

```bash
cd experiments/multibinary
grep -rn "ipc_dispatchers" docs rticx-stm32h7  # only deliberate "removed" notes may remain
```

---

## Risks and notes

- **Accepted inaccuracy:** when the combined list is too short for local tasks,
  `rticx-sw-pass`/`rticx-async-pass` report the remainder as `found`, so the
  number is wrong. Deliberate; revisit later with an accurate combined-demand
  check.
- **Shape duplication:** xbin must parse `dispatchers` with the same shape rules
  as sw-pass to steal from the right core. Keep the messages aligned and mark
  the helper `TODO(extract)`.
- **Both sw and async bound:** pre-existing limitation — sw-pass consumes
  `dispatchers` first, async sees none. Unchanged by this work.
- **Priority-assigning passes:** the steal assumes the local priorities are
  final when xbin runs. xbin is bound before sw/async; no
  `rticx-auto-assign`/`rticx-deadline-pass` is bound in the H7/mock
  distributions, so this holds today. Document that a distribution must bind
  xbin after any pass that rewrites priorities.
- **Determinism:** head-stealing keeps the mapping deterministic
  (`(source, priority)` ascending for cross lines, priority ascending for the
  remainder).
