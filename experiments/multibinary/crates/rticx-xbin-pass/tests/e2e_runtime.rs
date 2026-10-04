//! M4-T2/M6-T1 acceptance: spawn from the producer applications, execute on
//! the receiver application's generated line dispatchers, verify the input
//! values, and observe the FIFO backpressure semantics.
//!
//! Both fixture projects are expanded with the **real** phase-2 pipeline — the
//! producers through [`XbinPass`] (their expansion is a plain module: no core
//! pass runs and the fixture provides `__rticx_interrupt_free`), the receiver
//! through the full [`RticMacroBuilder`] on `MockCoreBackend` with
//! [`SoftwarePass`] bound after the cross-binary pass (so the receiver structs
//! become core tasks, the line dispatchers become hardware tasks and the
//! generated `RticSwTask` trait and pend function come from the software pass)
//! — and written into one throwaway binary together with the in-tree
//! `rticx-xbin-mock` runtime.
//!
//! The binary is `#![no_main]`: the receiver expansion generates the project
//! entry (`main`) inside its `#[app]` module, so running it executes the real
//! boot sequence (`init`, `post_init` init hooks, idle). The idle task then
//! drives the cross-binary scenario:
//!
//! 1. it finishes the mock boot (each producer's `configure` + `init_fifos`
//!    hooks); boot sequencing between the simulated cores is
//!    distribution-owned and is not exercised by the mock;
//! 2. `Task::cross_spawn(input)` enqueues and rings the pair's doorbell; the
//!    input is not executed until the pair's router runs;
//! 3. calling the generated router ISR (`__xbin_router_{source}_1`) drains the
//!    pair's task ids into the line ready queues, pends the line dispatcher
//!    through the software pass's generated pend function, which runs the
//!    dispatcher ISR synchronously, draining FIFOs and calling `exec`;
//! 4. a full FIFO returns `Err(Some(input))`; draining frees it again;
//! 5. repeated spawn/drain cycles wrap the ring and keep the order.
//!
//! The **two-app fixture** (`mock_runtime_spawn_reaches_the_receiver_dispatcher`)
//! has one producer on global core 0 and one receiver on global core 1.
//!
//! The **three-app fixture** (`three_applications_spawn_through_their_own_routers`,
//! M6-T1) adds a second producer on global core 2: both producers declare
//! nothing and receive their generated stubs, the receiver runs two line
//! dispatchers from its `ipc_dispatchers` pool (one per `(source, priority)`
//! line) behind two per-pair routers, each producer updates only its own
//! FIFO, the producer on core 2 initializes its own `(2 -> 1)` FIFO before
//! its `post_init`, and the two sources backpressure independently.
//!
//! All applications share one process-global [`MockSystem`] through
//! `crate::backend_for`, so they see the same pools and doorbells, exactly
//! like three cores in one project.
//!
//! [`MockSystem`]: rticx_xbin_mock::MockSystem

use std::path::{Path, PathBuf};
use std::process::Command;

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use rticx_core::mock_backend::MockCoreBackend;
use rticx_core::{RticMacroBuilder, RticPass};
use rticx_sw_pass::{SoftwarePass, SwPassBackend};
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use rticx_xbin_proto::{
    AppEntry, CoreEntry, DoorbellEntry, FieldEntry, FifoEntry, Hash64, PoolEntry, SystemView,
    TargetRef, TaskEntry, TypeEntry, TypeKind,
};

/// A two-application system view: `app-m7` (global core 0) spawns
/// `EncryptTask` on `app-m4` (global core 1) at priority 3, capacity 2.
const SYSTEM_JSON: &str = r#"{
  "schema_version": 2,
  "rticx_generation": "0.2",
  "topology_hash": "0x2b7f11101722fb4e",
  "layout_hash": "0x59021e1c399c4ddb",
  "apps": [
    {
      "package": "app-m7",
      "target": { "kind": "bin", "name": "m7" },
      "source_hash": "0x26a74c0569235dd7",
      "core_ids": [0],
      "external_cores": [1]
    },
    {
      "package": "app-m4",
      "target": { "kind": "bin", "name": "m4" },
      "source_hash": "0x70b1fa36c7b61323",
      "core_ids": [1],
      "external_cores": [0]
    }
  ],
  "cores": [
    { "global_id": 0, "physical_core": 0, "app": "app-m7", "local_index": 0 },
    { "global_id": 1, "physical_core": 1, "app": "app-m4", "local_index": 0 }
  ],
  "types": [
    {
      "name": "EncryptReq",
      "kind": "message",
      "size": 12,
      "align": 4,
      "fields": [
        { "name": "addr", "ty": "u32", "offset": 0 },
        { "name": "len", "ty": "u32", "offset": 4 },
        { "name": "key", "ty": "u32", "offset": 8 }
      ]
    }
  ],
  "tasks": [
    {
      "id": 1,
      "name": "EncryptTask",
      "receiver_core": 1,
      "spawner_core": 0,
      "priority": 3,
      "capacity": 2,
      "input_type": "EncryptReq",
      "fifo": { "source": 0, "target": 1, "pool": "p01", "offset": 0, "elem_size": 12, "depth": 3 }
    }
  ],
  "pools": [
    {
      "id": "p01",
      "core_a": 0,
      "core_b": 1,
      "base_from_a": "0x30040000",
      "base_from_b": "0x30040000",
      "budget": 4096,
      "used": 100
    }
  ],
  "doorbells": [
    { "source": 0, "target": 1, "priority": 3, "line": 0 }
  ]
}
"#;

