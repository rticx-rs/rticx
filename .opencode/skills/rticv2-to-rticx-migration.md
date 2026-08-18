# RTICv2 to RTICX Migration Guide

Comprehensive reference and step-by-step procedure for porting applications from **RTIC v2** (`rtic` v2.x) to **RTICX** (`rticx-*`).

This skill serves as both an automated reference for AI coding agents and an authoritative porting guide for developers.

---

## 1. Core Architectural Differences

| Feature / Concept | RTIC v2 | RTICX |
|---|---|---|
| **Crate Model** | Monolithic `rtic` crate with backend features | Modular distribution crates (`rticx-cortex-m`, `rticx-rp2040`, `rticx-riscv`) |
| **App Macro** | `#[rtic::app(...)]` | `#[<distro>::app(...)]` (e.g. `#[rticx_cortex_m::app(...)]`) |
| **`peripherals` attribute** | `peripherals = true / false` | **Removed / Unsupported** (acquire via `pac::Peripherals::take().unwrap()`) |
| **Task Representation** | Free functions annotated with `#[task]` | **Structs** implementing traits (`RticTask`, `RticSwTask`, `RticAsyncTask`, `RticIdleTask`) |
| **Local Task State** | `#[local] struct Local` & `#[task(local = [...])]` | **Fields of the Task Struct** initialized via `TaskInits` |
| **Shared Resource Access** | `cx.shared.my_resource.lock(\|r\| ...)` | `self.shared().my_resource.lock(\|r\| ...)` |
| **Task Initialization** | `init` returns `(Shared, Local)` | `init` returns `(SharedResources, TaskInits)` |
| **Startup Spawning** | `my_task::spawn(...).unwrap()` inside `#[init]` | **Forbidden in `#[init]`**; done in **`#[post_init]`** |
| **External Tasks** | `extern "Rust" { #[task] ... }` block inside `app` | Zero boilerplate: `impl RticTask for MyTask` in external module |
| **Async Tasks** | `#[task] async fn foo(cx: ...)` | `#[async_task(...)] struct Foo;` + `impl RticAsyncTask for Foo` |
| **Sync Software Tasks** | (Deprecated/merged into async) | Dedicated lightweight `#[sw_task(...)]` + `impl RticSwTask` |
| **Async Channels** | `rtic_sync::channel::{Sender, Receiver}` | `rticx_async::channel::{Sender, Receiver}` |
| **Monotonics** | `rtic-monotonics` | Upstream `rtic-monotonics` (fully compatible) |
| **Multicore** | Experimental / out-of-tree | Native single-binary multicore (`cores = N`, `spawn_by = N`, `cross_spawn`) |

---

## 2. Distribution Selection & Dependencies (`Cargo.toml`)

### 2.1. Selecting the Distribution

RTICX uses separate, target-specific distribution crates rather than a single monolithic crate.

> [!IMPORTANT]
> **LLM Rule:** If the target architecture or distribution is not explicitly provided by the user or evident from the existing crate PAC, the LLM **must ask the user** which distribution to target before performing the migration.

| Target Platform | RTICX Distribution | Repository / Location |
|---|---|---|
| Single-core Cortex-M (ARMv7-M, ARMv6-M) | `rticx-cortex-m` | `distributions/rticx-cortex-m/` |
| Dual-core Cortex-M0+ (RP2040) | `rticx-rp2040` | `https://github.com/rticx-rs/rticx-rp2040` |
| Single-core RISC-V (SLIC, ESP32-C3, ESP32-C6) | `rticx-riscv` | `https://github.com/rticx-rs/rticx-riscv` |

### 2.2. Dependency Mapping

Update `Cargo.toml` dependencies:

