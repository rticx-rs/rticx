# rticx-async-pass

Async/await software-task compilation pass for the RTICX real-time
concurrency framework.

## Overview

`rticx-async-pass` extends the RTICX syntax with `#[async_task]` attributes,
adding first-class `async fn` / `.await` software tasks on top of the RTIC
priority model.  Each async task has a future that is polled by an **executor
loop** — which is itself a hardware-task, exactly like `sw_task` dispatchers 
in `rticx-sw-pass`. Also tasks at the same (core, priority) share one
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

`make_channel!` generates static definition for a channel and as a result it should not be used in re-entrant code:
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
let _ = Ping::spawn(());                         // same core -> Result<(), Input>
let _ = Pong::spawn_from(Self::current_core(), input);     // cross-core (spawn_by)
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
    }
}
```

The dispatcher is **waker-driven**: when a future returns `Poll::Pending`, it
has registered a waker; when a channel `send` or `recv` completes, the waker
fires, which calls the task's generated wake function. The wake function sets
the slot's `pending` flag *and* pends the dispatcher interrupt, causing the
dispatcher ISR to run again and poll the now-ready future. No busy-looping;
the dispatcher returns between polls. This way a high priority dispatcher would 
not starve a low priority dispatcher when the tasks bound to it are not ready. 

---

## Future storage — `ExecSlot<F>` in `rticx-async`

The pass uses a **type-witness pattern** (borrowed from upstream RTIC) to
achieve monomorphized inline future storage with **zero heap allocation**.

### Architecture

For each async task, the pass generates three artifacts:

1. **A wrapper `async fn`** — calls the user's `exec().await`, serves as a
   type witness (zero-cost, never called directly):
   ```rust
   async fn __rticx_async_Ping(task: &mut Ping, input: ()) {
       <Ping as RticAsyncTask>::exec(task, input).await;
   }
   ```

2. **An `ExecSlotPtr`** — opaque pointer static (non-generic):
   ```rust
   static PING_PTR: ExecSlotPtr = ExecSlotPtr::new();
   ```

3. **A slot local in `main()`** — injected via the `RticPass::main_injection`
   hook at `BeforeIdle`. Since `main() -> !`, the local lives forever on the
   stack:
   ```rust
   // injected into main() before the idle loop
   let __ping_slot = core::mem::ManuallyDrop::new(
       ExecSlot::new_from_witness(__rticx_async_Ping)
   );
   PING_PTR.store(&*__ping_slot as *const _ as *const ());
   ```

At the dispatch site, the pointer is recovered with the concrete type via the
same witness function:

```rust
let future = __rticx_async_Ping(task, input);            // concrete future
let slot = recover_slot(__rticx_async_Ping, &PING_PTR);  // &ExecSlot<F>
slot.spawn(future);                                       // inline MaybeUninit<F>
slot.poll(__ping_wake);                                   // monomorphized poll
```

The type `F` is inferred by the compiler from the wrapper async fn's return
type — the proc-macro never needs to name it.

### Key properties

| Property | Detail |
|----------|--------|
| Heap allocation | **None** — zero `Box`, zero `extern crate alloc` |
| Per-spawn allocation | **None** — future stored inline in `MaybeUninit<F>` |
| Dispatch | **Monomorphized** — no `dyn Future` vtable |
| Memory | Executor lives on `main()`'s stack frame (`-> !`, never dropped) |
| Stable Rust | Yes |

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

## Heap allocation lifecycle

**Allocation**: `Box::pin(future)` inside `ExecSlot::install()` (called by the dispatcher ISR). Uses a 2048-byte `embedded-alloc::Heap` (bump allocator) configured as `#[global_allocator]`.

**Freeing**: When `poll()` returns `Poll::Ready(())`, the code does `*self.future.get() = None`, which drops the `Pin<Box<dyn Future>>`. The `Box` deallocator runs, but with a bump allocator, individual `dealloc` calls are typically no-ops — the heap space isn't truly reclaimed.

