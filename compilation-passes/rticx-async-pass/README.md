# rticx-async-pass

Async/await software-task compilation pass for the RTICX real-time
concurrency framework.

## Overview

`rticx-async-pass` extends the RTICX syntax with `#[async_task]` attributes,
adding first-class `async fn` / `.await` software tasks on top of the RTIC
priority model.  Each async task has a future that is polled by an **executor
loop** — which is itself a hardware-task, exactly like `sw_task` dispatchers
in `rticx-sw-pass`.  Tasks at the same (core, priority) share one
dispatcher/executor.

---

## Syntax (user-facing)

```rust
#[async_task(priority = 2, shared = [counter])] // core, spawn_by optional for multicore
struct Ping {
    rx: Receiver<'static, u32, 4>,
    tx: Sender<'static, u32, 4>,
}

impl RticAsyncTask for Ping {
    type InitArgs = Self;           // late-init via TaskInits
    type SpawnInput = ();           // input type for exec
    fn init(s: Self::InitArgs) -> Self { s }
    async fn exec(&mut self, _input: Self::SpawnInput) { … }
}
```

Channels are created with the `make_channel!` macro provided by `rticx-async`:

```rust
use rticx_async::{channel::{Receiver, Sender}, make_channel};

let (tx1, rx1) = make_channel!(u32, 4);
let (tx2, rx2) = make_channel!(u32, 4);
TaskInits { ping: Ping { tx: tx1, rx: rx2}, … }
```

`make_channel!` generates a static channel and a one-shot guard.  It must not
be used in re-entrant code:

```rust
fn create_ping_pong_channel() -> (Sender<'static, u32, 4>, Receiver<'static, u32, 4>) {
    make_channel!(u32, 4) // same expansion site → same `static CHANNEL`
}

#[init]
fn init() -> (Shared, TaskInits) {
    let (tx1, rx1) = create_ping_pong_channel(); // 1st call: OK
    let (tx2, rx2) = create_ping_pong_channel(); // 2nd call: PANICS
    // ...
}
```

Spawning:

```rust
let _ = Ping::spawn(());                             // same core → Result<(), Input>
let _ = Pong::spawn_from(Self::current_core(), input); // cross-core (spawn_by)
// Ok(()) on success; Err(input) if the task is already running.
// No join handle is returned.
```

---

## Architecture — what the pass generates

For each **`#[async_task]`** the pass emits:

| Artifact | Description |
|----------|-------------|
| Rewritten `#[task]` attr | `#[task(priority, shared, core, task_trait = RticAsyncTask)]` — parsed by the core pass for init, resource proxies, and SRP analysis |
| Wrapper `async fn __rticx_async_<Task>(&mut T, input)` | Type witness: calls `T::exec(task, input).await`; used by the compiler to infer the concrete future type `F` |
| `static __<Task>__PTR: ExecSlotPtr` | Opaque, non-generic pointer to the typed `ExecSlot<F>` |
| `fn __<Task>__wake()` | Waker: recovers the slot via `recover_slot(wrapper, &PTR)`, sets `pending`, pends the dispatcher |
| `impl Task { pub fn spawn(…) }` | Enqueues input + pends dispatcher; uses `recover_slot` + `try_allocate` (CAS on `running`), returns `Err(input)` if already occupied |
| `impl Task { pub fn spawn_from(…) }` | Cross-core variant (only when `spawn_by ≠ core`) |

The user's `impl RticAsyncTask { async fn exec(&mut self, input) { … } }`
block is passed through unchanged.

### Executor storage: type-witness pattern

Because the user's `async fn exec()` returns an unnameable `impl Future` type,
the future cannot be stored in a named `static mut` directly.  Instead, the
pass generates a **wrapper async fn** whose return type IS known to the
compiler:

```rust
// Generated wrapper — serves ONLY as a type witness, never called directly
async fn __rticx_async_Ping(task: &mut Ping, input: ()) {
    <Ping as RticAsyncTask>::exec(task, input).await;
}
```

The compiler infers the concrete future type `F` from this wrapper.  A
non-generic `ExecSlotPtr` stores an opaque pointer to `ExecSlot<F>`.  The
function `recover_slot(wrapper, &PTR)` uses the wrapper's type signature to
cast the opaque pointer back to `&'static ExecSlot<F>` at every use-site:

```rust
let future = __rticx_async_Ping(task, input);            // concrete Future F
let slot = recover_slot(__rticx_async_Ping, &PING_PTR);  // &ExecSlot<F>
slot.spawn(future);                                       // inline MaybeUninit<F>
slot.poll(__ping_wake);                                   // monomorphized, no vtable
```

The actual `ExecSlot<F>` lives as a local in `main()` — injected via the
`RticPass::main_injection` hook at `BeforeIdle`.  Since `main() -> !`, the
local is never dropped:

```rust
fn main() -> ! {
    // ... interrupt_free init ...
    let __ping_slot = core::mem::ManuallyDrop::new(
        ExecSlot::new_from_witness(__rticx_async_Ping)
    );
    PING_PTR.store(&*__ping_slot as *const _ as *const ());
    idle();
}
```

**Zero heap, zero `extern crate alloc`, fully monomorphized — on stable Rust.**

### `ExecSlot<F>` struct

```rust
pub struct ExecSlot<F: Future<Output = ()> + 'static> {
    future: UnsafeCell<MaybeUninit<F>>,  // inline, no Box
    running: AtomicBool,
    pending: AtomicBool,
}
```

Methods: `new()`, `new_from_witness(fn)`, `try_allocate()`, `spawn(f)`,
`poll(wake)`, `set_pending()`.

### Dispatcher

For each **(core, priority)** group the pass emits an **executor dispatcher**
— a hardware task bound to a dispatcher IRQ:

```rust
#[task(binds = TIM6, priority = 2, core = 0)]
struct Core0Priority2Dispatcher;

impl RticTask for Core0Priority2Dispatcher {
    fn init() -> Self { Self }
    fn exec(&mut self) {
        // 1. Install newly-spawned futures
        //    dequeue task ident → dequeue input → spawn(future) into ExecSlot<F>
        // 2. Poll every slot once (when pending) — monomorphized, no dyn dispatch
        // 3. On Poll::Ready: drop the future, free the slot (running ← false)
    }
}
```

The dispatcher is **waker-driven**: when a future returns `Poll::Pending`, it
has registered a waker; when a channel `send` or `recv` completes, the waker
fires, which calls the task's generated wake function.  The wake function sets
the slot's `pending` flag *and* pends the dispatcher interrupt, causing the
dispatcher ISR to run again and poll the now-ready future.  No busy-looping;
the dispatcher returns between polls.

---

## Waker design

Wakers follow upstream RTIC's bare-function-pointer pattern:

```
RawWaker.data = transmuted fn()
RawWaker.wake  = data -> fn()()
```

The generated `__<Task>__wake` function recovers the typed slot via
`recover_slot`, calls `set_pending()`, and pends the dispatcher interrupt for
the task's core.  When a channel `send` wakes a blocked `recv`, the channel
calls the task's generated wake function → the dispatcher ISR fires → the
future is polled again at the right hardware priority.

For multicore, the wake function uses the `generate_wake_pend_fn` backend hook
to optionally implement a runtime core check (local pend when called from the
same core, cross pend when called from another core).

---

## Starvation between dispatchers

Each priority level gets its own dispatcher interrupt, and each task's waker
pends only its own dispatcher.  A high-priority dispatcher can starve lower
ones — this is normal NVIC priority-preemptive behavior, identical to
hardware tasks in vanilla RTIC.

Within the same priority: all tasks share one dispatcher.  The dispatcher
polls all tasks in its group each invocation.  If TaskA's poll wakes TaskB
(same prio), the dispatcher ISR is re-pended and re-enters after returning —
effectively round-robin.

---

## `RticPass::main_injection`

The core pass generates `main()` after all passes have run.  The
`RticPass::main_injection` hook lets passes inject code at three points:

```rust
pub enum MainInjectionPoint {
    BeforeInit,      // inside interrupt_free, before system_init
    BeforePostInit,  // inside interrupt_free, before post_init
    BeforeIdle,      // after interrupt_free, before idle loop
}
```

The async pass uses `BeforeIdle` to place `ManuallyDrop<ExecSlot<F>>` locals
on `main()`'s stack.

---

## SRP (Stack Resource Policy) preservation

The async pass **does not change** SRP analysis:

- `#[async_task(shared = [counter])]` is rewritten to
  `#[task(shared = [counter], priority = 2, task_trait = RticAsyncTask)]`.
- The core pass computes the resource ceiling from `shared` declarations
  exactly as for hardware tasks.
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
| `queue_path() -> Path` | Path to the SPSC queue type for input/ready queues |
| `async_runtime_path() -> Path` | Path to the `rticx-async` re-export, e.g. `rticx_cortex_m::export::async_rt` |
| `generate_local_pend_fn(core, fn) -> ItemFn` | Fill body of the core-local interrupt-pend function |
| `generate_cross_pend_fn(core, fn) -> Option<ItemFn>` | Fill body of the cross-core pend function; `None` on single-core |
| `generate_wake_pend_fn(core, fn) -> ItemFn` | Fill body of the pend inside generated waker fns; default delegates to `generate_local_pend_fn` |
| `custom_interrupt_path(core) -> Option<Path>` | Override the interrupt-type path (default: `pac[core]::Interrupt`) |
| `subscribe(&mut self, InfoBus)` | Default no-op; guaranteed called before any other method |

---

## Multicore support

The pass inherits the sw-pass multicore model unchanged:

- `#[async_task(core = C, spawn_by = S)]` — task lives on core `C`, spawnable from core `S`.
- Each core gets its own dispatchers; cross-core and core-local task priorities must be disjoint (analysis rejects overlap).
- `spawn_from(spawner_token, input)` sends the input to the target core's queue and triggers the cross-pend mechanism.
- The future is always created **on the target core** by the target core's dispatcher — only the spawn input travels across cores.

---

## Testing strategy

- **Token-level tests** (in `tests/`) — parse, analysis, and codegen assertions
  driven by `AsyncPass::run_pass` with a mock backend.  Fast, no hardware.
- **`rticx-async` unit tests** — channel send/recv/try/full/drop, executor
  slot poll/wake/completion cycles (host-side, requiring only `std`).
- **QEMU end-to-end** — `async_ping_pong` example in `rticx-cortex-m` under
  `lm3s6965evb`; the example terminates with `debug::exit(EXIT_SUCCESS)` when
  the async tasks complete a handshake through the channel.

---

## Limitations & future work

- **Priority 0 executors** need a dispatcher IRQ assigned in `dispatchers = […]` (no implicit idle-driven loop like upstream RTIC).
- **No join handles** — `spawn()` returns `Result<(), Input>`; a spawned task cannot be awaited from the spawner.
- **No cancellation** — an in-flight future cannot be cancelled; the slot must complete naturally.
- **Cross-core channel waking** requires the backend to implement `generate_wake_pend_fn` with a runtime core check; works automatically on single-core.
- **Multicore distributor** — currently only the single-core `rticx-cortex-m` distribution implements `AsyncPassBackend`; multicore distributions (e.g. `rticx-rp2040`) need an async executor backend.

---

## License

MIT