```toml
# --- REMOVE RTIC v2 DEPENDENCIES ---
# rtic = { version = "2.1.1", features = ["thumbv7-backend"] }
# rtic-sync = "1.3.0"
# rtic-common = "1.3.0"

# --- ADD RTICX DEPENDENCIES ---

# 1. Distribution crate (select based on target)
# Example for Cortex-M:
rticx-cortex-m = { version = "0.1", features = ["async"] }

# Example for RP2040:
# rticx-rp2040 = { version = "0.1", features = ["async"] }

# Example for RISC-V (ESP32-C3):
# rticx-riscv = { version = "0.1", default-features = false, features = ["esp32c3", "async"] }

# 2. Async runtime support (REQUIRED when using async tasks or channels)
rticx-async = "0.2"

# 3. Monotonics (identical to RTIC v2)
rtic-monotonics = { version = "2.2.1", features = ["cortex-m-systick"] } # or esp32c3-systimer, rp2040, etc.
```

### 2.3. Mandatory Feature Flags

- **`async`**: RTIC v2 applications extensively use `async fn` tasks. In RTICX, you **must enable the `async` feature** on the distribution crate and include `rticx-async` in `Cargo.toml`.
- **`swtasks`**: If the application uses synchronous software tasks (`#[sw_task]`), enable the `swtasks` feature on the distribution crate.
- **`armv6m`**: If building `rticx-cortex-m` for ARMv6-M (Cortex-M0/M0+), enable `features = ["armv6m"]` (enables interrupt source-masking locks instead of BASEPRI).
- **Target selector features on `rticx-riscv`**: Must enable one of `slic`, `esp32c3`, or `esp32c6`.

---

## 3. Application Attribute (`#[app]`)

### 3.1. Attribute Transformation

```rust
// --- RTIC v2 ---
#[rtic::app(device = lm3s6965, peripherals = true, dispatchers = [SSI0])]
mod app { ... }

// --- RTICX ---
#[rticx_cortex_m::app(device = lm3s6965, dispatchers = [SSI0])]
mod app { ... }
```

### 3.2. Why `peripherals` is Removed & How Tasks Access Peripherals

RTICX intentionally removes the `peripherals = true/false` attribute.
In RTIC v2, `peripherals = true` injected `cx.device` (the PAC peripherals) and `cx.core` into `init`. In RTICX, this attribute is unsupported and generates a compile error.

#### 1. Peripheral Acquisition in `#[init]`
In RTICX, acquire PAC peripherals directly inside `#[init]`:
```rust
#[init]
fn system_init() -> (Shared, TaskInits) {
    let dp = pac::Peripherals::take().unwrap();
    let cp = unsafe { cortex_m::Peripherals::steal() };
    // Configure clocks, GPIOs, peripherals...
```

#### 2. Exclusive (Task-Private) Peripherals $\rightarrow$ Pass via `TaskInits`
If a peripheral or driver instance is exclusively used by a single task (e.g., `uart_rx`, `timer_channel`, `spi_device`), pass it directly to that task struct at initialization via `TaskInits`:
```rust
#[task(binds = UART0_IRQ, priority = 3)]
pub struct UartRxTask {
    pub uart_rx: UartRx,
}

impl RticTask for UartRxTask {
    fn exec(&mut self) {
        let mut byte = [0u8; 1];
        let _ = self.uart_rx.read(&mut byte);
    }
}
```

#### 3. Shared Peripherals $\rightarrow$ Place in `#[shared]` & Access with `.lock(...)`
If multiple tasks need access to the same peripheral or driver (e.g., `uart_tx` for logging, an `i2c_bus`, or shared hardware timer), place the peripheral inside the `#[shared]` resources struct and access it inside tasks using `self.shared().peripheral.lock(|p| { ... })`:
```rust
#[shared]
struct Shared {
    uart_tx: UartTx,
    alarm: Alarm0,
}

#[task(binds = TIMER_IRQ_0, priority = 2, shared = [uart_tx, alarm])]
pub struct AlarmTask;

impl RticTask for AlarmTask {
    fn exec(&mut self) {
        self.shared().uart_tx.lock(|tx| {
            tx.write_full_blocking(b"Alarm fired!\r\n");
        });
    }
}
```


---

## 4. Initialization & Task Construction (`#[init]`, `TaskInits`, `#[post_init]`)

### 4.1. `#[init]` Return Signature

