#![allow(dead_code)]

use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::quote;
use rticx_async_pass::AsyncPassBackend;
use syn::{ItemFn, parse_quote};

pub fn single_core_args() -> TokenStream {
    quote!(device = mypac)
}

pub fn multi_core_args() -> TokenStream {
    quote!(device = mypac, cores = 2)
}

pub fn app_mod(items: TokenStream) -> syn::ItemMod {
    syn::parse_quote! {
        mod app {
            #items
        }
    }
}

pub fn single_core_sw_args() -> TokenStream {
    quote!(device = mypac, dispatchers = [IRQ0])
}

pub fn multi_core_sw_args() -> TokenStream {
    quote!(device = mypac, cores = 2, dispatchers = [[IRQ0], [IRQ1]])
}

pub fn three_core_sw_args() -> TokenStream {
    quote!(
        device = mypac,
        cores = 3,
        dispatchers = [[IRQ0], [IRQ1], [IRQ2]]
    )
}

pub fn single_core_sw_app_module() -> syn::ItemMod {
    app_mod(quote! {
        struct Bar;

        #[async_task(priority = 2)]
        struct Foo;

        impl RticAsyncTask for Foo {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self {
                Foo
            }
            fn exec(&mut self, input: u32) {}
        }
    })
}

pub fn multi_core_sw_app_module() -> syn::ItemMod {
    app_mod(quote! {
        #[async_task(priority = 2, core = 0)]
        struct Task0;

        impl RticAsyncTask for Task0 {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self {
                Task0
            }
            fn exec(&mut self, input: u32) {}
        }

        #[async_task(priority = 3, core = 1, spawn_by = 0)]
        struct Cross;

        impl RticAsyncTask for Cross {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self {
                Cross
            }
            fn exec(&mut self, input: u32) {}
        }
    })
}

pub fn assert_err_contains<T>(result: syn::Result<T>, substr: &str) {
    let err = match result {
        Ok(_) => panic!("expected an error, but parsing/analysis succeeded"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains(substr),
        "expected error to contain {substr:?}, got: {err}"
    );
}

pub fn assert_section_present(generated: &str, expected: TokenStream, label: &str) {
    let expected = expected.to_string();
    assert!(
        generated.contains(&expected),
        "missing expected section `{label}` in the generated output\n\
         expected:\n{expected}\n\n\
         generated:\n{generated}"
    );
}

pub struct MockAsyncBackend {
    pub cross: bool,
}

impl AsyncPassBackend for MockAsyncBackend {
    fn queue_path(&self) -> syn::Path {
        parse_quote!(rticx::export::Queue)
    }

    fn generate_local_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> ItemFn {
        let body = parse_quote!({
            mock_local_pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        empty_body_fn
    }

    fn generate_cross_pend_fn(&self, _core: u32, mut empty_body_fn: ItemFn) -> Option<ItemFn> {
        if !self.cross {
            return None;
        }
        let body = parse_quote!({
            mock_cross_pend(irq_nbr);
        });
        empty_body_fn.block = Box::new(body);
        Some(empty_body_fn)
    }
}

pub fn mod_to_string(item_mod: &syn::ItemMod) -> String {
    item_mod.to_token_stream().to_string()
}