/// The generated `ipc_types.rs` stand-in: the fixture's `EncryptReq` type, as
/// `cargo xbin sync` emits it next to `system.json` (M1-T7). The pass re-emits
/// it into each `#[app]` module, so the retired checked-in crate is replaced by
/// the injected module.
const IPC_TYPES_RS: &str = "\
// @generated by `cargo xbin sync`\n\
#[repr(C)]\n\
#[derive(Clone, Copy, PartialEq, Eq, Debug)]\n\
pub struct EncryptReq {\n\
    pub addr: u32,\n\
    pub len: u32,\n\
    pub key: u32,\n\
}\n\
const _: () = {\n\
    assert!(core::mem::size_of::<EncryptReq>() == 12);\n\
    assert!(core::mem::align_of::<EncryptReq>() == 4);\n\
};\n\
unsafe impl CrossCoreMessage for EncryptReq {}\n";

/// Code-generation backend of the harness.
struct TestBackend;

impl XbinPassBackend for TestBackend {
    fn backend(&self) -> syn::Expr {
        syn::parse_quote!(__rticx_xbin_backend())
    }

    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt)
    }

    fn ring_doorbell_fn(&self, source: u32, target: u32, mut template: syn::ItemFn) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().doorbell_send(#source, #target, task_id)
        });
        template
    }

    fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident {
        format_ident!("__xbin_router_{source}_{target}")
    }

    fn read_doorbell_msg_fn(
        &self,
        target: u32,
        source: u32,
        mut template: syn::ItemFn,
    ) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().take_message(#source, #target)
        });
        template
    }

    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        Some(syn::parse_quote!(__XbinInterrupt))
    }
}

/// Software-task backend of the receiver harness.
///
/// Binding [`SoftwarePass`] gives the receivers the generated `RticSwTask`
/// trait and the generated `__rticx_local_irq_pend` the routers call (M6.5-T5).
/// The mock has no interrupt controller, so the generated pend body runs the
/// bound dispatcher ISR synchronously (tail-chaining). `irqs` names the
/// dispatcher interrupt entries of the fixture (one per line), so the pend
/// body references exactly the handlers the core pass generates.
struct TestSwBackend {
    irqs: &'static [&'static str],
}

impl TestSwBackend {
    fn new(irqs: &'static [&'static str]) -> Self {
        Self { irqs }
    }
}

impl SwPassBackend for TestSwBackend {
    fn queue_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt::Queue)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: syn::ItemFn) -> syn::ItemFn {
        // The dispatchers' `binds = IRQ…` entries make the core pass generate
        // the handlers under those interrupt names.
        let arms = self.irqs.iter().map(|irq| {
            let irq = format_ident!("{irq}");
            quote!(__XbinInterrupt::#irq => #irq(),)
        });
        empty_body_fn.block = Box::new(syn::parse_quote!({
            match irq_nbr {
                #(#arms)*
            }
        }));
        empty_body_fn
    }

    fn generate_cross_pend_fn(
        &self,
        _core: u32,
        _empty_body_fn: syn::ItemFn,
    ) -> Option<syn::ItemFn> {
        None
    }

    fn custom_interrupt_path(&self, _core: u32) -> Option<syn::Path> {
        Some(syn::parse_quote!(__XbinInterrupt))
    }
}

