//! Integration tests for the parsing phase of `rtic-sw-pass`.
//!
//! These exercise ` AppParameters::parse `, ` TaskParams::from_attr `, and the
//! top-level ` App::parse ` (partitioning, dispatcher routing, rest-of-code
//! handling, error cases).

use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::quote;
use rtic_core::parse_utils::RticAttr;
use rtic_sw_pass::software_pass::parse::App;
use rtic_sw_pass::software_pass::parse::ast::{AppParameters, TaskParams};

mod common;

use common::assert_err_contains;

/// Convenience: parse args + an items tokenstream into an `App`.
fn parse_app(args: TokenStream, items: TokenStream) -> syn::Result<App> {
    let app_mod = common::app_mod(items);
    App::parse(&args, app_mod)
}

// ---------------------------------------------------------------------------
// Block A : AppParameters::parse (parse/ast.rs)
// ---------------------------------------------------------------------------

fn parse_app_params(args: TokenStream) -> syn::Result<AppParameters> {
    AppParameters::parse(&args)
}

#[test]
fn app_params_default_cores_one() {
    let params = parse_app_params(common::single_core_args()).expect("valid args");
    assert_eq!(params.cores, 1);
    assert_eq!(params.pacs.len(), 1);
    assert_eq!(params.pacs[0].to_token_stream().to_string(), "mypac");
    assert!(params.dispatchers.is_empty());
}

#[test]
fn app_params_cores_explicit() {
    let params = parse_app_params(common::multi_core_args()).expect("valid args");
    assert_eq!(params.cores, 2);
    assert_eq!(params.pacs.len(), 2);
    assert!(
        params
            .pacs
            .iter()
            .all(|p| p.to_token_stream().to_string() == "mypac")
    );
}

#[test]
fn app_params_missing_device_errors() {
    let args: TokenStream = quote!(cores = 2);
    assert_err_contains(parse_app_params(args), "device");
}

#[test]
fn app_params_device_array_mismatch_errors() {
    let args: TokenStream = quote!(device = [pac0, pac1], cores = 1);
    assert_err_contains(parse_app_params(args), "doesn't match");
}

#[test]
fn app_params_dispatchers_flat_single_core() {
    let args: TokenStream = quote!(device = mypac, dispatchers = [IRQ0, IRQ1]);
    let params = parse_app_params(args).expect("valid args");
    assert_eq!(params.cores, 1);
    assert!(params.dispatchers.contains_key(&0));
    let names: Vec<String> = params.dispatchers[&0]
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    assert_eq!(names, vec!["IRQ0", "IRQ1"]);
}

#[test]
fn app_params_dispatchers_nested_multi_core() {
    let args: TokenStream = quote!(device = mypac, cores = 2, dispatchers = [[IRQ0], [IRQ1]]);
    let params = parse_app_params(args).expect("valid args");
    assert_eq!(params.cores, 2);
    assert!(params.dispatchers.contains_key(&0));
    assert!(params.dispatchers.contains_key(&1));
    let n0: Vec<String> = params.dispatchers[&0]
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    let n1: Vec<String> = params.dispatchers[&1]
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    assert_eq!(n0, vec!["IRQ0"]);
    assert_eq!(n1, vec!["IRQ1"]);
}

#[test]
fn app_params_dispatchers_count_mismatch_errors() {
    let args: TokenStream = quote!(device = mypac, cores = 2, dispatchers = [[IRQ0]]);
    assert_err_contains(parse_app_params(args), "does not match");
}

// ---------------------------------------------------------------------------
// Block B : TaskParams::from_attr (parse/ast.rs)
// ---------------------------------------------------------------------------

/// Builds a `RticAttr` from a `#[sw_task(...)]` attribute string.
fn sw_task_attr(args: TokenStream) -> RticAttr {
    let s: syn::ItemStruct = syn::parse_quote! {
        #[sw_task(#args)]
        struct DummySwt;
    };
    RticAttr::parse_from_attr(&s.attrs[0]).expect("valid sw_task attribute")
}

