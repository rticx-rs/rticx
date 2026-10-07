# xbin: uncached-platform support policy, drop the unsupported cache policy, compact the FIFO header

## Goal

State a clear v1 platform-support rule — a shared IPC region must be
**effectively non-cacheable (or configurable to be so)**, which covers the
overwhelming majority of multicore embedded MCUs — and make the code match it by
removing the v1-unsupported cacheable policy and compacting the FIFO header from
two 32-byte padded slots (64 bytes) to two `AtomicU32` indices (8 bytes).

## Context

Workspace: `experiments/multibinary` (standalone Cargo workspace, its own
`Cargo.toml`). Docs: `docs/architecture.md`, `docs/user-guide.md`. Historical
design: `plans/multibinary-multicore-plan.md`.

Current state that the change reacts to:

- **Platform rule is over-specified.** `docs/architecture.md` §6 and §9 and
  `docs/user-guide.md` §7.4 require the pool be mapped *"Normal, Non-cacheable,
  Shareable"* and forbid Device/Strongly-ordered. That is one ARMv7-M MPU
  recipe. It excludes valid mechanisms (MMU page attributes, an uncached
  address alias, a vendor cache-attribute/PMA register, or simply a core with no
  data cache) and, read literally, excludes cores that are already uncached.
- **A cacheable variant exists but is unsupported.**
  `rticx-xbin-pass::CachePolicy` (`crates/rticx-xbin-pass/src/binding.rs`)
  has `NormalNonCacheableShareable` and `NormalCacheableShareable`; the value is
  carried on `IpcPool.policy`, converted in
  `rticx-xbin-pass/src/lib.rs::pool_decl`, serialized as
  `rticx-xbin-proto`'s `PoolCachePolicy` on `PoolDecl.policy`
  (`crates/rticx-xbin-proto/src/manifest.rs`), and matched in
  `crates/rticx-xbin-proto/src/merge.rs`. v1 supports exactly one policy, so the
  whole policy surface is dead weight.
- **The 64-byte header is two padded index slots.**
  `crates/rticx-xbin-proto/src/fifo.rs`: `FIFO_INDEX_STRIDE = 32`,
  `FIFO_HEADER = 2 * FIFO_INDEX_STRIDE = 64`.
  `crates/rticx-xbin-rt/src/fifo.rs`: `head`/`tail` are `AtomicUsize` padded to
  32 via an `IndexSlot { value, _pad }`. The padding only matters for a
  *cacheable, coherent* pool; v1 pools are non-cacheable, so it is waste. The
  header must use a fixed-width counter, because `AtomicUsize` is 8 bytes on the
  host and 4 on the target and the canonical image is computed on the host.

Decisions taken by this plan (do not re-litigate while implementing):

- Remove `CachePolicy`, `PoolCachePolicy`, the `IpcPool.policy` field, the
  `PoolDecl.policy` field and the policy comparison in merge. Bump
  `MANIFEST_SCHEMA_VERSION` 4 → 5 (a field is removed from the manifest).
  `system.json` schema stays 3 — `PoolEntry` never carried the policy.
- New canonical FIFO image: `head: AtomicU32 @0`, `tail: AtomicU32 @4`,
  elements `@8`; `FIFO_INDEX_STRIDE = 4`, `FIFO_HEADER = 8`, `FIFO_ALIGN` stays
  `8`.
- Platform support is a **documented contract plus the per-distro
  `configure_shared_memory` hook**, not a compile-time gate (the framework
  cannot portably detect an MPU at compile time).

### Arithmetic to reuse while re-baselining tests

- `EncryptReq`: size 12, align 4, `capacity = 2` → depth 3 →
  `fifo_size = 8 + 3*12 = 44`.
- `OtherReq` (the `BackTask` input): size 4, `capacity = 1` → depth 2 →
  `fifo_size = 8 + 2*4 = 16`.
- Three `EncryptReq` FIFOs in one direction: offsets `0, 48, 96`
  (`44` then `align_up(44)=48`, `48+44=92` then `align_up=96`).