/// A producer fixture application: it declares nothing (the pass generates its
/// task stubs from the view) plus the pieces its codegen-only expansion needs
/// and `pub` wrappers over the generated private init hooks.
///
/// A producer is expanded through [`XbinPass`] alone, so no core pass provides
/// `__rticx_interrupt_free` (the pass generated `cross_spawn` calls it): the
/// fixture supplies the host no-op itself, like a distribution would provide
/// the target's critical section.
fn producer_module(core: u32) -> syn::ItemMod {
    let module = format_ident!("producer_{core}");
    syn::parse_quote! {
        pub mod #module {
            fn __rticx_xbin_backend() -> rticx_xbin_mock::MockBackend {
                crate::backend_for(#core)
            }

            fn __rticx_interrupt_free<R>(f: impl FnOnce() -> R) -> R {
                f()
            }

            pub fn test_configure() {
                __rticx_xbin_configure_shared_memory(&__rticx_xbin_backend());
            }

            pub fn test_init_fifos() {
                __rticx_xbin_init_fifos_core0(&__rticx_xbin_backend());
            }

            /// Builds this producer's own view of the fixture message. Each
            /// application has its own injected `ipc_types` module, so the
            /// single-crate harness constructs the producer's type for the
            /// spawn and the receiver's type for the comparison.
            pub fn spawn_request(addr: u32) -> ipc_types::EncryptReq {
                ipc_types::EncryptReq {
                    addr,
                    len: addr + 10,
                    key: addr + 20,
                }
            }
        }
    }
}

/// `#[app]` arguments of the producer on `core`; a single local core, so the
/// global id is `core` itself.
fn producer_args(core: u32, external: u32) -> TokenStream {
    quote!(
        device = pac,
        cores = 1,
        core_ids = [#core],
        external_cores = [#external]
    )
}

/// The two-app receiver fixture application: the task, a log of executed
/// inputs and the scenario driver in `#[idle]`.
///
/// The receiver runs through the full pipeline — the cross-binary pass, the
/// bound [`SoftwarePass`] (which generates the `RticSwTask` trait and the
/// `__rticx_local_irq_pend` function) and the core pass — so the generated
/// entry boots the application (init, init hooks, idle). The driver runs after
/// the boot sequence, from the idle task, and calls the generated **router
/// ISR** directly; the router pends the line dispatcher through the generated
/// pend function, which runs the dispatcher ISR synchronously (M6.5-T3).
fn two_app_receiver_module() -> syn::ItemMod {
    syn::parse_quote! {
        pub mod receiver_app {
            use std::sync::Mutex;

            use rticx_xbin_rt::backend::CrossBinBackend;

            fn __rticx_xbin_backend() -> rticx_xbin_mock::MockBackend {
                crate::backend_for(1)
            }

            /// Fixture interrupt enum of the line dispatcher, reached by the
            /// generated router through `custom_interrupt_path`. On a real
            /// distribution this is the PAC's `Interrupt` enum.
            #[allow(non_camel_case_types)]
            pub enum __XbinInterrupt {
                IRQ0,
            }

            fn request(addr: u32) -> ipc_types::EncryptReq {
                ipc_types::EncryptReq {
                    addr,
                    len: addr + 10,
                    key: addr + 20,
                }
            }

            /// Inputs executed by the receiver task, in dispatcher order.
            pub static RECEIVED: Mutex<Vec<ipc_types::EncryptReq>> = Mutex::new(Vec::new());

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            pub struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;

                fn exec(&mut self, input: Self::SpawnInput) {
                    RECEIVED
                        .lock()
                        .expect("the receiver log is never poisoned")
                        .push(input);
                }
            }

            pub fn test_configure() {
                __rticx_xbin_configure_shared_memory(&__rticx_xbin_backend());
            }

            #[idle]
            struct Idle;

            impl RticIdleTask for Idle {
                fn exec(&mut self) -> ! {
                    // Complete the mock boot: global core 0 produces the task
                    // FIFO, so its FIFO initializer runs here (the receiver's
                    // own `configure` ran in the generated entry). Boot
                    // sequencing between the simulated cores is
                    // distribution-owned and is not exercised by the mock.
                    crate::producer_0::test_configure();
                    crate::producer_0::test_init_fifos();

                    let backend = __rticx_xbin_backend();

                    // -- spawn enqueues and rings; the router wakes the line
                    // dispatcher, which executes the task
                    crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(1))
                        .expect("the first spawn enqueues");
                    assert!(
                        backend.router_wait(0, 1, std::time::Duration::from_millis(10)),
                        "the ring publishes the task id on the pair doorbell (M6.5-T2)"
                    );
                    assert!(
                        RECEIVED.lock().expect("the receiver log").is_empty(),
                        "nothing executes before the router runs"
                    );

                    __xbin_router_0_1();
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").as_slice(),
                        &[request(1)],
                        "spawn -> ring -> router -> pended dispatcher -> exec"
                    );

                    // -- a full FIFO (capacity 2) rejects the third input; the
                    // two successful rings coalesce on the pair doorbell and
                    // one router run drains both FIFO elements
                    crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(2))
                        .expect("the second spawn enqueues");
                    crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(3))
                        .expect("the third spawn enqueues");
                    assert_eq!(
                        crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(4)),
                        Err(Some(crate::producer_0::spawn_request(4))),
                        "a full FIFO returns the input to the spawner"
                    );

                    __xbin_router_0_1();
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").as_slice(),
                        &[request(1), request(2), request(3)],
                        "coalesced notifications lose no spawns"
                    );

                    // -- a duplicate notification is idempotent: the dispatcher
                    // finds an empty FIFO and is a no-op
                    backend
                        .doorbell_send(0, 1, 1)
                        .expect("the mock doorbell accepts the id");
                    __xbin_router_0_1();
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").len(),
                        3,
                        "a duplicate id loses no input and adds none"
                    );

                    // -- unknown ids are ignored
                    backend
                        .doorbell_send(0, 1, 99)
                        .expect("the mock doorbell accepts the id");
                    __xbin_router_0_1();
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").len(),
                        3,
                        "the router ignores unknown task ids"
                    );

                    // -- the ring wraps: depth (capacity + 1) slots are reused
                    for addr in 10..16 {
                        crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(addr))
                            .expect("a wrapping spawn enqueues");
                        __xbin_router_0_1();
                    }
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").len(),
                        9,
                        "every spawn past the ring depth was executed"
                    );
                    assert_eq!(
                        RECEIVED.lock().expect("the receiver log").as_slice(),
                        &[
                            request(1),
                            request(2),
                            request(3),
                            request(10),
                            request(11),
                            request(12),
                            request(13),
                            request(14),
                            request(15),
                        ],
                        "every spawn executed in order through the dispatcher"
                    );

                    println!("xbin: e2e ok");
                    std::process::exit(0);
                }
            }

            #[init]
            fn init() -> TaskInits {
                TaskInits { idle: Idle }
            }
        }
    }
}

