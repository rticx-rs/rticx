//! Integration tests for the analysis phase of `rtic-sw-pass`.
//!
//! These run `App::parse` followed by `Analysis::run` and inspect the resulting
//! `SubAnalysis` (positive scenarios) or assert the expected `syn::Error`
//! (negative scenarios).

use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::quote;
use rtic_sw_pass::software_pass::analyze::Analysis;
use rtic_sw_pass::software_pass::parse::App;

mod common;

use common::{app_mod, assert_err_contains};

/// Parse args + items and run the analysis.
fn analyze(args: TokenStream, items: TokenStream) -> syn::Result<Analysis> {
    let app_mod = app_mod(items);
    let app = App::parse(&args, app_mod)?;
    Analysis::run(&app)
}

// ---------------------------------------------------------------------------
// Positive scenarios
// ---------------------------------------------------------------------------

#[test]
fn analysis_single_core_one_task_one_dispatcher() {
    let args: TokenStream = quote!(device = mypac, dispatchers = [IRQ0]);
    let items = quote! {
        #[sw_task(priority = 2)]
        struct Foo;
        impl RticSwTask for Foo {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Foo }
            fn exec(&mut self, input: u32) {}
        }
    };
    let analysis = analyze(args, items).expect("analysis succeeds");
    assert_eq!(analysis.sub_analysis.len(), 1);
    let sub = &analysis.sub_analysis[0];
    assert_eq!(sub.core, 0);
    assert_eq!(sub.tasks_priority_map.len(), 1);
    let group = &sub.tasks_priority_map[&2];
    assert_eq!(group.len(), 1);
    assert_eq!(group[0].0.to_string(), "Foo");
    assert_eq!(group[0].1, 0); // core-local task
    assert_eq!(sub.dispatcher_priority_map.len(), 1);
    assert_eq!(
        sub.dispatcher_priority_map[&2]
            .to_token_stream()
            .to_string(),
        "IRQ0"
    );
}

#[test]
fn analysis_single_core_two_tasks_same_prio() {
    let args: TokenStream = quote!(device = mypac, dispatchers = [IRQ0]);
    let items = quote! {
        #[sw_task(priority = 2)]
        struct Foo;
        impl RticSwTask for Foo {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Foo }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 2)]
        struct Bar;
        impl RticSwTask for Bar {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Bar }
            fn exec(&mut self, input: u32) {}
        }
    };
    let analysis = analyze(args, items).expect("analysis succeeds");
    let sub = &analysis.sub_analysis[0];
    assert_eq!(sub.tasks_priority_map.len(), 1);
    let group = &sub.tasks_priority_map[&2];
    assert_eq!(group.len(), 2);
    let names: Vec<String> = group.iter().map(|(i, _)| i.to_string()).collect();
    assert!(names.contains(&"Foo".to_string()));
    assert!(names.contains(&"Bar".to_string()));
    assert_eq!(sub.dispatcher_priority_map.len(), 1);
}

#[test]
fn analysis_single_core_two_tasks_diff_prio() {
    let args: TokenStream = quote!(device = mypac, dispatchers = [IRQ0, IRQ1]);
    let items = quote! {
        #[sw_task(priority = 2)]
        struct Foo;
        impl RticSwTask for Foo {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Foo }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 3)]
        struct Bar;
        impl RticSwTask for Bar {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Bar }
            fn exec(&mut self, input: u32) {}
        }
    };
    let analysis = analyze(args, items).expect("analysis succeeds");
    let sub = &analysis.sub_analysis[0];
    assert_eq!(sub.tasks_priority_map.len(), 2);
    assert!(sub.tasks_priority_map.contains_key(&2));
    assert!(sub.tasks_priority_map.contains_key(&3));
    assert_eq!(sub.dispatcher_priority_map.len(), 2);
    // Both dispatchers must be assigned (order is non-deterministic across
    // HashMap keys, so just check both are present as values).
    let dispatcher_names: Vec<String> = sub
        .dispatcher_priority_map
        .values()
        .map(|p| p.to_token_stream().to_string())
        .collect();
    assert!(dispatcher_names.contains(&"IRQ0".to_string()));
    assert!(dispatcher_names.contains(&"IRQ1".to_string()));
}

#[test]
fn analysis_multi_core_local_tasks_each_core() {
    let args = common::multi_core_sw_args();
    let items = quote! {
        #[sw_task(priority = 2, core = 0)]
        struct Task0;
        impl RticSwTask for Task0 {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Task0 }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 3, core = 1)]
        struct Task1;
        impl RticSwTask for Task1 {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Task1 }
            fn exec(&mut self, input: u32) {}
        }
    };
    let analysis = analyze(args, items).expect("analysis succeeds");
    assert_eq!(analysis.sub_analysis.len(), 2);
    let core0 = &analysis.sub_analysis[0];
    assert_eq!(core0.core, 0);
    assert_eq!(core0.tasks_priority_map.len(), 1);
    assert!(core0.tasks_priority_map.contains_key(&2));
    let core1 = &analysis.sub_analysis[1];
    assert_eq!(core1.core, 1);
    assert_eq!(core1.tasks_priority_map.len(), 1);
    assert!(core1.tasks_priority_map.contains_key(&3));
}