- Two directions sharing a pool, name order `BackTask` < `EncryptTask`:
  `BackTask @0` (16), `EncryptTask @16` (44), `used = 60`.
- `size_of::<Fifo<Msg, 3>>() = 44`, `align_of = 4`;
  `size_of::<Fifo<Word, 2>>() = 16`.

### Commands

```bash
cd /home/zakaria/rtic-mc-experiments/experiments/multibinary
cargo fmt --all --check
cargo test                      # default-members (all but the H7 no_std lib)
cargo test -p rticx-xbin-rt     # focused while iterating
cargo test -p rticx-xbin-proto
cargo test -p rticx-xbin-pass
cargo test -p rticx-xbin-driver

# H7 distribution (macro + no_std lib; host cannot build the lib)
cd rticx-stm32h7 && make clippy && make build
```

`cargo xbin` is only needed for the `make examples` / Renode acceptance run, not
for `cargo test`.

## Definition of done

- [ ] `cargo fmt --all --check` passes.
- [ ] `cargo test` passes (workspace default-members).
- [ ] `RUSTFLAGS="-Dwarnings" cargo clippy --all-targets` passes for the
      default-members.
- [ ] `cd rticx-stm32h7 && make clippy && make build` passes for `cm7`/`cm4`.
- [ ] `grep -rn "CachePolicy\|PoolCachePolicy\|NormalCacheableShareable\|is_cacheable" docs crates rticx-stm32h7` returns no matches.
- [ ] `grep -rn "cache-line-padded\|32-byte cache-line\|two 32-byte\|FIFO_HEADER\` (64)" docs crates` returns no matches.
- [ ] `grep -rn "AtomicUsize" crates/rticx-xbin-rt docs` returns no matches.
- [ ] `rticx_xbin_proto::FIFO_HEADER == 8`, `rticx_xbin_rt::FIFO_HEADER == 8`.
- [ ] `docs/architecture.md` states the non-cacheable-or-configurable rule and
      the market coverage claim; the worked example uses offsets `0/4/8/44`.

## Out of scope

- Supporting cacheable-but-maintained pools (a future `CachePolicy::Maintained`
  / transport-owned clean+invalidate). Do not add hooks now.
- A compile-time platform gate or a new public conformance enum.
- Cross-core atomicity support for cores without exclusives (e.g. ARMv6-M /
  RP2040 SIO spinlocks); only the *documentation caveat* is in scope.
- Changing `FIFO_ALIGN` (stays 8) or the `depth = capacity + 1` discipline.

---

## Phase 1 — Compact the FIFO header to two `AtomicU32` (8 bytes)

**Objective:** the canonical image is `head u32 @0`, `tail u32 @4`,
elements `@8`, and every host-side size/offset expectation matches it.

**Files:**
- `crates/rticx-xbin-proto/src/fifo.rs` — `FIFO_INDEX_STRIDE` 32 → 4,
  `FIFO_HEADER` 64 → 8; rewrite the module doc offset diagram and the
  "32-byte cache-line slot" wording.
- `crates/rticx-xbin-rt/src/fifo.rs` — replace `IndexSlot`/`INDEX_PAD` with
  `head: AtomicU32` / `tail: AtomicU32` and `use portable_atomic::AtomicU32`;
  update the `const` asserts to `tail @4`, `buffer @8`; load/store with
  `as usize` / `0u32`; add a const assert that the depth fits a `u32`
  (`DEPTH as u64 <= u64::from(u32::MAX) + 1`, compared in `u64` to avoid the
  32-bit `u32::MAX + 1` overflow); rewrite the module doc.
- `crates/rticx-xbin-rt/src/lib.rs` — re-export list is unchanged; fix any
  "cache-line-padded" wording in the doc.
- `crates/rticx-xbin-rt/tests/fifo.rs` — drift guard update.
- `crates/rticx-xbin-proto/tests/merge.rs`, `crates/rticx-xbin-proto/tests/alloc.rs`,
  `crates/rticx-xbin-driver/tests/fixture.rs`, `crates/rticx-xbin-pass/tests/codegen.rs`
  — numeric expectations (see steps).