fn two_app_receiver_args() -> TokenStream {
    quote!(
        device = pac,
        cores = 1,
        core_ids = [1],
        external_cores = [0],
        ipc_dispatchers = [IRQ0]
    )
}

/// The three-app receiver fixture application (M6-T1): two cross receivers fed
/// by two producer cores through two pairs, two line dispatchers and the
/// multi-source scenario driver in `#[idle]`.
///
/// `ipc_dispatchers` lists one interrupt per cross line in ascending
/// `(source, priority)` order, so `IRQ0` wakes the `(0, 3)` dispatcher and
/// `IRQ1` the `(2, 4)` one.
fn three_app_receiver_module() -> syn::ItemMod {
    syn::parse_quote! {
        pub mod receiver_app {
            use std::sync::Mutex;

            use rticx_xbin_rt::backend::CrossBinBackend;

            fn __rticx_xbin_backend() -> rticx_xbin_mock::MockBackend {
                crate::backend_for(1)
            }

            #[allow(non_camel_case_types)]
            pub enum __XbinInterrupt {
                IRQ0,
                IRQ1,
            }

            fn request(addr: u32) -> ipc_types::EncryptReq {
                ipc_types::EncryptReq {
                    addr,
                    len: addr + 10,
                    key: addr + 20,
                }
            }

            /// Inputs executed by `EncryptTask` (spawned by global core 0).
            pub static ENCRYPTED: Mutex<Vec<ipc_types::EncryptReq>> = Mutex::new(Vec::new());
            /// Inputs executed by `SensorTask` (spawned by global core 2).
            pub static SENSED: Mutex<Vec<ipc_types::EncryptReq>> = Mutex::new(Vec::new());

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            pub struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;

                fn exec(&mut self, input: Self::SpawnInput) {
                    ENCRYPTED
                        .lock()
                        .expect("the receiver log is never poisoned")
                        .push(input);
                }
            }

            #[sw_task(priority = 4, capacity = 1, spawn_by = 2)]
            pub struct SensorTask;

            impl RticSwTask for SensorTask {
                type SpawnInput = ipc_types::EncryptReq;

                fn exec(&mut self, input: Self::SpawnInput) {
                    SENSED
                        .lock()
                        .expect("the receiver log is never poisoned")
                        .push(input);
                }
            }

            #[idle]
            struct Idle;

            impl RticIdleTask for Idle {
                fn exec(&mut self) -> ! {
                    let backend = __rticx_xbin_backend();

                    // -- complete the mock boot. Each producer core
                    // initializes its own FIFO before its `post_init` would
                    // spawn (M6-T1); boot sequencing between the simulated
                    // cores is distribution-owned.
                    crate::producer_0::test_configure();
                    crate::producer_0::test_init_fifos();

                    // Dirty the `(2 -> 1)` FIFO through the receiver's view,
                    // then let its producer core initialize it: the topology
                    // owner (core 0) is not an endpoint of that pool.
                    let pool = backend
                        .ipc_region(2, 1)
                        .expect("the fixture declares the `{1, 2}` pool");
                    let sensor_fifo = unsafe {
                        rticx_xbin_rt::Fifo::<ipc_types::EncryptReq, 2usize>::view_at(
                            pool.base_from_target(),
                        )
                    };
                    assert!(unsafe { (*sensor_fifo).enqueue(request(99)) }.is_ok());
                    assert_eq!(unsafe { (*sensor_fifo).len() }, 1, "the FIFO is dirty");

                    crate::producer_2::test_configure();
                    crate::producer_2::test_init_fifos();
                    assert_eq!(
                        unsafe { (*sensor_fifo).len() },
                        0,
                        "the producer on core 2 initializes its own pool (M6-T1)"
                    );

                    // -- first source: spawn -> ring -> its router -> its line
                    // dispatcher -> exec
                    crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(1))
                        .expect("the first spawn enqueues");
                    assert!(
                        backend.router_wait(0, 1, std::time::Duration::from_millis(10)),
                        "the ring publishes the id on the `0->1` doorbell"
                    );
                    assert!(
                        ENCRYPTED.lock().expect("the receiver log").is_empty()
                            && SENSED.lock().expect("the receiver log").is_empty(),
                        "nothing executes before the routers run"
                    );

                    __xbin_router_0_1();
                    assert_eq!(
                        ENCRYPTED.lock().expect("the receiver log").as_slice(),
                        &[request(1)],
                        "the `0->1` router pends the `(0, 3)` dispatcher"
                    );
                    assert!(
                        SENSED.lock().expect("the receiver log").is_empty(),
                        "the other pair's dispatcher did not run"
                    );

                    // -- second source: its own router and dispatcher line
                    crate::producer_2::SensorTask::cross_spawn(crate::producer_2::spawn_request(2))
                        .expect("the second source spawns");
                    assert!(
                        backend.router_wait(2, 1, std::time::Duration::from_millis(10)),
                        "the ring publishes the id on the `2->1` doorbell"
                    );
                    __xbin_router_2_1();
                    assert_eq!(
                        SENSED.lock().expect("the receiver log").as_slice(),
                        &[request(2)],
                        "the `2->1` router pends the `(2, 4)` dispatcher"
                    );
                    assert_eq!(
                        ENCRYPTED.lock().expect("the receiver log").as_slice(),
                        &[request(1)],
                        "the first source's FIFO is untouched"
                    );

                    // -- per-source backpressure: `SensorTask` has capacity 1
                    crate::producer_2::SensorTask::cross_spawn(crate::producer_2::spawn_request(3))
                        .expect("the second `SensorTask` spawn enqueues");
                    assert_eq!(
                        crate::producer_2::SensorTask::cross_spawn(crate::producer_2::spawn_request(4)),
                        Err(Some(crate::producer_2::spawn_request(4))),
                        "a full `2->1` FIFO returns the input to its spawner"
                    );
                    assert!(
                        crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(5)).is_ok(),
                        "the other source's FIFO is unaffected"
                    );

                    __xbin_router_2_1();
                    assert_eq!(
                        SENSED.lock().expect("the receiver log").as_slice(),
                        &[request(2), request(3)],
                        "draining the `2->1` pair loses no spawns"
                    );
                    __xbin_router_0_1();
                    assert_eq!(
                        ENCRYPTED.lock().expect("the receiver log").as_slice(),
                        &[request(1), request(5)],
                        "the `0->1` pair drains independently"
                    );

                    // -- coalesced notifications of one pair lose no spawn:
                    // fill `EncryptTask` (capacity 2), ring twice, drain once
                    crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(6))
                        .expect("the wrap spawn enqueues");
                    crate::producer_0::EncryptTask::cross_spawn(crate::producer_0::spawn_request(7))
                        .expect("the wrap spawn enqueues");
                    __xbin_router_0_1();
                    assert_eq!(
                        ENCRYPTED.lock().expect("the receiver log").as_slice(),
                        &[request(1), request(5), request(6), request(7)],
                        "coalesced notifications lose no spawns"
                    );

                    // -- a duplicate notification is idempotent
                    backend
                        .doorbell_send(2, 1, 2)
                        .expect("the mock doorbell accepts the id");
                    __xbin_router_2_1();
                    assert_eq!(
                        SENSED.lock().expect("the receiver log").len(),
                        2,
                        "a duplicate id loses no input and adds none"
                    );

                    println!("xbin: e2e three-app ok");
                    std::process::exit(0);
                }
            }

            #[init]
            fn init() -> TaskInits {
                TaskInits { idle: Idle }
            }
        }
    }
}

