//! Integration tests for the parse phase of `rticx-auto-assign`.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_auto_assign::parse::{APP_CORES, App};
use rticx_core::parse_utils::RticAttr;
use std::sync::atomic::Ordering;

mod common;

use common::{app_mod, assert_err_contains, multi_core_args, single_core_args};

/// Builds a params `RticAttr` from a raw args tokenstream (mirrors what
/// `AutoAssignPass::run_pass` does internally).
fn params(args: &TokenStream) -> RticAttr {
    RticAttr::parse_from_tokens(args.clone()).expect("params parse")
}

#[test]
fn cores_default_one() {
    let args = single_core_args();
    let app = app_mod(quote! {});
    let _ = App::parse(&params(&args), app).expect("parse");
    assert_eq!(APP_CORES.load(Ordering::SeqCst), 1);
}

#[test]
fn cores_explicit() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {});
    let _ = App::parse(&params(&args), app).expect("parse");
    assert_eq!(APP_CORES.load(Ordering::SeqCst), 2);
}

#[test]
fn shared_without_core_single_core_defaults_zero() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[shared]
        struct S {
            x: u32,
        }
    });
    let parsed = App::parse(&params(&args), app).expect("parse");
    assert_eq!(parsed.shared_resources.len(), 1);
    assert_eq!(parsed.shared_resources[0].core, 0);
    assert_eq!(parsed.shared_resources[0].shared_items.len(), 1);
}

#[test]
fn shared_without_core_multi_core_errors() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[shared]
        struct S {
            x: u32,
        }
    });
    assert_err_contains(
        App::parse(&params(&args), app),
        "has to be explicitly assinged in the struct",
    );
}

#[test]
fn task_explicit_core_preserved() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[task(core = 1)]
        struct T;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");
    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].core, Some(1));
}

#[test]
fn task_shared_items_parsed() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[task(shared = [a, b])]
        struct T;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");
    assert_eq!(parsed.tasks.len(), 1);
    let items: Vec<String> = parsed.tasks[0]
        .shared_items
        .iter()
        .map(|i| i.to_string())
        .collect();
    assert_eq!(items, vec!["a", "b"]);
}

#[test]
fn sw_task_recognized_as_task() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[sw_task]
        struct T;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");
    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].task_struct.ident, "T");
}

#[test]
fn idle_recognized_as_task() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[idle]
        struct I;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");
    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].task_struct.ident, "I");
}

#[test]
fn mod_visibility_ident_and_plain_structs_preserved() {
    let args = single_core_args();
    let app = syn::parse_quote! {
        pub mod app {
            struct Plain;
        }
    };
    let parsed = App::parse(&params(&args), app).expect("parse");
    assert_eq!(parsed.mod_ident, "app");
    assert_eq!(parsed.mod_visibility.to_token_stream().to_string(), "pub");
    assert_eq!(parsed.tasks.len(), 0);
    assert_eq!(parsed.rest_of_code.len(), 1);
}