- **RTIC v2**: returned `(Shared, Local)`.
- **RTICX**: returns `(Shared, TaskInits)` (or `TaskInits` alone if no `#[shared]` struct exists).

```rust
#[init]
fn system_init() -> (Shared, TaskInits) {
    // 1. Hardware initialization...
    
    // 2. Return shared resources and initialized task structs
    (
        Shared { counter: 0 },
        TaskInits {
            worker: Worker { rx: rx_chan },
            blinker: Blinker::new(led_pin),
        },
    )
}
```

### 4.2. `TaskInits` vs. `init = generated`

Every task in RTICX is a struct. Tasks must be constructed during initialization:

1. **Stateful Tasks**: Pass the initialized struct in `TaskInits`. The field name in `TaskInits` is the `snake_case` version of the task struct name.
2. **Stateless / Unit Struct Tasks**: Add `init = generated` to the task attribute to let RTICX construct it automatically. It will be **omitted** from `TaskInits`:
   ```rust
   #[task(binds = SysTick, priority = 1, init = generated)]
   struct Tick;
   
   // Tick is constructed automatically by RTICX — no entry needed in TaskInits!
   ```

### 4.3. CRITICAL: Spawning in `#[init]` is Forbidden

> [!CAUTION]
> In RTICX, **calling `Task::spawn(...)` inside `#[init]` will always fail and return `Err(input)`** because the dispatcher queues and executors are not yet active while `init` executes.

### 4.4. Migration to `#[post_init]`

Move all startup spawns from `#[init]` to an optional `#[post_init]` function:

```rust
// --- RTIC v2 ---
#[init]
fn init(cx: init::Context) -> (Shared, Local) {
    foo::spawn().unwrap(); // Spawned inside init
    (Shared {}, Local {})
}

// --- RTICX ---
#[init]
fn system_init() -> (Shared, TaskInits) {
    (Shared {}, TaskInits { foo: Foo })
}

#[post_init]
fn post_init() {
    let _ = Foo::spawn(()); // Spawned in post_init once the system is ready!
}
```

---

## 5. Tasks: Converting Free Functions to Structs + Traits

RTICX replaces free-function task handlers with **Structs that implement RTICX Traits**.

### 5.1. Hardware Tasks (`#[task(...)]`)

| RTIC v2 | RTICX |
|---|---|
| `#[task(binds = UART0, priority = 1, shared = [counter])]`<br>`fn uart0(mut cx: uart0::Context) {`<br>&nbsp;&nbsp;&nbsp;&nbsp;`cx.shared.counter.lock(\|c\| *c += 1);`<br>`}` | `#[task(binds = UART0, priority = 1, shared = [counter], init = generated)]`<br>`struct Uart0;`<br><br>`impl RticTask for Uart0 {`<br>&nbsp;&nbsp;&nbsp;&nbsp;`fn exec(&mut self) {`<br>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;`self.shared().counter.lock(\|c\| *c += 1);`<br>&nbsp;&nbsp;&nbsp;&nbsp;`}`<br>`}` |

### 5.2. Async Software Tasks (`#[async_task(...)]`)

| RTIC v2 | RTICX |
|---|---|
| `#[task(priority = 2, shared = [counter])]`<br>`async fn worker(mut cx: worker::Context, val: u32) {`<br>&nbsp;&nbsp;&nbsp;&nbsp;`cx.shared.counter.lock(\|c\| *c += val);`<br>`}` | `#[async_task(priority = 2, shared = [counter], capacity = 4, init = generated)]`<br>`struct Worker;`<br><br>`impl RticAsyncTask for Worker {`<br>&nbsp;&nbsp;&nbsp;&nbsp;`type SpawnInput = u32;`<br>&nbsp;&nbsp;&nbsp;&nbsp;`async fn exec(&mut self, val: u32) {`<br>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;`self.shared().counter.lock(\|c\| *c += val);`<br>&nbsp;&nbsp;&nbsp;&nbsp;`}`<br>`}` |

### 5.3. Synchronous Software Tasks (`#[sw_task(...)]`)

For low-overhead, non-async message-passing tasks, use `#[sw_task]`:

```rust
#[sw_task(priority = 2, shared = [counter], capacity = 4, init = generated)]
struct Worker;

impl RticSwTask for Worker {
    type SpawnInput = u32;
    fn exec(&mut self, val: u32) {
        self.shared().counter.lock(|c| *c += val);
    }
}
```

### 5.4. Idle Task (`#[idle]`)

| RTIC v2 | RTICX |
|---|---|
| `#[idle]`<br>`fn idle(cx: idle::Context) -> ! {`<br>&nbsp;&nbsp;&nbsp;&nbsp;`loop { cortex_m::asm::wfi(); }`<br>`}` | `#[idle(init = generated)]`<br>`struct Idle;`<br><br>`impl RticIdleTask for Idle {`<br>&nbsp;&nbsp;&nbsp;&nbsp;`fn exec(&mut self) -> ! {`<br>&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;`loop { cortex_m::asm::wfi(); }`<br>&nbsp;&nbsp;&nbsp;&nbsp;`}`<br>`}` |

### 5.5. Priority 0 Async Tasks (Idle Executor)

In RTICX, if you have background async work running in the idle priority level, declare it as `#[async_task(priority = 0)]`:
```rust
#[async_task(priority = 0, init = generated)]
struct Background;

impl RticAsyncTask for Background {
    type SpawnInput = ();
    async fn exec(&mut self, _input: ()) {
        loop {
            Mono::delay(1000.millis()).await;
        }
    }
}
```
> [!NOTE]
> Priority-0 async tasks are polled by a framework-generated idle loop. Defining both a custom `#[idle]` task and a `priority = 0` async task simultaneously is a compile error.

---

## 6. Local State: Eliminating `#[local]`

In RTIC v2, task-local variables were declared in `#[local] struct Local { ... }` or in `local = [x: u32 = 0]` attributes.
In RTICX, **`#[local]` is completely eliminated**. Task state is simply represented as **struct fields**:

```rust
// --- RTIC v2 ---
#[task(binds = UART0, local = [times: u32 = 0])]
fn uart0(cx: uart0::Context) {
    *cx.local.times += 1;
}

// --- RTICX ---
#[task(binds = UART0, priority = 1)]
struct Uart0 {
    times: u32,
}

impl RticTask for Uart0 {
    fn exec(&mut self) {
        self.times += 1;
    }
}

// In #[init]:
#[init]
fn system_init() -> (Shared, TaskInits) {
    (Shared {}, TaskInits { uart0: Uart0 { times: 0 } })
}
```

---

## 7. Async Channels (`rticx-async`)

Channels migrate directly from `rtic-sync` to `rticx-async`:

```rust
// --- RTIC v2 ---
use rtic_sync::{channel::{Receiver, Sender}, make_channel};

// --- RTICX ---
use rticx_async::{channel::{Receiver, Sender}, make_channel};
```

Channels are constructed in `#[init]` via `make_channel!(Type, CAPACITY)` and passed to tasks via `TaskInits`:

```rust
#[init]
fn system_init() -> (Shared, TaskInits) {
    let (tx, rx) = make_channel!(u32, 4);

    (
        Shared,
        TaskInits {
            sender: SenderTask { tx },
            receiver: ReceiverTask { rx },
        },
    )
}
```

---

## 8. External Tasks: Zero Boilerplate Migration

### 8.1. RTIC v2 Approach (Clunky `extern "Rust"`)

In RTIC v2, splitting tasks into external files required declaring `extern "Rust"` blocks inside the app module:
```rust
// RTIC v2: In main.rs
#[rtic::app(device = ...)]
mod app {
    extern "Rust" {
        #[task]
        async fn external_worker(cx: external_worker::Context);
    }
}

// RTIC v2: In external_file.rs
pub async fn external_worker(cx: app::external_worker::Context<'_>) { ... }
```

### 8.2. RTICX Approach (Native Trait Implementation)

RTICX supports external tasks with standard Rust syntax without any special macro attributes:

**In `main.rs` (`app` module):**
```rust
#[rticx_cortex_m::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6])]
pub mod app {
    // 1. Declare the task struct inside the app module
    #[task(binds = USART1, priority = 2, shared = [uart_tx])]
    pub struct UartRxTask {
        pub rx_buffer: heapless::Vec<u8, 64>,
    }
}
```

**In `src/external_uart.rs`:**
```rust
use crate::app::{UartRxTask, RticTask, RticMutex};

impl RticTask for UartRxTask {
    fn exec(&mut self) {
        self.shared().uart_tx.lock(|tx| {
            // Access shared resources and self fields naturally!
        });
    }
}
```

---

## 9. Comprehensive Porting Walkthrough Example

### Before: RTIC v2 Application

```rust
//! RTIC v2 Example
#![no_std]
#![no_main]

use panic_halt as _;
use rtic_monotonics::systick::prelude::*;
use rtic_sync::{channel::*, make_channel};

systick_monotonic!(Mono, 1000);

#[rtic::app(device = stm32f0::stm32f0x0, peripherals = true, dispatchers = [TIM6])]
mod app {
    use super::*;

    #[shared]
    struct Shared {
        counter: u32,
    }

    #[local]
    struct Local {}

    const CAPACITY: usize = 4;

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        let mut cp = cx.core;
        Mono::start(cp.SYST, 10_000_000);

        let (tx, rx) = make_channel!(u32, CAPACITY);

        receiver::spawn(rx).unwrap();
        sender::spawn(tx).unwrap();

        (Shared { counter: 0 }, Local {})
    }

    #[task(shared = [counter])]
    async fn sender(mut cx: sender::Context, mut tx: Sender<'static, u32, 4>) {
        for i in 0..10 {
            tx.send(i).await.unwrap();
            cx.shared.counter.lock(|c| *c += 1);
            Mono::delay(100.millis()).await;
        }
    }

    #[task]
    async fn receiver(_cx: receiver::Context, mut rx: Receiver<'static, u32, 4>) {
        while let Ok(val) = rx.recv().await {
            // Process val
        }
    }
}
```

### After: Ported RTICX Application

```rust
//! Ported to RTICX
#![no_std]
#![no_main]

use panic_halt as _;
use rtic_monotonics::systick::prelude::*;
use rticx_async::{channel::*, make_channel};

systick_monotonic!(Mono, 1000);

#[rticx_cortex_m::app(device = stm32f0::stm32f0x0, dispatchers = [TIM6])]
mod app {
    use super::*;

    #[shared]
    struct Shared {
        counter: u32,
    }

    #[init]
    fn system_init() -> (Shared, TaskInits) {
        let cp = unsafe { cortex_m::Peripherals::steal() };
        Mono::start(cp.SYST, 10_000_000);

        let (tx, rx) = make_channel!(u32, 4);

        (
            Shared { counter: 0 },
            TaskInits {
                sender: SenderTask { tx },
                receiver: ReceiverTask { rx },
            },
        )
    }

    #[post_init]
    fn post_init() {
        let _ = SenderTask::spawn(());
        let _ = ReceiverTask::spawn(());
    }

    #[async_task(priority = 1, shared = [counter])]
    pub struct SenderTask {
        pub tx: Sender<'static, u32, 4>,
    }

    impl RticAsyncTask for SenderTask {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            for i in 0..10 {
                let _ = self.tx.send(i).await;
                self.shared().counter.lock(|c| *c += 1);
                Mono::delay(100.millis()).await;
            }
        }
    }

    #[async_task(priority = 1)]
    pub struct ReceiverTask {
        pub rx: Receiver<'static, u32, 4>,
    }

    impl RticAsyncTask for ReceiverTask {
        type SpawnInput = ();
        async fn exec(&mut self, _input: ()) {
            while let Ok(_val) = self.rx.recv().await {
                // Process val
            }
        }
    }
}
```

---

## 10. Migration Step-by-Step Checklist

When porting an RTIC v2 project or example to RTICX, follow these steps systematically:

1. **Clarify Distribution Target**:
   - Determine the hardware target. If ambiguous, ask the user whether to use `rticx-cortex-m`, `rticx-rp2040`, or `rticx-riscv`.