#[test]
fn task_params_defaults() {
    let attr = sw_task_attr(quote!());
    let params = TaskParams::from_attr(&attr);
    assert_eq!(params.priority, 0);
    assert_eq!(params.core, 0);
    assert_eq!(params.spawn_by, 0);
}

#[test]
fn task_params_explicit_values() {
    let attr = sw_task_attr(quote!(priority = 3, core = 1, spawn_by = 0));
    let params = TaskParams::from_attr(&attr);
    assert_eq!(params.priority, 3);
    assert_eq!(params.core, 1);
    assert_eq!(params.spawn_by, 0);
}

#[test]
fn task_params_spawn_by_defaults_to_core() {
    let attr = sw_task_attr(quote!(core = 2));
    let params = TaskParams::from_attr(&attr);
    assert_eq!(params.core, 2);
    assert_eq!(params.spawn_by, 2);
}

// ---------------------------------------------------------------------------
// Block C : App::parse (parse/mod.rs)
// ---------------------------------------------------------------------------

#[test]
fn parse_empty_single_core_app() {
    let app = parse_app(common::single_core_args(), quote!()).expect("valid empty app");
    assert_eq!(app.mod_ident.to_string(), "app");
    assert!(app.rest_of_code.is_empty());
    assert_eq!(app.app_params.cores, 1);
    assert_eq!(app.sub_apps.len(), 1);
    let sub = &app.sub_apps[0];
    assert_eq!(sub.core, 0);
    assert!(sub.sw_tasks.is_empty());
    assert!(sub.mc_sw_tasks.is_empty());
    assert!(sub.dispatchers.is_empty());
}

#[test]
fn parse_preserves_mod_visibility() {
    let args = common::single_core_args();
    let app_mod: syn::ItemMod = syn::parse_quote! {
        pub mod app {}
    };
    let app = App::parse(&args, app_mod).expect("valid app");
    assert!(matches!(app.mod_visibility, syn::Visibility::Public(_)));
}

#[test]
fn parse_local_sw_task_with_impl() {
    let items = quote! {
        #[sw_task]
        struct Foo;

        impl RticSwTask for Foo {
            fn exec(&mut self) {}
        }
    };
    let app = parse_app(common::single_core_args(), items).expect("valid app");
    let sub = &app.sub_apps[0];
    assert_eq!(sub.sw_tasks.len(), 1);
    assert_eq!(sub.sw_tasks[0].name().to_string(), "Foo");
    assert!(sub.sw_tasks[0].task_impl.is_some());
    assert!(sub.mc_sw_tasks.is_empty());
    // Struct and impl are not leaked into rest_of_code.
    assert!(app.rest_of_code.is_empty());
}

#[test]
fn parse_sw_task_without_impl() {
    let items = quote! {
        #[sw_task]
        struct Foo;
    };
    let app = parse_app(common::single_core_args(), items).expect("valid app");
    let sub = &app.sub_apps[0];
    assert_eq!(sub.sw_tasks.len(), 1);
    assert!(sub.sw_tasks[0].task_impl.is_none());
}

#[test]
fn parse_custom_sw_trait_name_recognized() {
    // The matcher accepts any trait whose name ends with `RticSwTask`.
    let items = quote! {
        #[sw_task]
        struct Foo;

        impl MyRticSwTask for Foo {
            fn exec(&mut self) {}
        }
    };
    let app = parse_app(common::single_core_args(), items).expect("valid app");
    let sub = &app.sub_apps[0];
    assert_eq!(sub.sw_tasks.len(), 1);
    assert!(sub.sw_tasks[0].task_impl.is_some());
}

#[test]
fn parse_non_sw_structs_and_impls_go_to_rest() {
    let items = quote! {
        struct Bar;

        impl Bar {
            fn hello(&self) {}
        }
    };
    let app = parse_app(common::single_core_args(), items).expect("valid app");
    let sub = &app.sub_apps[0];
    assert!(sub.sw_tasks.is_empty());
    // A plain struct and a non-trait impl -> both in rest_of_code.
    assert_eq!(app.rest_of_code.len(), 2);
    let kinds: Vec<&str> = app
        .rest_of_code
        .iter()
        .map(|item| match item {
            syn::Item::Struct(_) => "struct",
            syn::Item::Impl(_) => "impl",
            _ => "other",
        })
        .collect();
    assert!(kinds.contains(&"struct"));
    assert!(kinds.contains(&"impl"));
}

