//! Integration tests for the parse phase of `rticx-deadline-pass`.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use rticx_core::parse_utils::RticAttr;
use rticx_deadline_pass::deadline_pass::parse::App;

mod common;

use common::{app_mod, single_core_args};

/// Builds a params `RticAttr` from a raw args tokenstream
fn params(args: &TokenStream) -> RticAttr {
    RticAttr::parse_from_tokens(args.clone()).expect("params parse")
}

#[test]
fn parse_task_with_deadline() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task(deadline = 10)]
        struct Task1;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].deadline, Some(10));
    assert_eq!(parsed.tasks[0].task_struct.ident.to_string(), "Task1");
}

#[test]
fn parse_sw_task_with_deadline() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[sw_task(deadline = 5)]
        struct SwTask1;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].deadline, Some(5));
    assert_eq!(parsed.tasks[0].task_struct.ident.to_string(), "SwTask1");
}

#[test]
fn parse_task_without_deadline() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task(binds = UART)]
        struct Task1;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].deadline, None);
}

#[test]
fn parse_task_with_deadline_and_other_attrs() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task(binds = UART, deadline = 100, shared = [x], core = 0)]
        struct Task1;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].deadline, Some(100));

    // Verify other attributes are preserved in params
    let params = &parsed.tasks[0].params;
    assert!(params.elements.contains_key("binds"));
    assert!(params.elements.contains_key("shared"));
    assert!(params.elements.contains_key("core"));
}

#[test]
fn parse_multiple_tasks_with_different_deadlines() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task(deadline = 10)]
        struct Task1;

        #[task(deadline = 5)]
        struct Task2;

        #[task(deadline = 20)]
        struct Task3;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 3);
    let deadlines: Vec<u32> = parsed.tasks.iter().map(|t| t.deadline.unwrap()).collect();
    assert_eq!(deadlines, vec![10, 5, 20]);
}

#[test]
fn parse_sw_task_with_all_attrs() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[sw_task(deadline = 15, shared = [x], core = 0, spawn_by = 1)]
        struct SwTask;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].deadline, Some(15));
    let params = &parsed.tasks[0].params;
    assert!(params.elements.contains_key("shared"));
    assert!(params.elements.contains_key("core"));
    assert!(params.elements.contains_key("spawn_by"));
}

#[test]
fn parse_preserves_plain_structs() {
    let args = single_core_args();
    let app = app_mod(quote! {
        struct PlainStruct;

        #[task(deadline = 10)]
        struct Task1;

        struct AnotherPlain {
            field: u32,
        }
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.rest_of_code.len(), 2);
}

#[test]
fn parse_preserves_mod_visibility_and_ident() {
    let args = single_core_args();
    let app: syn::ItemMod = syn::parse_quote! {
        pub mod myapp {
            #[task(deadline = 1)]
            struct T;
        }
    };
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.mod_ident.to_string(), "myapp");
    assert_eq!(parsed.mod_visibility.to_token_stream().to_string(), "pub");
}

#[test]
fn parse_task_deadline_zero() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task(deadline = 0)]
        struct Task1;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks[0].deadline, Some(0));
}

#[test]
fn parse_task_large_deadline() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[task(deadline = 4294967295)]  // u32::MAX
        struct Task1;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks[0].deadline, Some(u32::MAX));
}

#[test]
fn parse_sw_task_recognized_as_task() {
    let args = single_core_args();
    let app = app_mod(quote! {
        #[sw_task(deadline = 7)]
        struct SwTask;
    });
    let parsed = App::parse(&params(&args), app).expect("parse");

    assert_eq!(parsed.tasks.len(), 1);
    assert_eq!(parsed.tasks[0].task_struct.ident.to_string(), "SwTask");
}