2. **Update `Cargo.toml`**:
   - Replace `rtic = "2.x"` with the target `rticx-<distro>`.
   - Enable `features = ["async"]` if async tasks/channels are used.
   - Enable `features = ["swtasks"]` if synchronous software tasks are used.
   - Replace `rtic-sync` with `rticx-async = "0.2"`.
   - Keep `rtic-monotonics` with the appropriate timer feature.
3. **Migrate `#[app]` Attribute**:
   - Change `#[rtic::app(...)]` to `#[<distro>::app(...)]`.
   - Remove `peripherals = true/false`.
   - Ensure `dispatchers = [...]` contains enough interrupts for the priority levels used.
4. **Convert Task Functions to Task Structs**:
   - Hardware tasks: `struct Name;` + `impl RticTask for Name { fn exec(&mut self) { ... } }`.
   - Sync software tasks: `struct Name;` + `impl RticSwTask for Name { type SpawnInput = T; fn exec(&mut self, input: T) { ... } }`.
   - Async software tasks: `struct Name;` + `impl RticAsyncTask for Name { type SpawnInput = T; async fn exec(&mut self, input: T) { ... } }`.
   - Idle tasks: `struct Name;` + `impl RticIdleTask for Name { fn exec(&mut self) -> ! { ... } }`.
5. **Migrate State & `#[local]`**:
   - Move local variables to fields on the respective task struct.
   - Remove the `#[local]` struct.
6. **Migrate Resource Locking**:
   - Replace `cx.shared.res.lock(...)` with `self.shared().res.lock(...)`.
7. **Migrate `#[init]`, Peripherals, and `TaskInits`**:
   - Update return type to `(Shared, TaskInits)`.
   - Acquire PAC peripherals directly (`pac::Peripherals::take().unwrap()`).
   - Pass exclusive peripherals/drivers directly to task structs via `TaskInits`.
   - Put shared peripherals (used across multiple tasks) into `#[shared]` and access via `.lock(...)`.
   - Construct stateful task structs in `TaskInits`.
   - Add `init = generated` to unit/empty task structs.
8. **Relocate Startup Spawns to `#[post_init]`**:
   - Remove any `Task::spawn(...)` from `#[init]`.
   - Create `#[post_init] fn post_init() { Task::spawn(...); }`.
9. **Migrate External Tasks**:
   - Remove `extern "Rust"` blocks.
   - Simply implement `RticTask` / `RticSwTask` / `RticAsyncTask` in the external module.
10. **Build and Validate**:
    - Run `cargo check` / `cargo build`.
    - Fix any dispatcher capacity or priority mismatches.

---

## 11. Common Pitfalls & Troubleshooting

- **Symptom: Spawning in `init` returns `Err(...)`**
  - *Cause*: Attempting to spawn tasks inside `#[init]`.
  - *Fix*: Move all spawn calls to `#[post_init]`.
- **Symptom: Unknown attribute `peripherals`**
  - *Cause*: Leaving `peripherals = true/false` in `#[<distro>::app(...)]`.
  - *Fix*: Remove the `peripherals` argument and acquire the PAC in `#[init]`.
- **Symptom: `cannot find type TaskInits in this scope` or missing fields in `TaskInits`**
  - *Cause*: Task struct was not marked `init = generated` and was omitted from the `TaskInits` struct returned by `#[init]`.
  - *Fix*: Either initialize the task in `TaskInits { task_snake_case: TaskStruct { ... } }` or add `init = generated` to the task attribute if it has no fields.
- **Symptom: Async tasks fail to compile with unresolved traits**
  - *Cause*: Missing `async` feature on distribution or missing `rticx-async` dependency.
  - *Fix*: Add `features = ["async"]` to `rticx-<distro>` and add `rticx-async` to `Cargo.toml`.
- **Symptom: External task module cannot find `lock` method on shared resources**
  - *Cause*: `RticMutex` trait is not in scope in the external file.
  - *Fix*: Add `use crate::app::RticMutex;` to the external module's imports.
