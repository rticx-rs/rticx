//! Integration tests for the codegen phase of `rticx-deadline-pass`.
//!
//! These run the full `DeadlineToPriorityPass::run_pass` pipeline (parse + analyze + codegen)
//! and verify that the generated `ItemMod` contains the expected sections. Because `RticAttr`
//! stores attribute elements in a `HashMap`, the emitted `#[task(...)]` element ordering is
//! non-deterministic, so assertions use individual element fragments (e.g. `priority = 1u32`)
//! rather than the whole attribute.

use proc_macro2::TokenStream;
use quote::quote;
use rticx_core::RticPass;
use rticx_deadline_pass::deadline_pass::DeadlineToPriorityPass;

mod common;

use common::{app_mod, assert_section_present, mod_to_string, multi_core_args, single_core_args};

/// Run the deadline pass end-to-end and return the generated module string.
fn run_pass(args: TokenStream, app_mod: syn::ItemMod) -> String {
    let (_, module) = DeadlineToPriorityPass::new(255)
        .run_pass(args, app_mod)
        .expect("pass succeeds");
    mod_to_string(&module)
}

// ===========================================================================
// Single-core: deadline converted to priority
// ===========================================================================

#[test]
fn codegen_single_core_injects_priority() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            struct Bar;
            #[task(deadline = 10)]
            struct T;
        }),
    );

    // passthrough struct
    assert_section_present(&generated, quote! { struct Bar ; }, "passthrough struct");
    // the task struct is re-emitted (its old attr removed, new attr prepended)
    assert_section_present(&generated, quote! { struct T ; }, "task struct");
    // the injected priority assignment (u32 suffix comes from `parse_quote!(#deadline)`)
    assert_section_present(&generated, quote! { task }, "task attribute name");
    assert_section_present(
        &generated,
        quote! { priority = 1u32 },
        "injected priority = 1u32",
    );
    // deadline attribute should be removed
    assert!(!generated.contains("deadline"));
    mod_app_shell(&generated);
}

#[test]
fn codegen_single_core_multiple_tasks_different_priorities() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            #[task(deadline = 100)]
            struct TaskLow;

            #[task(deadline = 10)]
            struct TaskHigh;

            #[task(deadline = 50)]
            struct TaskMed;
        }),
    );

    assert_section_present(&generated, quote! { struct TaskLow ; }, "low priority task");
    assert_section_present(
        &generated,
        quote! { struct TaskHigh ; },
        "high priority task",
    );
    assert_section_present(&generated, quote! { struct TaskMed ; }, "med priority task");

    // Shortest deadline = highest priority (1)
    assert_section_present(
        &generated,
        quote! { priority = 1u32 },
        "priority 1 for shortest deadline",
    );
    // Longest deadline = lowest priority (3)
    assert_section_present(
        &generated,
        quote! { priority = 3u32 },
        "priority 3 for longest deadline",
    );
    // Middle deadline = middle priority (2)
    assert_section_present(
        &generated,
        quote! { priority = 2u32 },
        "priority 2 for middle deadline",
    );

    assert!(!generated.contains("deadline"));
    mod_app_shell(&generated);
}

#[test]
fn codegen_single_core_preserves_other_attributes() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            #[task(binds = UART, deadline = 50, shared = [x], core = 0)]
            struct T;
        }),
    );

    // The task struct is re-emitted
    assert_section_present(&generated, quote! { struct T ; }, "task struct");
    // Original attributes preserved (except deadline)
    assert_section_present(&generated, quote! { binds = UART }, "binds preserved");
    assert_section_present(&generated, quote! { shared = [x] }, "shared preserved");
    assert_section_present(&generated, quote! { core = 0 }, "core preserved");
    // deadline removed, priority inserted
    assert_section_present(&generated, quote! { priority = 1u32 }, "priority injected");
    assert!(!generated.contains("deadline"));
    mod_app_shell(&generated);
}

