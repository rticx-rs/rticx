//! Shared helpers for the `rticx-core` integration tests.

#![allow(dead_code)]

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse_quote;

/// A minimal set of macro arguments for a single-core application with one PAC path.
pub fn single_core_app_args() -> TokenStream {
    quote!(device = mypac)
}

/// Macro arguments for a multi-core (2 cores) application sharing a single PAC path.
pub fn multi_core_app_args() -> TokenStream {
    quote!(device = mypac, cores = 2)
}

/// A minimal `#[app(...)]` module for a single-core application with one hardware task
/// and one shared resource.
pub fn single_core_app_module() -> syn::ItemMod {
    parse_quote! {
        mod app {
            #[shared]
            struct Shared {
                pub counter: u32,
            }

            #[init]
            fn init() -> Shared {
                Shared { counter: 0 }
            }

            #[task(binds = UART, priority = 2, shared = [counter])]
            struct UartTask;

            impl RticTask for UartTask {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    UartTask
                }
                fn exec(&mut self) {
                    // task body
                }
            }

            #[idle]
            struct Idle;

            impl RticIdleTask for Idle {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    Idle
                }
                fn exec(&mut self) -> ! {
                    loop {}
                }
            }
        }
    }
}

/// A minimal `#[app(...)]` module for a 2-core application. Each core declares its own
/// shared resources, init, a hardware task bound to an interrupt, and an idle task.
pub fn multi_core_app_module() -> syn::ItemMod {
    parse_quote! {
        mod app {
            #[shared(core = 0)]
            struct Shared0 {
                pub counter: u32,
            }

            #[shared(core = 1)]
            struct Shared1 {
                pub counter: u32,
            }

            #[init(core = 0)]
            fn init0() -> Shared0 {
                Shared0 { counter: 0 }
            }

            #[init(core = 1)]
            fn init1() -> Shared1 {
                Shared1 { counter: 0 }
            }

            #[task(binds = UART0, priority = 2, shared = [counter], core = 0)]
            struct UartTask0;

            impl RticTask for UartTask0 {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    UartTask0
                }
                fn exec(&mut self) {}
            }

            #[task(binds = UART1, priority = 3, shared = [counter], core = 1)]
            struct UartTask1;

            impl RticTask for UartTask1 {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    UartTask1
                }
                fn exec(&mut self) {}
            }

            #[idle(core = 0)]
            struct Idle0;

            impl RticIdleTask for Idle0 {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    Idle0
                }
                fn exec(&mut self) -> ! {
                    loop {}
                }
            }

            #[idle(core = 1)]
            struct Idle1;

            impl RticIdleTask for Idle1 {
                type InitArgs = ();
                fn init(_: ()) -> Self {
                    Idle1
                }
                fn exec(&mut self) -> ! {
                    loop {}
                }
            }
        }
    }
}