#[test]
fn analysis_multi_core_cross_core_disjoint_prio() {
    // Core 1 hosts a local task (prio 2) and a cross-core task spawned by
    // core 0 (prio 3). The priorities are disjoint, so analysis must succeed.
    let args: TokenStream = quote!(
        device = mypac,
        cores = 2,
        dispatchers = [[IRQ0], [IRQ1, IRQ2]]
    );
    let items = quote! {
        #[sw_task(priority = 2, core = 1)]
        struct Local1;
        impl RticSwTask for Local1 {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Local1 }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 3, core = 1, spawn_by = 0)]
        struct Cross;
        impl RticSwTask for Cross {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Cross }
            fn exec(&mut self, input: u32) {}
        }
    };
    let analysis = analyze(args, items).expect("analysis succeeds");
    let core1 = &analysis.sub_analysis[1];
    assert_eq!(core1.core, 1);
    assert_eq!(core1.tasks_priority_map.len(), 2);
    assert!(core1.tasks_priority_map.contains_key(&2));
    assert!(core1.tasks_priority_map.contains_key(&3));
    // The prio-3 group must be tagged with spawn_by = 0.
    let prio3 = &core1.tasks_priority_map[&3];
    assert_eq!(prio3.len(), 1);
    assert_eq!(prio3[0].0.to_string(), "Cross");
    assert_eq!(prio3[0].1, 0); // spawn_by
    let prio2 = &core1.tasks_priority_map[&2];
    assert_eq!(prio2[0].1, 1); // core-local on core 1
}

#[test]
fn analysis_no_tasks_no_dispatchers() {
    let args = common::single_core_args();
    let analysis = analyze(args, quote! {}).expect("analysis succeeds");
    let sub = &analysis.sub_analysis[0];
    assert!(sub.tasks_priority_map.is_empty());
    assert!(sub.dispatcher_priority_map.is_empty());
}

// ---------------------------------------------------------------------------
// Negative scenarios
// ---------------------------------------------------------------------------

#[test]
fn analysis_overlapping_local_vs_mc_priority() {
    // Core 0 has a local task (prio 2) and a cross-core task that runs on
    // core 0 but is spawned by core 1, also at prio 2 -> overlap.
    let args = common::multi_core_sw_args();
    let items = quote! {
        #[sw_task(priority = 2, core = 0)]
        struct Local;
        impl RticSwTask for Local {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Local }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 2, core = 0, spawn_by = 1)]
        struct Cross;
        impl RticSwTask for Cross {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Cross }
            fn exec(&mut self, input: u32) {}
        }
    };
    let result = analyze(args, items);
    assert_err_contains(
        result,
        "overlapping priority with other core-local software tasks",
    );
}

#[test]
fn analysis_mc_same_prio_diff_spawn_by() {
    // Two cross-core tasks on core 0, both prio 3, but spawned by different
    // cores (1 and 2) -> forbidden.
    let args = common::three_core_sw_args();
    let items = quote! {
        #[sw_task(priority = 3, core = 0, spawn_by = 1)]
        struct CrossA;
        impl RticSwTask for CrossA {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { CrossA }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 3, core = 0, spawn_by = 2)]
        struct CrossB;
        impl RticSwTask for CrossB {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { CrossB }
            fn exec(&mut self, input: u32) {}
        }
    };
    let result = analyze(args, items);
    assert_err_contains(
        result,
        "have the same priority but they are spawned by different cores",
    );
}

#[test]
fn analysis_dispatchers_too_few() {
    // Two distinct priorities, but only one dispatcher provided.
    let args: TokenStream = quote!(device = mypac, dispatchers = [IRQ0]);
    let items = quote! {
        #[sw_task(priority = 2)]
        struct Foo;
        impl RticSwTask for Foo {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Foo }
            fn exec(&mut self, input: u32) {}
        }
        #[sw_task(priority = 3)]
        struct Bar;
        impl RticSwTask for Bar {
            type InitArgs = ();
            type SpawnInput = u32;
            fn init(_: ()) -> Self { Bar }
            fn exec(&mut self, input: u32) {}
        }
    };
    let result = analyze(args, items);
    assert_err_contains(result, "Expected 2 dispatchers, but found 1.");
}
