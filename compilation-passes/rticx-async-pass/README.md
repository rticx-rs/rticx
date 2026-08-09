# rticx-async-pass

Async/await software-task compilation pass for the RTICX real-time
concurrency framework.

## Overview

`rticx-async-pass` extends the RTICX syntax with `#[async_task]` attributes,
adding first-class `async fn` / `.await` software tasks on top of the RTIC
priority model.  Each async task has a future that is polled by an **executor
loop** — which is itself a hardware-task dispatcher, exactly like `sw_task`
dispatchers in `rticx-sw-pass`.  Tasks at the same (core, priority) share one
dispatcher/executor.

The pass is a **pre-core pass** registered by a distribution via
`RticMacroBuilder::bind_pre_core_pass`.  It rewrites `#[async_task]` structs
into `#[task(…, task_trait = RticAsyncTask)]` so that the core pass handles
initialization, resource locking, and SRP ceiling analysis unchanged.

---

## Syntax (user-facing)

```rust
// ── Attribute ───────────────────────────────────────────────────────
#[async_task(priority = 2, shared = [counter])]          // core, spawn_by optional
struct Ping {
    rx: Receiver<'static, u32, 4>,
    tx: Sender<'static, u32, 4>,
}

impl RticAsyncTask for Ping {
    type InitArgs = Self;           // → late-init via TaskInits
    type SpawnInput = ();           // input type for exec
    fn init(s: Self::InitArgs) -> Self { s }
    async fn exec(&mut self, _input: Self::SpawnInput) { … }
}
```

Channels are created with the `make_channel!` macro provided by `rticx-async`
(no `static mut`, no user `unsafe`):

```rust
use rticx_async::{channel::{Receiver, Sender}, make_channel};

let (tx, rx) = make_channel!(u32, 4);
// tx, rx: 'static — can immediately go into TaskInits
TaskInits { ping: Ping { tx, rx: … }, … }
```

Spawning:

```rust
let _ = Ping::spawn(());                         // same core → Result<(), Input>
let _ = Pong::spawn_from(core_token, input);     // cross-core (spawn_by)
// Ok(()) on success; Err(input) if the task is already running
// No join handle is returned.
```

---

## Architecture — what the pass generates

For each **`#[async_task]`** the pass emits:

| Artifact | Description |
|----------|-------------|
| Rewritten `#[task]` attr | `#[task(priority, shared, core, task_trait = RticAsyncTask)]` — parsed by the core pass for init, resource proxies, and SRP analysis |
| `static mut __<Task>__EXEC: ExecSlot` | Future storage slot (one per task) |
| `static mut __<Task>__INPUTS: Queue<SpawnInput, 2>` | SPSC queue carrying spawn inputs |
| `fn __<Task>__wake()` | RawWaker callback: sets the pending flag and pends the dispatcher interrupt |
| `impl Task { pub fn spawn(…) }` | Enqueues input + pends dispatcher; uses `try_allocate` (CAS on `running` flag), returns `Err(input)` if already occupied |
| `impl Task { pub fn spawn_from(…) }` | Cross-core variant (only when `spawn_by ≠ core`) |

The user's `impl RticAsyncTask { async fn exec(&mut self, input) { … } }`
block is passed through unchanged; the dispatcher calls
`RticAsyncTask::exec(&mut TASK, input)`.

For each **(core, priority)** group the pass emits an **executor dispatcher**
— a hardware task bound to a dispatcher IRQ:

```rust
#[task(binds = TIM6, priority = 2, core = 0)]
struct Core0Priority2Dispatcher;

impl RticTask for Core0Priority2Dispatcher {
    fn init() -> Self { Self }
    fn exec(&mut self) {
        // 1. Install newly-spawned futures
        //    dequeue task ident → dequeue input → Box::pin(RticAsyncTask::exec(…))
        // 2. Poll every slot once (when pending)
        // 3. On Poll::Ready: drop the future, free the slot (running ← false)
        // 4. If any slot is still running → re-pend myself
    }
}
```

The dispatcher self-pends until all futures complete (the executor keeps the
hardware-task ISR alive as long as work remains).

---

## Future storage — `ExecSlot` in `rticx-async`

Because `async fn exec(&mut self, input)` returns an unnameable `impl Future`
type, the future cannot be stored in a named `static` directly.  The pass uses
**heap allocation managed at the distribution level**:

- Each task gets one `ExecSlot` holding `Option<Pin<Box<dyn Future<Output=()> + 'static>>>`.
- `try_allocate` → CAS `running` (AcqRel) → reserves the slot.
- `install` → `Box::pin(future)` and writes it into the slot (called by the dispatcher on the target core).
- `poll(wake: fn())` → checks `pending` (Acquire CAS clear), builds a `Waker` from the bare `fn()` pointer (clone/drop no-ops, same pattern as upstream RTIC), polls the pinned future; on `Poll::Ready` drops the box and marks `running = false`.
- Atomistic in `portable_atomic::AtomicBool` — works on both armv7-m (native CAS) and armv6-m (critical-section fallback).