#[test]
fn codegen_single_core_sw_task_converted() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            #[sw_task(deadline = 20, shared = [x], core = 0)]
            struct SwTask;
        }),
    );

    assert_section_present(&generated, quote! { struct SwTask ; }, "sw_task struct");
    assert_section_present(&generated, quote! { sw_task }, "sw_task attribute name");
    assert_section_present(
        &generated,
        quote! { priority = 1u32 },
        "priority injected (deadline 20 -> 1)",
    );
    assert_section_present(&generated, quote! { shared = [x] }, "shared preserved");
    assert_section_present(&generated, quote! { core = 0 }, "core preserved");
    assert!(!generated.contains("deadline"));
    mod_app_shell(&generated);
}

// ===========================================================================
// Multi-core: deadline converted to priority per core
// ===========================================================================

#[test]
fn codegen_multi_core_injects_priority_per_core() {
    let generated = run_pass(
        multi_core_args(2),
        app_mod(quote! {
            #[task(deadline = 10, core = 0)]
            struct TaskCore0;

            #[task(deadline = 5, core = 1)]
            struct TaskCore1;

            #[task(deadline = 20, core = 0)]
            struct AnotherCore0;
        }),
    );

    assert_section_present(&generated, quote! { struct TaskCore0 ; }, "core 0 task");
    assert_section_present(&generated, quote! { struct TaskCore1 ; }, "core 1 task");
    assert_section_present(
        &generated,
        quote! { struct AnotherCore0 ; },
        "another core 0 task",
    );

    // Each core's tasks get priorities based on their deadlines
    // Core 0: deadlines 10, 20 -> priorities 1, 2
    // Core 1: deadline 5 -> priority 1
    assert_section_present(&generated, quote! { priority = 1u32 }, "priority 1 exists");
    assert_section_present(&generated, quote! { priority = 2u32 }, "priority 2 exists");
    assert!(!generated.contains("deadline"));
    mod_app_shell(&generated);
}

// ===========================================================================
// Tasks without deadline: preserved unchanged
// ===========================================================================

#[test]
fn codegen_task_without_deadline_preserved() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            #[task(binds = UART, shared = [x])]
            struct NoDeadline;

            #[task(deadline = 10)]
            struct WithDeadline;
        }),
    );

    // Task without deadline keeps its original attributes (no priority added)
    assert_section_present(
        &generated,
        quote! { struct NoDeadline ; },
        "no-deadline task",
    );
    assert_section_present(&generated, quote! { binds = UART }, "binds preserved");
    assert_section_present(&generated, quote! { shared = [x] }, "shared preserved");
    assert!(!generated.contains("deadline"));

    // Task with deadline gets new priority
    assert_section_present(
        &generated,
        quote! { struct WithDeadline ; },
        "with-deadline task",
    );
    assert_section_present(
        &generated,
        quote! { priority = 1u32 },
        "new priority injected",
    );
    mod_app_shell(&generated);
}

#[test]
fn codegen_plain_structs_passed_through() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            struct Plain1;
            struct Plain2 { field: u32 }
            #[task(deadline = 10)]
            struct Task1;
        }),
    );

    assert_section_present(&generated, quote! { struct Plain1 ; }, "plain struct 1");
    // Note: the generated output for struct with fields doesn't have trailing comma in the expected format
    assert_section_present(
        &generated,
        quote! { struct Plain2 { field : u32 } },
        "plain struct 2 with field",
    );
    assert_section_present(&generated, quote! { struct Task1 ; }, "task struct");
    mod_app_shell(&generated);
}

#[test]
fn codegen_shared_resources_passed_through() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            #[shared]
            struct Shared {
                x: u32,
            }

            #[task(deadline = 10, shared = [x])]
            struct T;
        }),
    );

    assert_section_present(
        &generated,
        quote! { struct Shared { x : u32 , } },
        "shared struct",
    );
    assert_section_present(&generated, quote! { struct T ; }, "task struct");
    assert_section_present(&generated, quote! { priority = 1u32 }, "priority injected");
    mod_app_shell(&generated);
}

/// Asserts the `mod app { ... }` wrapper is present.
fn mod_app_shell(generated: &str) {
    assert_section_present(generated, quote! { mod app }, "app module shell");
}