fn three_app_receiver_args() -> TokenStream {
    quote!(
        device = pac,
        cores = 1,
        core_ids = [1],
        external_cores = [0, 2],
        ipc_dispatchers = [IRQ0, IRQ1]
    )
}

/// The fixture view with both applications' source hashes recorded, as
/// `cargo xbin sync` would (M3-T4).
fn two_app_system() -> String {
    let mut view = SystemView::from_json(SYSTEM_JSON).expect("fixture system view");
    for (package, args, app_mod) in [
        ("app-m7", &producer_args(0, 1), &producer_module(0)),
        (
            "app-m4",
            &two_app_receiver_args(),
            &two_app_receiver_module(),
        ),
    ] {
        let application = view
            .apps
            .iter_mut()
            .find(|application| application.package == package)
            .expect("fixture application");
        application.source_hash = rticx_xbin_pass::app_source_hash(args, app_mod);
    }
    view.seal();
    view.to_json()
}

/// Builds the three-application fixture view (M6-T1): `app-m7` (global core 0)
/// and `app-m5` (global core 2) each spawn onto `app-m4` (global core 1)
/// through their own `(source -> target)` pool, priority line and doorbell.
fn three_app_system() -> String {
    let mut view = SystemView::empty("0.2");
    view.apps = vec![
        AppEntry {
            package: "app-m7".to_string(),
            target: TargetRef::bin("m7"),
            source_hash: Hash64::ZERO,
            core_ids: vec![0],
            external_cores: vec![1],
        },
        AppEntry {
            package: "app-m5".to_string(),
            target: TargetRef::bin("m5"),
            source_hash: Hash64::ZERO,
            core_ids: vec![2],
            external_cores: vec![1],
        },
        AppEntry {
            package: "app-m4".to_string(),
            target: TargetRef::bin("m4"),
            source_hash: Hash64::ZERO,
            core_ids: vec![1],
            external_cores: vec![0, 2],
        },
    ];
    view.cores = vec![
        CoreEntry {
            global_id: 0,
            physical_core: 0,
            app: "app-m7".to_string(),
            local_index: 0,
        },
        CoreEntry {
            global_id: 2,
            physical_core: 2,
            app: "app-m5".to_string(),
            local_index: 0,
        },
        CoreEntry {
            global_id: 1,
            physical_core: 1,
            app: "app-m4".to_string(),
            local_index: 0,
        },
    ];
    view.types = vec![TypeEntry {
        name: "EncryptReq".to_string(),
        kind: TypeKind::Message,
        size: 12,
        align: 4,
        fields: vec![
            FieldEntry {
                name: "addr".to_string(),
                ty: "u32".to_string(),
                offset: 0,
            },
            FieldEntry {
                name: "len".to_string(),
                ty: "u32".to_string(),
                offset: 4,
            },
            FieldEntry {
                name: "key".to_string(),
                ty: "u32".to_string(),
                offset: 8,
            },
        ],
        variants: Vec::new(),
    }];
    view.tasks = vec![
        TaskEntry {
            id: 1,
            name: "EncryptTask".to_string(),
            receiver_core: 1,
            spawner_core: 0,
            priority: 3,
            capacity: 2,
            input_type: "EncryptReq".to_string(),
            fifo: FifoEntry {
                source: 0,
                target: 1,
                pool: Some("p01".to_string()),
                offset: 0,
                elem_size: 12,
                depth: 3,
            },
        },
        TaskEntry {
            id: 2,
            name: "SensorTask".to_string(),
            receiver_core: 1,
            spawner_core: 2,
            priority: 4,
            capacity: 1,
            input_type: "EncryptReq".to_string(),
            fifo: FifoEntry {
                source: 2,
                target: 1,
                pool: Some("p12".to_string()),
                offset: 0,
                elem_size: 12,
                depth: 2,
            },
        },
    ];
    view.pools = vec![
        PoolEntry {
            id: "p01".to_string(),
            core_a: 0,
            core_b: 1,
            base_from_a: 0x3004_0000,
            base_from_b: 0x3004_0000,
            budget: 4096,
            used: 100,
        },
        PoolEntry {
            id: "p12".to_string(),
            core_a: 1,
            core_b: 2,
            base_from_a: 0x3004_1000,
            base_from_b: 0x3004_1000,
            budget: 4096,
            used: 88,
        },
    ];
    view.doorbells = vec![
        DoorbellEntry {
            source: 0,
            target: 1,
            priority: 3,
            line: 0,
        },
        DoorbellEntry {
            source: 2,
            target: 1,
            priority: 4,
            line: 1,
        },
    ];

    for (package, args, app_mod) in [
        ("app-m7", producer_args(0, 1), producer_module(0)),
        ("app-m5", producer_args(2, 1), producer_module(2)),
        (
            "app-m4",
            three_app_receiver_args(),
            three_app_receiver_module(),
        ),
    ] {
        let application = view
            .apps
            .iter_mut()
            .find(|application| application.package == package)
            .expect("fixture application");
        application.source_hash = rticx_xbin_pass::app_source_hash(&args, &app_mod);
    }
    view.seal();
    view.to_json()
}