**Steps (TDD):**

1. **Red:** in `crates/rticx-xbin-rt/tests/fifo.rs`, assert the new image
   before changing the sources: add/keep the `FIFO_HEADER`/`FIFO_INDEX_STRIDE`
   drift comparisons (they fail against the current 64), change
   `host_fifo_size`'s rounding from `align_of::<usize>()` to `align_of::<u32>()`,
   and change `assert_eq!(align_of::<Fifo<Msg, 3>>(), align_of::<usize>())` to
   `align_of::<u32>()`. Run `cargo test -p rticx-xbin-rt`; expect RED
   (drift guard mismatch, size assertions off).
2. **Green (proto + rt):** apply the constant and layout changes above. Run
   `cargo test -p rticx-xbin-rt`; expect GREEN (threaded hand-off test still
   passes — the FIFO does only loads/stores, no RMW).
3. **Red/green (host allocations):** `cargo test -p rticx-xbin-proto` fails on
   the recorded sizes. Re-baseline, per the arithmetic above:
   - `tests/merge.rs`: `fifo_bytes() == 100` → `44`; `used == 100` → `44`
     (single-FIFO cases). In `pool_fit_accounts_for_fifo_alignment`, the two
     `EncryptReq` FIFOs are now at `0` and `48` with `used == 92`; lower that
     test's budget to `91` so it still overflows and update its message to
     `"… need 92 bytes, but the distribution reserves 91"`; assert Beta's
     offset is `48`. In the shared-pool case, `BackTask @0`/`EncryptTask @16`
     with `used == 60`; the overflow test's budget of `150` no longer
     overflows — set it below `60` (e.g. `50`) and update the message
     accordingly. In the exact-fit test, set budget `60` and assert `used == 60`,
     `BackTask.fifo_bytes() == 16`, `EncryptTask.fifo_bytes() == 44`.
   - `tests/alloc.rs`: `used == 100` → `44`; offsets `104`/`208` → `48`/`96`;
     `BackTask` `72` → `16` and `EncryptTask`'s offset `72` → `16`; fix the
     comments that spell out `64 + …`.
   - `crates/rticx-xbin-driver/tests/fixture.rs`: `fifo_bytes() == 100` → `44`
     and both `pool.used == 100` → `44`.
   - `crates/rticx-xbin-pass/tests/codegen.rs`: the asserted literal
     `__RTICX_XBIN_FIFO_OFFSET + 100usize <= __RTICX_XBIN_POOL_BUDGET` → `44usize`.
     (The `beta["fifo"]["offset"] = 64` at ~line 1050 is an arbitrary distinct
     offset for the router test, not header-derived — leave it.)
   Run each affected crate's tests; expect GREEN.
4. **Realism pass (not asserted, optional but preferred):** update the
   hand-written `system.json` strings in `crates/rticx-xbin-pass/tests/init_hooks.rs`
   (second FIFO offset `88` → `32`), `tests/codegen.rs`, `tests/e2e_runtime.rs`
   and `tests/priority_lines.rs` (`"used": 100` → `44`) so fixtures reflect a
   re-synced project. Re-run `cargo test -p rticx-xbin-pass`, `-p rticx-xbin-driver`.
5. **Refactor:** fold the `as usize`/`as u32` conversions into small private
   helpers if they read poorly; keep tests GREEN. Run `cargo fmt --all`.

**Exit criteria / verification:**
- [ ] `cargo test -p rticx-xbin-rt -p rticx-xbin-proto -p rticx-xbin-pass -p rticx-xbin-driver` passes.
- [ ] `grep -rn "AtomicUsize\|INDEX_PAD\|IndexSlot" crates/rticx-xbin-rt` returns no matches.
- [ ] `rticx_xbin_proto::FIFO_HEADER == rticx_xbin_rt::FIFO_HEADER == 8`.

---

## Phase 2 — Remove the v1-unsupported cache policy

