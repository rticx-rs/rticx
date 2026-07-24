//! Shared helpers for the `rtic-core` integration tests.

#![allow(dead_code)]

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse_quote;

/// A minimal set of macro arguments for a single-core application with one PAC path.
pub fn single_core_app_args() -> TokenStream {
    quote!(device = mypac)
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