/// Expands a producer fixture through the cross-binary pass (no core pass).
fn expand_producer(system: &Path, package: &str, target: &str, core: u32, external: u32) -> String {
    let pass = XbinPass::with_system(system, package, target).with_backend(TestBackend);
    let (_, module) = pass
        .run_pass(producer_args(core, external), producer_module(core))
        .expect("producer code generation succeeds");
    module.to_token_stream().to_string()
}

/// Expands a receiver fixture through the cross-binary pass, the software
/// pass and the full core pass (the real phase-2 pipeline).
fn expand_receiver(
    system: &Path,
    package: &str,
    target: &str,
    args: TokenStream,
    app_mod: syn::ItemMod,
    irqs: &'static [&'static str],
) -> String {
    let pass = XbinPass::with_system(system, package, target).with_backend(TestBackend);
    let mut builder = RticMacroBuilder::new(MockCoreBackend);
    builder.bind_pre_core_pass(pass);
    builder.bind_pre_core_pass(SoftwarePass::new(TestSwBackend::new(irqs)));
    builder.build_rtic_macro2(args, app_mod, None).to_string()
}

fn cargo() -> PathBuf {
    std::env::var_os("CARGO")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cargo"))
}

/// Writes the throwaway `#![no_main]` binary executing the expansions of
/// `modules` (file name -> expansion) over the mock system holding `pools`.
fn write_project(root: &Path, modules: &[(&str, &str)], pools: &[(u32, u32)]) {
    let rt = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rticx-xbin-rt")
        .canonicalize()
        .expect("the runtime crate ships with the workspace");
    let mock = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../mock/rticx-xbin-mock")
        .canonicalize()
        .expect("the mock crate ships with the workspace");

    std::fs::create_dir_all(root.join("src")).expect("src dir");
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"xbin-e2e-runtime\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             publish = false\n\n\
             [dependencies]\n\
             pac = {{ path = \"pac\" }}\n\
             rticx-xbin-rt = {{ path = \"{}\" }}\n\
             rticx-xbin-mock = {{ path = \"{}\" }}\n\n\
             [workspace]\n",
            rt.display(),
            mock.display()
        ),
    )
    .expect("project manifest");

    let mut main = String::from(
        "// @generated by the M4-T2/M6-T1 end-to-end runtime test\n\
         #![no_main]\n\
         #![allow(dead_code, unused_imports, unused_variables, non_snake_case, \
         non_upper_case_globals, static_mut_refs)]\n\n\
         use std::sync::LazyLock;\n\n\
         use rticx_xbin_mock::{MockBackend, MockSystem};\n\n",
    );
    for (name, expansion) in modules {
        std::fs::write(
            root.join(format!("src/{name}.rs")),
            format!("{expansion}\n"),
        )
        .expect("module expansion");
        main.push_str(&format!("include!(\"{name}.rs\");\n"));
    }
    main.push_str(
        "\n/// The process-global mock system shared by every application, so a\n\
         /// spawn on one core reaches the router of another.\n\
         fn system() -> &'static MockSystem {\n\
             static SYSTEM: LazyLock<MockSystem> = LazyLock::new(|| {\n\
                 let mut system = MockSystem::new();\n",
    );
    for (core_a, core_b) in pools {
        main.push_str(&format!(
            "        system\n\
             \x20           .add_pool({core_a}, {core_b}, 4096)\n\
             \x20           .expect(\"the fixture declares the `{{{core_a}, {core_b}}}` pool\");\n"
        ));
    }
    main.push_str(
        "        system\n\
         \x20   });\n\
         \x20   &SYSTEM\n\
         }\n\n\
         fn backend_for(core: u32) -> MockBackend {\n\
         \x20   system().backend(core)\n\
         }\n",
    );
    std::fs::write(root.join("src/main.rs"), main).expect("harness source");

    std::fs::create_dir_all(root.join("pac/src")).expect("pac dir");
    std::fs::write(
        root.join("pac/Cargo.toml"),
        "[package]\nname = \"pac\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n",
    )
    .expect("pac manifest");
    std::fs::write(root.join("pac/src/lib.rs"), "").expect("pac source");
}