**Why fragmentation isn't an issue**: Each task has exactly one `ExecSlot`, and `try_allocate()` (CAS) ensures only one future per slot at a time. When a future completes, the slot becomes IDLE, and the next `spawn()` of the same task allocates a new `Box::pin()` into the same slot position. The bump pointer doesn't advance because old futures are dropped, so the allocator reuses the same heap region. No fragmentation for same-size futures.

**Overflow prevention**: Fixed 2048-byte heap. If all concurrent futures exceed that, `Box::pin()` panics (OOM). TODO: we need to improve this later so that heap size is adjustable by the user + `spawn`/`spawn_from` return out of memory error.

### Why not purely static allocation?

The wrapper-fn type-witness approach places executors on `main()`'s stack,
achieving zero-heap static storage.  This works because:

1. The `RticPass::main_injection` hook allows passes to inject code into
   `main()` at 3 injection points (`BeforeInit`, `BeforePostInit`, `BeforeIdle`).
2. The async pass injects slot creation as `let` bindings at `BeforeIdle` —
   these locals live in `main() -> !` and are never dropped.
3. `ManuallyDrop` suppresses drop glue, and the opaque pointer is stored in a
   static `ExecSlotPtr`.

No `Box`, no heap, no `extern crate alloc`.  This pattern is equivalent to
upstream RTIC's approach of placing `AsyncTaskExecutor` instances in `main()`.

## Starvation between dispatchers
Correct by design. Each priority level gets its own dispatcher interrupt, and each task's waker pends only its own dispatcher. A high-priority dispatcher can starve lower ones — this is normal NVIC priority-preemptive behavior, identical to vanilla RTIC hardware tasks. If a prio-3 task keeps reawakening, the CPU serves it ahead of prio-2 tasks, which is correct.
Within the same priority: all tasks share one dispatcher. The dispatcher polls all tasks in its group each invocation. If TaskA's poll wakes TaskB (same prio), the dispatcher ISR is re-pended and re-enters after returning — effectively round-robin.

---

## Waker design

Wakers follow upstream RTIC's bare-function-pointer pattern:

```
RawWaker.data = transmuted fn()
RawWaker.wake  = data -> fn()()
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

- `#[async_task(shared = [counter])]` is rewritten to  `#[task(shared = [counter], priority = 2, task_trait = RticAsyncTask)]`.
- The core pass computes the resource ceiling from `shared` declarations exactly as for hardware tasks.
- The generated `shared()` proxy and its `lock(|r| …)` body use the same BASEPRI / source-masking backend hooks.
- The future is polled inside the dispatcher ISR at the **task's hardware priority** — ceiling elevation works identically to a sync task's `exec()`.
- The closure-based `lock` API makes it structurally impossible to hold a resource guard across an `.await` point.

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

- `#[async_task(core = C, spawn_by = S)]` — task lives on core `C`, spawnable from core `S`.
- Each core gets its own dispatchers; cross-core and core-local task priorities must be disjoint (analysis rejects overlap).
- `spawn_from(spawner_token, input)` sends the input to the target core's queue and triggers the cross-pend mechanism.
- The future is always created **on the target core** by the target core's dispatcher — only the spawn input travels across cores.

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

- **No join handles** — `spawn()` returns `Result<(), Input>`; a spawned task cannot be awaited from the spawner.
- **No cancellation** — an in-flight future cannot be cancelled; the slot must complete naturally.
- **Heap required for init** — none; executors live on `main()`'s stack via `main_injection`. See [Why not purely static?](#why-not-purely-static-allocation).
- **Priority 0 executors** need a dispatcher IRQ assigned in `dispatchers = […]` (no implicit idle-driven loop like upstream RTIC).
- **Cross-core channel waking** requires the backend to implement `generate_wake_pend_fn` with a runtime core check; works automatically on single-core.

---

## License

MIT