The **distribution** provides a hidden global allocator (e.g. `embedded-alloc::LlffHeap`)
and calls its init from the core backend's `post_init` hook when the pass is
detected on the `InfoBus`.  End-user code never sees a `#[global_allocator]` or
heap configuration.

---

## Waker design

Wakers follow upstream RTIC's bare-function-pointer pattern:

```
RawWaker.data = transmuted fn()
RawWaker.wake  = data → fn()()
```

The generated `__<Task>__wake` function sets the slot's `pending` flag and pends the
dispatcher interrupt for the task's core.  When a channel `send` wakes a blocked
`recv`, the channel calls the task's generated wake function → the dispatcher ISR
fires → the future is polled again at the right hardware priority.

For multicore, the wake function uses the `generate_wake_pend_fn` backend hook to
optionally implement a runtime core check (local pend when called from the same
core, cross pend when called from another core).

---

## SRP (Stack Resource Policy) preservation

The async pass **does not change** SRP analysis:

- `#[async_task(shared = [counter])]` is rewritten to
  `#[task(shared = [counter], priority = 2, task_trait = RticAsyncTask)]`.
- The core pass computes the resource ceiling from `shared` declarations exactly
  as for hardware tasks.
- The generated `shared()` proxy and its `lock(|r| …)` body use the same
  BASEPRI / source-masking backend hooks.
- The future is polled inside the dispatcher ISR at the **task's hardware
  priority** — ceiling elevation works identically to a sync task's `exec()`.
- The closure-based `lock` API makes it structurally impossible to hold a
  resource guard across an `.await` point.

Preemption is unchanged: the dispatcher is a normal interrupt and can be
preempted by higher-priority interrupts; the future's polling simply resumes
when the dispatcher re-enters.

---

## `AsyncPassBackend` trait

Distribution proc-macro crates implement `AsyncPassBackend` and pass it to
`AsyncPass::new`.

| Method | Purpose |
|--------|---------|
| `queue_path() -> Path` | Path to the SPSC queue type for input/ready queues (same as `SwPassBackend`) |
| `async_runtime_path() -> Path` | **New** — path to the `rticx-async` re-export, e.g. `rticx_cortex_m::export::async_rt` |
| `generate_local_pend_fn(core, fn) -> ItemFn` | Fill body of the core-local interrupt-pend function called by `spawn()` and wakers |
| `generate_cross_pend_fn(core, fn) -> Option<ItemFn>` | Fill body of the cross-core pend function for `spawn_from()`; return `None` on single-core targets |
| `generate_wake_pend_fn(core, fn) -> ItemFn` | **New** — fill body of the pend inside generated waker fns; default delegates to `generate_local_pend_fn` |
| `custom_interrupt_path(core) -> Option<Path>` | Override the interrupt-type path (default: `pac[core]::Interrupt`) |
| `subscribe(&mut self, InfoBus)` | Default no-op; guaranteed called before any other method |

---

## Multicore support

The pass inherits the sw-pass multicore model unchanged:

- `#[async_task(core = C, spawn_by = S)]` — task lives on core `C`, spawnable
  from core `S`.
- Each core gets its own dispatchers; cross-core and core-local task priorities
  must be disjoint (analysis rejects overlap).
- `spawn_from(spawner_token, input)` sends the input to the target core's queue
  and triggers the cross-pend mechanism.
- The future is always created **on the target core** by the target core's
  dispatcher — only the spawn input travels across cores.

---

## Testing strategy

- **Token-level tests** (in `tests/`) — parse, analysis, and codegen assertions
  driven by `AsyncPass::run_pass` with a mock backend.  Fast, no hardware.
- **`rticx-async` unit tests** — channel send/recv/try/full/drop, executor slot
  poll/wake/completion cycles (host-side, requiring only `std`).
- **QEMU end-to-end** — `async_ping_pong` example in `rticx-cortex-m` under
  `lm3s6965evb`; the example terminates with `debug::exit(EXIT_SUCCESS)` when
  the two async tasks complete a handshake through the channel.

---

## Limitations & future work

- **No join handles** — `spawn()` returns `Result<(), Input>`; a spawned task
  cannot be awaited from the spawner.
- **No cancellation** — an in-flight future cannot be cancelled; the slot must
  complete naturally.
- **Heap required** — future storage uses `Box`; a future iteration could
  provide fixed-capacity inline slots with a compile-time size assertion.
- **Priority 0 executors** need a dispatcher IRQ assigned in `dispatchers = […]`
  (no implicit idle-driven loop like upstream RTIC).
- **Cross-core channel waking** requires the backend to implement
  `generate_wake_pend_fn` with a runtime core check; works automatically on
  single-core.

---

## License

MIT
