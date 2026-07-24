#![cfg(feature = "multipac")]

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rtic_core::RticMacroBuilder;
use rtic_core::mock_backend::MockCoreBackend;
use rtic_core::parser::ast::AppArgs;

mod common;

#[test]
fn multipac_app_args_are_parsed() {
    let args: TokenStream = quote!(device = [mypac0, mypac1], cores = 2);
    let parsed = AppArgs::parse(args).expect("valid multipac args");
    assert_eq!(parsed.pacs.len(), 2);
    assert_eq!(parsed.pacs[0].to_token_stream().to_string(), "mypac0");
    assert_eq!(parsed.pacs[1].to_token_stream().to_string(), "mypac1");
}

#[test]
fn multipac_device_cores_mismatch_fails() {
    let args: TokenStream = quote!(device = [mypac0, mypac1], cores = 3);
    let err = AppArgs::parse(args).expect_err("mismatch should fail");
    assert!(err.to_string().contains("doesn't match"));
}

#[test]
fn multipac_full_pipeline_generates_pac_uses() {
    let args: TokenStream = quote!(device = [mypac0, mypac1], cores = 2);
    let input: syn::ItemMod = syn::parse_quote! {
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

        }
    };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let expanded = builder.build_rtic_macro2(args, input);
    let expanded = expanded.to_string();
    assert!(expanded.contains("use mypac0 as _"));
    assert!(expanded.contains("use mypac1 as _"));
}

#[test]
#[cfg(feature = "multibin")]
fn multipac_analysis_isolated_per_core() {
    let args: TokenStream = quote!(device = [mypac0, mypac1], cores = 2);
    let module: syn::ItemMod = syn::parse_quote! {
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
        }
    };

    let builder = RticMacroBuilder::new(MockCoreBackend);
    let expanded = builder.build_rtic_macro2(args, module);
    let expanded = expanded.to_string();
    assert!(expanded.contains("# [cfg (core = \"0\")] use mypac0 as _"));
    assert!(expanded.contains("# [cfg (core = \"1\")] use mypac1 as _"));
}