#[test]
fn parse_partition_local_vs_cross_core() {
    // core 0 local task, and a cross-core task that runs on core 1 but is
    // spawned by core 0 -> should land in core 1's `mc_sw_tasks`.
    let items = quote! {
        #[sw_task(core = 0)]
        struct Local;

        impl RticSwTask for Local {
            fn exec(&mut self) {}
        }

        #[sw_task(core = 1, spawn_by = 0)]
        struct Cross;

        impl RticSwTask for Cross {
            fn exec(&mut self) {}
        }
    };
    let app = parse_app(common::multi_core_args(), items).expect("valid app");
    assert_eq!(app.sub_apps.len(), 2);

    let core0 = &app.sub_apps[0];
    assert_eq!(core0.core, 0);
    assert_eq!(core0.sw_tasks.len(), 1);
    assert_eq!(core0.sw_tasks[0].name().to_string(), "Local");
    assert!(core0.mc_sw_tasks.is_empty());

    let core1 = &app.sub_apps[1];
    assert_eq!(core1.core, 1);
    assert!(core1.sw_tasks.is_empty());
    assert_eq!(core1.mc_sw_tasks.len(), 1);
    assert_eq!(core1.mc_sw_tasks[0].name().to_string(), "Cross");
    // The cross-core task belongs neither to core 0 sw_tasks nor core 0 mc.
    assert!(core0.sw_tasks.iter().all(|t| t.name() != "Cross"));
}

#[test]
fn parse_dispatchers_flat_single_core_routed() {
    let args: TokenStream = quote!(device = mypac, dispatchers = [IRQ0, IRQ1]);
    let app = parse_app(args, quote!()).expect("valid app");
    let sub = &app.sub_apps[0];
    let names: Vec<String> = sub
        .dispatchers
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    assert_eq!(names, vec!["IRQ0", "IRQ1"]);
}

#[test]
fn parse_dispatchers_nested_multi_core_routed() {
    let args: TokenStream = quote!(device = mypac, cores = 2, dispatchers = [[IRQ0], [IRQ1]]);
    let app = parse_app(args, quote!()).expect("valid app");
    assert_eq!(app.sub_apps.len(), 2);
    let n0: Vec<String> = app.sub_apps[0]
        .dispatchers
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    let n1: Vec<String> = app.sub_apps[1]
        .dispatchers
        .iter()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    assert_eq!(n0, vec!["IRQ0"]);
    assert_eq!(n1, vec!["IRQ1"]);
}

#[test]
fn parse_dispatchers_count_mismatch_errors() {
    let args: TokenStream = quote!(device = mypac, cores = 2, dispatchers = [[IRQ0]]);
    assert_err_contains(parse_app(args, quote!()), "does not match");
}

#[test]
fn parse_sw_task_priority_param() {
    let items = quote! {
        #[sw_task(priority = 5)]
        struct Foo;

        impl RticSwTask for Foo {
            fn exec(&mut self) {}
        }
    };
    let app = parse_app(common::single_core_args(), items).expect("valid app");
    let sub = &app.sub_apps[0];
    assert_eq!(sub.sw_tasks.len(), 1);
    assert_eq!(sub.sw_tasks[0].params.priority, 5);
}

#[test]
fn parse_multiple_sw_tasks_in_order() {
    let items = quote! {
        #[sw_task]
        struct Foo;
        impl RticSwTask for Foo {
            fn exec(&mut self) {}
        }

        #[sw_task]
        struct Bar;
        impl RticSwTask for Bar {
            fn exec(&mut self) {}
        }
    };
    let app = parse_app(common::single_core_args(), items).expect("valid app");
    let sub = &app.sub_apps[0];
    assert_eq!(sub.sw_tasks.len(), 2);
    // Insertion order is preserved.
    assert_eq!(sub.sw_tasks[0].name().to_string(), "Foo");
    assert_eq!(sub.sw_tasks[1].name().to_string(), "Bar");
}
