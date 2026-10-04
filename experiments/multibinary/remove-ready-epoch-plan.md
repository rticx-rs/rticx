# Plan: remove the ready-bitmap / epoch boot framework

**Status:** ready to execute
**Scope:** `experiments/multibinary/**` (the standalone experimental workspace)
**Decision record:** this plan implements a design decision to drop the
framework-provided `SharedState` (ready bitmap + epoch) machinery from the
multi-binary extension.

## 1. Why

The framework currently provides a cross-core "ready bitmap + epoch" protocol
(`rticx-xbin-rt/src/state.rs`, `SharedState`/`ReadyCache`) and wires it into
generated code:

- `__rticx_xbin_init_shared` (owner core) and
  `__rticx_xbin_mark_ready_core<N>` (every core) hooks;
- a spawn-side `ReadyCache` target-ready gate inside every generated
  `cross_spawn`;
- runtime `CrossBinBackend` methods `shared_state()` / `init_shared()` /
  `mark_ready()` / `is_ready()` / `epoch()`.

This was rejected because:

1. **Boot sequencing is a distribution responsibility.** It is up to the
   distribution to guarantee the other cores are booted before interrupts/IPC
   are used. RTICX-core already provides multiple injection points
   (`RticPass::main_injection`, `CorePassBackend::post_init`) for exactly this.
2. **The framework should not define this.** It bakes one boot handshake into
   every distribution and every generated binary.
3. **It over-complicates a distribution.** Every distribution must implement
   and reason about the ready/epoch protocol even when its hardware has a
   simpler/different boot guarantee.

## 2. Decisions (locked)

1. **Keep generated** `__rticx_xbin_configure_shared_memory` and
   `__rticx_xbin_init_fifos_core<N>`. Only the ready/epoch hooks go.
2. **Distribution replaces boot release with `post_init`.** `CorePassBackend::post_init`
   is the intended hook (e.g. the RP2040 distribution wakes the other core
   there). The H7 distribution moves its `clear_doorbells()` +
   `release_secondary_core()` there.
3. **Peer reset is a distribution responsibility.** The framework no longer
   detects or reports resets.
4. **The H7 demo uses a distribution-owned "peer up" flag** instead of the
   framework ready bit.

## 3. Target end state (what must remain)

Do **not** over-delete. The following are unrelated to this change and stay:

- `rticx_xbin_rt::Fifo`, `CrossCoreMessage`, `Queue`, `IpcRegion`.
- `CrossBinBackend::{current_global_core_id, ipc_region, configure_shared_memory,
  clean_range, invalidate_range}`.
- Generated `__rticx_xbin_configure_shared_memory` (before user `init`) and
  `__rticx_xbin_init_fifos_core<N>` (before `post_init`).
- `cross_spawn`'s runtime current-core guard, FIFO enqueue, doorbell ring, and
  its `Ok`/`Err(None)`/`Err(Some)` error model minus the "target not ready"
  case.

New `cross_spawn` semantics:

| Result | Meaning |
|---|---|
| `Ok(())` | input enqueued, target doorbell rung |
| `Err(None)` | input enqueued, but the doorbell could not be rung; re-notify |
| `Err(Some(input))` | nothing enqueued: FIFO full **or** caller is on the wrong core |

## 4. Session protocol

- Work top to bottom. Each phase is a checklist; tick items as they land.
- Keep the workspace compiling after each **phase group** below; the runtime
  trait removal (Phase 1), the mock (Phase 3) and the H7 distro (Phase 4) must
  land before `cargo test` at the workspace root passes again.
