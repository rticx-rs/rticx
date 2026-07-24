//! Integration tests for the auto-assignment core of `rtic-auto-assign`.
//!
//! These exercise `auto_assign::run` directly: they parse an app module, run
//! the assignment pass, and inspect the `core` field of each parsed task (or
//! the returned error) to verify the logic.

use proc_macro2::TokenStream;
use quote::quote;
use rtic_auto_assign::auto_assign;
use rtic_auto_assign::parse::App;
use rtic_core::parse_utils::RticAttr;

mod common;

use common::{app_mod, assert_err_contains, multi_core_args, single_core_args};

fn parse(args: &TokenStream, app_mod: syn::ItemMod) -> App {
    let params = RticAttr::parse_from_tokens(args.clone()).expect("params parse");
    App::parse(&params, app_mod).expect("app parse")
}

/// Find a task by name in the parsed app.
fn task_core<'a>(app: &'a App, name: &str) -> &'a Option<u32> {
    app.tasks
        .iter()
        .find(|t| t.task_struct.ident == name)
        .map(|t| &t.core)
        .unwrap_or_else(|| panic!("no task named {name}"))
}

// ---------------------------------------------------------------------------
// Positive cases
// ---------------------------------------------------------------------------

#[test]
fn single_core_assigns_zero_without_shared() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task]
        struct T;
    });
    let mut parsed = parse(&args, app);
    auto_assign::run(&mut parsed).expect("assign");
    assert_eq!(*task_core(&parsed, "T"), Some(0));
}

#[test]
fn multi_core_preserves_explicit_core() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[task(core = 1)]
        struct T;
    });
    let mut parsed = parse(&args, app);
    auto_assign::run(&mut parsed).expect("assign");
    assert_eq!(*task_core(&parsed, "T"), Some(1));
}

#[test]
fn multi_core_infers_core_from_shared() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[shared(core = 1)]
        struct S {
            x: u32,
        }

        #[task(shared = [x])]
        struct T;
    });
    let mut parsed = parse(&args, app);
    auto_assign::run(&mut parsed).expect("assign");
    assert_eq!(*task_core(&parsed, "T"), Some(1));
}

#[test]
fn multi_core_infers_core_zero_from_shared() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[shared(core = 0)]
        struct S {
            x: u32,
        }

        #[task(shared = [x])]
        struct T;
    });
    let mut parsed = parse(&args, app);
    auto_assign::run(&mut parsed).expect("assign");
    assert_eq!(*task_core(&parsed, "T"), Some(0));
}

#[test]
fn single_core_with_shared_still_assigns_zero() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[shared]
        struct S {
            x: u32,
        }

        #[task(shared = [x])]
        struct T;
    });
    let mut parsed = parse(&args, app);
    auto_assign::run(&mut parsed).expect("assign");
    assert_eq!(*task_core(&parsed, "T"), Some(0));
}

// ---------------------------------------------------------------------------
// Negative cases
// ---------------------------------------------------------------------------

#[test]
fn multi_core_explicit_core_needed() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[task]
        struct T;
    });
    let mut parsed = parse(&args, app);
    assert_err_contains(
        auto_assign::run(&mut parsed),
        "A core needs to be explicitly assigned to T task",
    );
}

#[test]
fn multi_core_resource_not_found() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[shared(core = 0)]
        struct S {
            x: u32,
        }

        #[task(shared = [ghost])]
        struct T;
    });
    let mut parsed = parse(&args, app);
    assert_err_contains(
        auto_assign::run(&mut parsed),
        "The resource `ghost` was not found in any of the structs with #[shared] attribute.",
    );
}

#[test]
fn multi_core_core_mismatch() {
    let args = multi_core_args(2);
    let app = app_mod(quote! {
        #[shared(core = 0)]
        struct S0 {
            x: u32,
        }

        #[shared(core = 1)]
        struct S1 {
            y: u32,
        }

        #[task(shared = [x, y])]
        struct T;
    });
    let mut parsed = parse(&args, app);
    assert_err_contains(
        auto_assign::run(&mut parsed),
        "The task `T` is only allowed to use resources from core 0.",
    );
}

#[test]
fn duplicate_resource_name() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[shared]
        struct A {
            dup: u32,
        }

        #[shared]
        struct B {
            dup: u32,
        }
    });
    let mut parsed = parse(&args, app);
    assert_err_contains(
        auto_assign::run(&mut parsed),
        "The resource name `dup` was found on multiple structs with #[shared] attribute, but resource names must be unique.",
    );
}