**Objective:** `CachePolicy`/`PoolCachePolicy` and the `policy` fields are gone;
the cross-endpoint match still checks id, physical cores, budget and swapped
bases; the manifest schema is bumped.

**Files:**
- `crates/rticx-xbin-pass/src/binding.rs` — delete `CachePolicy`, its
  `is_cacheable`, the `IpcPool.policy` field, the module-doc example line, and
  the `cache_policy_marks_the_cacheable_fallback` test; reword the `IpcPool` doc
  to drop "and policy".
- `crates/rticx-xbin-pass/src/lib.rs` — drop `PoolCachePolicy` from the
  `rticx_xbin_proto` import list and `CachePolicy` from the re-export; simplify
  `pool_decl` to stop mapping the policy.
- `crates/rticx-xbin-proto/src/manifest.rs` — delete `PoolDecl.policy` and the
  `PoolCachePolicy` enum; bump `MANIFEST_SCHEMA_VERSION` to `5` and document
  schema 5 ("removed the redundant per-pool `policy`: v1 has a single supported
  cache policy, documented in `rticx-xbin-pass::XbinPassBackend`").
- `crates/rticx-xbin-proto/src/lib.rs` — drop the `PoolCachePolicy` re-export.
- `crates/rticx-xbin-proto/src/merge.rs` — delete the
  `|| reverse.policy != pool.policy` arm in `match_pools`; update the module
  docs and the `match_pools` doc line that list "policy".
- `crates/rticx-xbin-proto/src/error.rs` — reword the `IpcPoolMismatch`
  display message (~line 334) to drop "and policy".
- `crates/mock/xbin-mock-capability/src/lib.rs` — drop the `CachePolicy`
  import and field.
- `crates/mock/xbin-mock-distro/src/lib.rs` — drop the
  `assert_eq!(mirror.policy, pool.policy)` in
  `pools_are_symmetric_across_endpoints` and the "and policy" doc comment.
- `rticx-stm32h7/rticx-stm32h7-macro/src/lib.rs` — drop the `CachePolicy`
  import, the `policy:` field in `ipc_pools`, and the policy assertion in the
  test module.
- Tests: `crates/rticx-xbin-proto/tests/manifest.rs` (drop the policy field and
  its JSON assertion; update the two `assert_eq!(MANIFEST_SCHEMA_VERSION, 4)` and
  `"schema_version": 4` assertions to `5`), `tests/merge.rs` (pool helper field,
  manifest builders, and the `IpcPoolMismatch` message string),
  `tests/alloc.rs` (pool helper field), and
  `crates/rticx-xbin-pass/tests/metadata.rs` (drop the `CachePolicy` import, the
  `policy` parameter of the `pool()` helper, the `axi` cacheable fixture, and the
  `policy` assertions at ~lines 614–625 and 637–640).

**Steps (TDD):**

1. **Red:** in `crates/rticx-xbin-proto/tests/manifest.rs`, change the schema
   expectations to `5` and remove the `"policy"` JSON assertion; in
   `crates/rticx-xbin-pass/tests/metadata.rs`, remove the `axi`
   `NormalCacheableShareable` fixture and its assertions. Run
   `cargo test -p rticx-xbin-proto -p rticx-xbin-pass`; expect RED
   (schema constant still 4, code still emits `policy`).
2. **Green (proto):** remove `PoolCachePolicy`/`PoolDecl.policy`, bump the
   schema constant, drop the merge comparison and reword the error; fix the
   proto test helpers. Run `cargo test -p rticx-xbin-proto`; expect GREEN.
3. **Green (pass + mock + H7):** remove `CachePolicy`/`IpcPool.policy`, simplify
   `pool_decl`, update `xbin-mock-capability`, `xbin-mock-distro` and
   `rticx-stm32h7-macro`. Run `cargo test -p rticx-xbin-pass -p xbin-mock-capability -p xbin-mock-distro`
   and `cd rticx-stm32h7 && make clippy`; expect GREEN.
4. **Refactor:** make sure the `IpcPoolMismatch` error text and its test string
   match exactly; run `cargo fmt --all`.

**Exit criteria / verification:**
- [ ] `cargo test -p rticx-xbin-proto -p rticx-xbin-pass -p xbin-mock-capability -p xbin-mock-distro` passes.
- [ ] `cd rticx-stm32h7 && make clippy && make build` passes for `cm7` and `cm4`.
- [ ] `grep -rn "CachePolicy\|PoolCachePolicy\|NormalCacheableShareable\|is_cacheable" crates rticx-stm32h7` returns no matches.
- [ ] `grep -rn "same budget and policy\|budget and policy" crates` returns no
      matches. (The two `docs` hits — `architecture.md`, `user-guide.md` — are
      Phase 3's responsibility; the `docs` half of this grep is verified there.)

---

## Phase 3 — State the platform policy and update the markdown docs

**Objective:** the docs say the IPC region must be non-cacheable or
configurable to be so, that this covers the overwhelming majority of multicore
embedded MCUs, and no doc still describes the removed policy or the 64-byte
header.

**Files:**
- `docs/architecture.md`
- `docs/user-guide.md`
- `crates/rticx-xbin-rt/src/fifo.rs` (module-doc platform/cache wording only;
  the layout edits landed in Phase 1)
- `plans/multibinary-multicore-plan.md` (only statements of current design; the
  rest is historical)
- `crates/rticx-xbin-pass/src/codegen.rs` (rustdoc: platform/cache wording only)
- `rticx-stm32h7/src/xbin.rs` (rustdoc: platform/cache wording only)
- `rticx-stm32h7/README.md`, `rticx-stm32h7/renode/README.md`,
  `rticx-stm32h7/examples-apps/app-cm7/src/main.rs` (platform/cache wording only)

**Steps:**

1. **`docs/architecture.md` §6 "Memory and layout":**
   - First bullet: drop "`CachePolicy`" and "same budget/policy"; change the
     canonical-image clause to "two 4-byte `AtomicU32` indices, then the element
     payload, 8-byte FIFO alignment, depth `capacity + 1`".
   - Runtime FIFO bullet: `AtomicUsize` → `AtomicU32`; drop
     "cache-line-padded indices"; note the 8-byte header.
   - Cache/MPU bullet: replace the ARMv7-M recipe with the portable
     requirement (text below).
   - Worked example: rewrite the `EncryptTask` block to
     ```
     offset  0   head index (AtomicU32, 4 bytes)
     offset  4   tail index (AtomicU32, 4 bytes)
     offset  8   elements, depth = capacity + 1 = 3 × 12 bytes
     offset 44   end of the FIFO (the next FIFO starts at the next 8-byte boundary, 48)
     ```
     and change "`FIFO_HEADER` (64)" to "`FIFO_HEADER` (8)".
   - Replace the "Cache-maintenance decision tree" with a
     "Cache policy and supported platforms" subsection containing the rule and
     coverage statement.
2. **`docs/architecture.md` §8 and §9:** in the boot-hook bullet and the
   `configure_shared_memory` contract row, replace "maps its own view of the IPC
   pools Normal, Non-cacheable, Shareable" with "makes its own view of the IPC
   pools effectively non-cacheable and shared (mechanism-agnostic)"; drop "and
   the policy" from the `ipc_pools` row and the final paragraph's redundancy.
3. **`docs/user-guide.md`:**
   - §7.4 first bullet: use the portable requirement text plus the coverage
     statement and the cross-core-atomicity caveat.
   - §7.4 "Cacheable mapping (unsupported v1)" bullet: reword to "a cacheable
     mapping without transport-owned maintenance is unsupported in v1".
   - §7.5 `IpcPool` description: drop "and a `CachePolicy`" and "budget and
     policy" (leave budget only).
   - §10 error table row: drop "policy" from the symmetric-capability fix text.
4. **`plans/multibinary-multicore-plan.md`:** update only current-design
   statements — the `rticx-xbin-rt` bullet ("cache-line-padded indices" →
   "4-byte `AtomicU32` head/tail, 8-byte header"), §7.1 step on 8-byte
   alignment, and the cache-policy mentions; add a one-line "(historical; see
   `docs/architecture.md` for the current contract)" note where it still
   describes the removed policy.
5. **Source rustdocs/READMEs** (`crates/rticx-xbin-pass/src/codegen.rs`,
   `rticx-stm32h7/src/xbin.rs`, `rticx-stm32h7/README.md`,
   `rticx-stm32h7/renode/README.md`,
   `rticx-stm32h7/examples-apps/app-cm7/src/main.rs`): reword the ARMv7-M
   recipe ("Normal, Non-cacheable, Shareable", the Device/Strongly-ordered
   prohibition) to the portable "effectively non-cacheable and shared" rule.
   Keep the "a cacheable mapping is unsupported in v1" statement, and apply the
   same rewrite to the `crates/rticx-xbin-rt/src/fifo.rs` module doc. No
   code/behavior change.

**Canonical rule text to reuse verbatim:**

> An IPC pool must be usable from every core that touches it as **effectively
> non-cacheable and shared**, by whatever mechanism the platform provides: a
> core with no data cache, an MPU/MMU region attribute, a cache-attribute/PMA
> register, an uncached address alias, or a design-time non-cacheable region.
> This covers the overwhelming majority of multicore embedded MCUs: all
> heterogeneous Cortex-M parts in the M7+M4 class, all homogeneous multicore
> M-class parts whose cores have no data cache, and most multicore RISC-V MCUs.
> A cacheable shared region without the cache maintenance the transport would
> have to perform is unsupported in v1 (D2). Cores that also lack cross-core
> atomic primitives (for example ARMv6-M, which has no `LDREX`/`STREX`)
> additionally require a distribution-provided cross-core lock; `portable-atomic`'s
> `critical-section` fallback is local-core only and is not sufficient.

**Exit criteria / verification:**
- [ ] `grep -rn "cache-line-padded\|32-byte cache-line\|two 32-byte\|FIFO_HEADER\` (64)\|AtomicUsize" docs plans` returns no matches in the current-design text.
- [ ] `grep -rn "CachePolicy\|PoolCachePolicy\|budget and policy\|Normal, Non-cacheable, Shareable" docs crates/rticx-xbin-pass/src/codegen.rs crates/rticx-xbin-rt/src/fifo.rs rticx-stm32h7` returns no matches (the historical plan may keep a clearly-labelled note).
- [ ] `grep -n "overwhelming majority of multicore embedded" docs/architecture.md` matches.
- [ ] `cargo test` still passes (docs-only phase, but keep the tree green).

---

## Progress log

| Phase | Status | Notes |
| ----- | ------ | ----- |
| 1     | DONE   | FIFO header → two `AtomicU32` (8 bytes), `FIFO_HEADER=8`; re-baselined host sizes. Note: real `OtherReq` fifo is 12 (size 2) not the plan's 16; offsets/`used==60` unchanged. Extra stale-`used` fixtures re-baselined in driver `visualize.rs`, proto `system.rs`, pass `receiver_compile.rs`, `codegen.rs`. |
| 2     | DONE   | `CachePolicy`/`PoolCachePolicy` and `policy` fields removed; manifest schema 4 → 5; `pool_decl` simplified; mocks + H7 macro updated. H7 `make clippy && make build` green (cm7/cm4). Note: `axi` cacheable fixture removed → the multi-pool "sorted by peer" ordering check lost coverage (production sort unchanged). |
| 3     | DONE   | Portable non-cacheable-or-configurable policy stated (canonical rule verbatim in `docs/architecture.md` "Cache policy and supported platforms" §6/§8/§9, `docs/user-guide.md` §7.4); 64-byte header/`AtomicUsize` wording removed; worked example uses `0/4/8/44`; source rustdocs/READMEs (codegen.rs, xbin.rs, H7 READMEs, app-cm7) reworded; `multibinary-multicore-plan.md` current-design statements updated with a labelled historical M6.9 note. Definition of Done greps all clean; `cargo fmt`/`clippy`/`cargo test`/H7 `make clippy && make build` green. |