/// Runs the throwaway binary and asserts it reached `done`.
fn run_project(project: &Path, done: &str, scenario: &str) {
    let output = Command::new(cargo())
        .arg("run")
        .arg("--quiet")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(project.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", project.join("target"))
        .output()
        .expect("failed to run cargo");
    assert!(
        output.status.success(),
        "the cross-binary end-to-end harness failed:\n\
         stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(done),
        "the harness did not reach the end of the {scenario} scenario"
    );
}

#[test]
fn mock_runtime_spawn_reaches_the_receiver_dispatcher() {
    let dir = tempfile::tempdir().expect("tempdir");
    let system_path = dir.path().join("system.json");
    std::fs::write(&system_path, two_app_system()).expect("system.json");
    std::fs::write(dir.path().join("ipc_types.rs"), IPC_TYPES_RS).expect("ipc_types.rs");

    let sender = expand_producer(&system_path, "app-m7", "m7", 0, 1);
    assert!(
        !sender.contains("compile_error"),
        "sender code generation failed: {sender}"
    );
    assert!(
        sender.contains("cross_spawn"),
        "the sender API is missing: {sender}"
    );

    let receiver = expand_receiver(
        &system_path,
        "app-m4",
        "m4",
        two_app_receiver_args(),
        two_app_receiver_module(),
        &["IRQ0"],
    );
    assert!(
        !receiver.contains("compile_error"),
        "receiver code generation failed: {receiver}"
    );
    assert!(
        receiver.contains("__RticxXbinRouter0To1"),
        "the router is missing: {receiver}"
    );
    assert!(
        receiver.contains("__rticx_xbin_read_0_1"),
        "the read-doorbell function is missing: {receiver}"
    );
    assert!(
        receiver.contains("__RticxXbinDispatcher0To1P3"),
        "the line dispatcher is missing: {receiver}"
    );
    assert!(
        receiver.contains("__rticx_local_irq_pend"),
        "the router does not pend through the software pass's function: {receiver}"
    );

    let project = dir.path().join("project");
    write_project(
        &project,
        &[("producer_0", &sender), ("receiver", &receiver)],
        &[(0, 1)],
    );
    run_project(&project, "xbin: e2e ok", "two-app");
}

/// The M4-T2 runtime harness extended to the three-application fixture (M6-T1).
#[test]
fn three_applications_spawn_through_their_own_routers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let system_path = dir.path().join("system.json");
    std::fs::write(&system_path, three_app_system()).expect("system.json");
    std::fs::write(dir.path().join("ipc_types.rs"), IPC_TYPES_RS).expect("ipc_types.rs");

    // Both producers declare nothing: their stubs and ring functions come
    // from the pass (M5.5). The second producer's stub is generated even
    // though the application never declares it.
    let first = expand_producer(&system_path, "app-m7", "m7", 0, 1);
    assert!(
        !first.contains("compile_error"),
        "first producer generation failed: {first}"
    );
    assert!(
        first.contains("pub struct EncryptTask") && first.contains("__rticx_xbin_ring_0_1"),
        "the first producer stub/ring is missing: {first}"
    );

    let second = expand_producer(&system_path, "app-m5", "m5", 2, 1);
    assert!(
        !second.contains("compile_error"),
        "second producer generation failed: {second}"
    );
    assert!(
        second.contains("pub struct SensorTask") && second.contains("__rticx_xbin_ring_2_1"),
        "the second producer stub/ring is missing: {second}"
    );
    assert!(
        second.contains("__rticx_xbin_init_fifos_core0"),
        "the producer on core 2 does not initialize its pool: {second}"
    );

    let receiver = expand_receiver(
        &system_path,
        "app-m4",
        "m4",
        three_app_receiver_args(),
        three_app_receiver_module(),
        &["IRQ0", "IRQ1"],
    );
    assert!(
        !receiver.contains("compile_error"),
        "receiver code generation failed: {receiver}"
    );
    for (symbol, label) in [
        ("__RticxXbinDispatcher0To1P3", "first line dispatcher"),
        ("__RticxXbinDispatcher2To1P4", "second line dispatcher"),
        ("__RticxXbinRouter0To1", "first pair router"),
        ("__RticxXbinRouter2To1", "second pair router"),
    ] {
        assert!(
            receiver.contains(symbol),
            "the {label} is missing: {receiver}"
        );
    }

    let project = dir.path().join("project");
    write_project(
        &project,
        &[
            ("producer_0", &first),
            ("producer_2", &second),
            ("receiver", &receiver),
        ],
        &[(0, 1), (1, 2)],
    );
    run_project(&project, "xbin: e2e three-app ok", "three-app");
}