- After each phase run the listed verification and append a line to
  [§10 Progress log](#10-progress-log).
- Suggested session split:
  - **Session A** = Phases 1–3 + `cargo test` for the three affected crates.
  - **Session B** = Phase 4 (H7) + H7 `make all`.
  - **Session C** = Phase 5 (test harnesses) + Phase 6 (docs) in either order.
  - **Session D** = final verification of §9.

---

## Phase 0 — Baseline

- [x] `cd experiments/multibinary && cargo test` is green before starting.
- [x] `cd experiments/multibinary/rticx-stm32h7 && make all` is green before starting.

---

## Phase 1 — Runtime crate `rticx-xbin-rt`

- [ ] **Delete `crates/rticx-xbin-rt/src/state.rs`.**
- [ ] **Delete `crates/rticx-xbin-rt/tests/state.rs`.**
- [ ] `crates/rticx-xbin-rt/src/lib.rs`
  - [ ] Remove `pub mod state;`.
  - [ ] Remove `pub use state::{MAX_CORES, ReadyCache, SharedState};`.
  - [ ] Update the crate doc block: drop the `SharedState`/`ReadyCache`
        bullet; describe that boot coordination is distribution-owned.
- [ ] `crates/rticx-xbin-rt/src/backend.rs`
  - [ ] Remove `fn shared_state(&self) -> &SharedState;` and the provided
        methods `init_shared`, `mark_ready`, `is_ready`, `epoch`.
  - [ ] Remove the `use crate::SharedState;` import.
  - [ ] Rewrite the module `# Boot protocol` section: the distribution owns
        boot sequencing; it must guarantee peers are up before IPC/interrupts
        are used. Mention `CorePassBackend::post_init` as the injection point.
  - [ ] Update the `# Shared-memory requirements` cross-reference to
        `crate::SharedState` (remove it).
- [ ] `crates/rticx-xbin-rt/src/fifo.rs`
  - [ ] Line ~160 doc: replace "Call it on the owner core during
        `init_shared`, before publishing" with wording that does not mention
        `init_shared` (e.g. "Call it on the producer core before any spawn").

**Verify:** `cargo build -p rticx-xbin-rt` (mock/pass/h7 still broken until
Phases 2–4).

---

## Phase 2 — Pass `rticx-xbin-pass`

File: `crates/rticx-xbin-pass/src/codegen.rs`

- [ ] **`HookPlan` struct (~L253–263)**
  - [ ] Remove the `owner_local: Option<u32>` field.
  - [ ] Keep `global_ids: Vec<u32>` and `fifo_locals: Vec<u32>`.
- [ ] **`HookPlan::injection` (~L268–311)**
  - [ ] `BeforeInit`: unchanged (`configure_shared_memory`).
  - [ ] `BeforePostInit`: drop the `owner_local`/`__rticx_xbin_init_shared`
        branch; keep only the `init_fifos_core<N>` statements.
  - [ ] `BeforeIdle`: remove entirely (return `None` via the `_ => None` arm).
- [ ] **`generate_init_hooks` (~L890–990)**
  - [ ] Remove `let owner = owner_core(view);` and `owner_local` computation.
  - [ ] Remove the whole `__rticx_xbin_mark_ready_core<N>` emission loop
        (~L921–936).
  - [ ] Remove the `__rticx_xbin_init_shared` emission block (~L938–956).
  - [ ] Keep the `__rticx_xbin_configure_shared_memory` item and the
        `__rticx_xbin_init_fifos_core<N>` loop.
  - [ ] Update `InitHooks`/`generate_init_hooks` doc comments (M3-T3/M6-T1
        wording: no shared-state init, no mark-ready).
- [ ] **Remove the now-unused `owner_core` helper (~L1141–1149).**
- [ ] **`generate_sender` / `cross_spawn` (~L1586–1645)**
  - [ ] Remove the `static __RTICX_XBIN_READY: ReadyCache = ...` and the
        `is_ready(...)` gate.
  - [ ] Update the `spawn_doc`: `Err(Some(input))` no longer mentions "target
        has not marked itself ready".
- [ ] **Module docs** (top of file, ~L22–30, L63–88) and any `//!` references to
      `init_shared`/`mark_ready`/`ReadyCache`/ready gate.

File: `crates/rticx-xbin-pass/src/lib.rs`

- [ ] Update the `//!` docs: remove the `ReadyCache`/ready-gate sentence
      (~L23–29) and the `__rticx_xbin_init_shared`/`mark_ready` hook list
      (~L37–42). Keep `configure_shared_memory` + `init_fifos`.

**Verify:** `cargo build -p rticx-xbin-pass` (tests still reference removed
symbols; fixed in Phase 5).

---

## Phase 3 — Mock `rticx-xbin-mock`

File: `crates/rticx-xbin-mock/src/lib.rs`

- [ ] Remove `use rticx_xbin_rt::{FIFO_ALIGN, SharedState};` → keep only
      `FIFO_ALIGN`.
- [ ] `SystemInner`: remove the `state: SharedState` field.
- [ ] `MockSystem::new`: remove the `state: SharedState::new()` initializer.
- [ ] Remove `MockSystem::state(&self) -> &SharedState`.
- [ ] `impl CrossBinBackend for MockBackend`: remove `fn shared_state(...)`.
- [ ] Update the crate/module docs (drop `[SharedState]` and ready/epoch
      wording; keep pools + doorbell transport).
- [ ] `crates/rticx-xbin-mock/tests/backend.rs`
  - [ ] Remove `ReadyCache` from the `use rticx_xbin_rt::{...}` line.
  - [ ] Delete `ready_and_epoch_delegate_to_the_shared_state`.
  - [ ] Delete `ready_cache_gates_spawns_across_a_peer_reset`.
  - [ ] Update the module doc (drop ready/epoch/peer-reset coverage).
  - [ ] `cache_hooks_are_no_ops_on_host` keeps `configure_shared_memory`.

**Verify:** `cargo test -p rticx-xbin-mock`.

---

## Phase 4 — STM32H7 distribution `rticx-stm32h7`

### 4.1 Runtime half `rticx-stm32h7/src/xbin.rs`

- [ ] Remove `use rticx_xbin_rt::SharedState;` and the `shared_state()` method.
- [ ] Remove `init_shared()`. Move its behaviour into a new distribution-owned
      boot API:
  - [ ] `pub fn boot_release()` — owner core only: `clear_doorbells()` then
        `release_secondary_core()`; also clear the new up-flag word (see 4.2).
  - [ ] `pub fn signal_self_up()` — set this core's bit in the up-flag word.
  - [ ] `pub fn core_is_up(physical: u32) -> bool` and
        `pub fn peer_is_up() -> bool` (peer of `CURRENT_CORE`).
  - [ ] Implement the up-flag word as a `portable_atomic::AtomicU32` at
        `SRAM3_BASE + bindings::BOOT_FLAG_OFFSET`; bit `p` = physical core `p`
        has booted.
- [ ] Update the module `//!` docs (boot release now in `post_init`; up-flag is
      the demo's peer-up handshake, not a framework protocol).

### 4.2 Bindings `rticx-stm32h7/rticx-stm32h7-bindings/src/lib.rs`

- [ ] Rename `SHARED_STATE_OFFSET` → `BOOT_FLAG_OFFSET` (value `0x0000`), update
      the doc comment.
- [ ] Update the layout doc block and the `control_area_precedes_the_pool` test:
      `const { assert!(BOOT_FLAG_OFFSET + 4 <= DOORBELL_OFFSET) };`
- [ ] Keep `DOORBELL_OFFSET`, `POOL_OFFSET`, everything else.
- [ ] Update the crate `//!` header (drop "shared ready/epoch state").

### 4.3 Macro `rticx-stm32h7/rticx-stm32h7-macro/src/lib.rs`

- [ ] In `Stm32H7Rtic::post_init`, after the NVIC priority/unmask statements:
  - [ ] emit `rticx_stm32h7::xbin::boot_release();` **only when**
        `physical_core() == bindings::PHYSICAL_CM7`;
  - [ ] emit `rticx_stm32h7::xbin::signal_self_up();` on every core (last).
- [ ] Update the doc comment at the `post_init` hook.

### 4.4 Examples `rticx-stm32h7/examples-apps/`

- [ ] `app-cm7/src/main.rs`
  - [ ] Update the module doc (no generated `init_shared`; boot release is in
        the distribution's `post_init`).
  - [ ] In `Idle::exec`, replace `while !backend.is_ready(1) { … }` with a poll
        of `rticx_stm32h7::xbin::peer_is_up()`; drop the now-unused
        `CrossBinBackend` import.
  - [ ] Update the printed log lines (no "marked itself ready" wording tied to
        the framework).
- [ ] `app-cm4/src/main.rs`
  - [ ] Update the module doc ("marks itself ready in the shared state" →
        "signals the distribution's peer-up flag").
  - [ ] Update the idle log line.

### 4.5 H7 docs

- [ ] `rticx-stm32h7/README.md`: shared-memory table (`+0x0000` row), the
      "initializes the shared region and releases the Cortex-M4" paragraph, and
      the expected boot log block.
- [ ] `rticx-stm32h7/renode/README.md`: the bullet that mentions
      `CrossBinBackend`/`init_shared` sequencing.

**Verify:**

```bash
cd experiments/multibinary/rticx-stm32h7
make all          # fmt-check, clippy cm7+cm4, build cm7+cm4, cargo xbin build --verify-elf
# optional, if Renode is installed:
make renode
```

---

## Phase 5 — Test harnesses in `rticx-xbin-pass/tests/`

### 5.1 `tests/init_hooks.rs`

- [ ] Update the module doc (it now proves `configure_shared_memory` +
      `init_fifos` only).
- [ ] `owner_app`: keep `test_system`, `test_configure_shared_memory`,
      `test_init_fifos`; **remove** `test_init_shared` and `test_mark_ready`.
- [ ] `write_project`'s generated `main`:
  - [ ] Keep: dirty both FIFOs, `test_init_fifos`, assert the `0->1` FIFO is
        zeroed and the `1->0` FIFO is untouched.
  - [ ] Remove: `system.state()` assertions, `test_init_shared`,
        `test_mark_ready`, epoch/magic/ready checks.
- [ ] Rename the test `mock_app_reaches_ready_state` → e.g.
      `mock_app_initializes_only_its_produced_fifos`; update its assertions
      (keep configure/init_fifos presence, keep the "no `doorbell_setup`"
      check, drop init_shared/mark_ready presence checks and the "owner ready"
      stdout check).

### 5.2 `tests/codegen.rs`

- [ ] `owner_codegen_init_hooks_snapshot`: remove the `__rticx_xbin_init_shared`
      header/body assertions and the `__rticx_xbin_mark_ready_core0`
      header + `mark_ready(0u32)` assertions. Keep
      `configure_shared_memory` and `init_fifos_core0`.
- [ ] `receiver_codegen_init_hooks_snapshot`: remove the
      `__rticx_xbin_mark_ready_core0` header + `mark_ready(1u32)` assertions.
      Keep the "no `__rticx_xbin_init_shared`" assertion and the configure
      assertion.
- [ ] `init_hooks_are_wired_into_the_entry_functions`: remove the
      `BeforePostInit` `__rticx_xbin_init_shared` assertions and the
      `BeforeIdle` `__rticx_xbin_mark_ready_core0` assertions. Keep/add the
      `BeforeInit` configure injection assertion and the `BeforePostInit`
      `init_fifos` injection assertion (`main_injection(BeforeIdle, *)` must now
      be `None`).
- [ ] Remove the `ReadyCache` snapshot assertions in the cross_spawn codegen
      test (~L500–540), replacing them with a check that the gate is absent.

### 5.3 `tests/receiver_compile.rs`

- [ ] Remove the `fn __rticx_xbin_mark_ready_core0` presence assertion and the
      `__rticx_xbin_mark_ready_core0 (& __rticx_xbin_backend ()) ;` injection
      assertion. Keep the `!expanded.contains("__rticx_xbin_init_shared")` and
      the configure-injection assertion.

### 5.4 `tests/e2e_runtime.rs`

- [ ] Update the module doc (drop the ready/epoch boot step and the M6-T2
      peer-reset scenario; describe configure + init_fifos boot and the
      distribution-owned ordering).
- [ ] `producer_module`: remove the `owner`/`init_shared` wrapper and the
      `test_mark_ready` wrapper; keep `test_configure` + `test_init_fifos`
      (apply to `producer_2` and the `expand_producer` call sites too).
- [ ] `two_app_receiver_module` idle: drop `test_init_shared` /
      `test_mark_ready` calls and the `shared_state().is_ready(...)` asserts;
      keep configure + init_fifos.
- [ ] Remove the "M6-T2 not-ready rejects the spawn" and "post-reset recovery"
      blocks (~L440–475); renumber/keep the following assertions consistent
      (the `RECEIVED` expected vector loses `request(21)`).
- [ ] `three_app_receiver_module` idle: same removals (drop
      `test_init_shared`/`test_mark_ready`/`shared_state().is_ready(...)`).
- [ ] `three_applications_spawn_through_their_own_routers`: remove the
      `!second.contains("__rticx_xbin_init_shared")` assertion
      (~L1176–1179); keep the `init_fifos` assertion.

**Verify:**

```bash
cd experiments/multibinary
cargo test -p rticx-xbin-pass
cargo test -p rticx-xbin-driver
cargo test
```

---

## Phase 6 — Documentation

- [ ] `docs/architecture.md`
  - [ ] TOC entry §8 "Boot, ready/epoch and reset" → e.g. "Boot sequencing
        (distribution-owned) and reset".
  - [ ] Component table rows for `rticx-xbin-pass` / `rticx-xbin-rt` /
        `rticx-xbin-mock` (drop `SharedState`/`ReadyCache`/ready gate, mention
        that boot sequencing is distribution-owned).
  - [ ] Rewrite §8 entirely: distribution owns boot ordering; generated code
        emits only `configure_shared_memory` (before user `init`) and
        `init_fifos_core<N>` (before `post_init`); reset handling is a
        distribution responsibility; `cross_spawn` no longer gates on target
        readiness.
  - [ ] §9 runtime-half paragraph: drop `shared_state()` and the delegated
        ready/epoch methods.
  - [ ] §10 failure-modes table: delete the "spawn before target ready" and
        "peer reset" rows; update the `Err(Some)` row.
  - [ ] §11 testing strategy: drop "ready/epoch, simulated reset".
- [ ] `docs/user-guide.md`
  - [ ] `Err(Some(input))` table (~L269): remove "target not ready".
  - [ ] Delete the "Ready state and peer reset" subsection (~L271–287).
  - [ ] Worked-example hook list (~L539–542): remove `init_shared`/`mark_ready`;
        describe `configure` + `init_fifos` and the distribution-owned boot
        handshake.
  - [ ] §7.3 "Expected boot order and readiness" (~L677–696): rewrite around
        the distribution owning the ordering; remove `init_shared`/`mark_ready`
        steps 2 and 4; keep steps for `configure_shared_memory` and
        `init_fifos`.
  - [ ] Troubleshooting table (~L901): delete the "`cross_spawn` always returns
        `Err(Some(input))` although the target booted" row.
- [ ] `multibinary-multicore-plan.md`
  - [ ] Add a short "Superseded decision" note near the top (or at §8/§9/§10)
        stating that the ready-bitmap/epoch framework was removed in favour of
        distribution-owned boot sequencing.
  - [ ] Update §8 (boot sequencing bullet, ready gate, `init_shared`/
        `mark_ready`, `ReadyCache`), §9 (runtime crate ready/epoch helpers),
        §10 (`shared_state()` + delegated methods; the "ready bitmap makes boot
        order and peer reset safe" paragraph), the failure-mode table, and the
        M2-T2 / M6-T2 milestone entries.
- [ ] `experiments/multibinary/README.md` — already being edited by the user;
      only touch if it still references ready/epoch.
- [ ] `rticx-stm32h7/README.md` + `renode/README.md` — handled in Phase 4.5.

**Verify:** `rg -n 'ReadyCache|SharedState|shared_state|init_shared|mark_ready' experiments/multibinary` returns no stale references (only intentional historical mentions in this plan file).

---

## 7 — Notes / risks to document in code

- **Latched doorbell + persistent FIFO.** Spawning before the consumer's router
  IRQ is enabled is safe only if the distribution's doorbell latches (HSEM
  status does) and the FIFO lives in shared memory. State this as the
  distribution contract in `backend.rs` and `docs/architecture.md` §8.
- **Peer reset** is now entirely distribution-owned; a distribution that does
  not handle it can consume stale FIFO state. Documented, not solved, by the
  framework.
- **No new framework hook is added** for boot release: distributions use
  `CorePassBackend::post_init` (H7/RP2040) or their own `RticPass::main_injection`.

## 8 — Out of scope

- Async cross-binary tasks.
- The out-of-tree `rticx-riscv` / `rticx-rp2040` distributions (they live in
  their own repositories; they must be updated there if they implement
  `CrossBinBackend`).
- The root RTICX workspace / `COMPATIBILITY.md` (this extension is an
  experimental standalone workspace).

## 9 — Final verification (Session D)

```bash
cd experiments/multibinary
cargo fmt --all --check
cargo test

cd rticx-stm32h7
make all
# if Renode is available: make renode

# No stale references:
rg -n 'ReadyCache|SharedState|shared_state|init_shared|mark_ready|bump_epoch|is_ready_at' \
  /home/zakaria/rtic-mc-experiments/experiments/multibinary \
  --glob '!remove-ready-epoch-plan.md'
```

Expected: empty grep (outside this plan file), all tests green, H7 `make all`
green, demo output shows the distribution-owned peer-up handshake instead of the
framework ready bit.

## 10 — Progress log

Append one line per completed session.

- (not started)
