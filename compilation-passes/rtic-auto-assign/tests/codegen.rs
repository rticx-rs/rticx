//! Integration tests for the codegen phase of `rtic-auto-assign`.
//!
//! These run the full `AutoAssignPass::run_pass` pipeline (parse + auto-assign +
//! codegen) and verify that the generated `ItemMod` contains the expected
//! sections. Because `RticAttr` stores attribute elements in a `HashMap`, the
//! emitted `#[task(...)]` element ordering is non-deterministic, so assertions
//! use individual element fragments (e.g. `core = 0u32`) rather than the
//! whole attribute.

use proc_macro2::TokenStream;
use quote::quote;
use rtic_auto_assign::AutoAssignPass;
use rtic_core::RticPass;

mod common;

use common::{app_mod, assert_section_present, mod_to_string, multi_core_args, single_core_args};

/// Run the auto-assign pass end-to-end and return the generated module string.
fn run_pass(args: TokenStream, app_mod: syn::ItemMod) -> String {
    let (_, module) = AutoAssignPass
        .run_pass(args, app_mod)
        .expect("pass succeeds");
    mod_to_string(&module)
}

// ===========================================================================
// Single-core: coreless task is auto-assigned core 0
// ===========================================================================

#[test]
fn codegen_single_core_injects_core_zero() {
    let generated = run_pass(
        single_core_args(),
        app_mod(quote! {
            struct Bar;
            #[task]
            struct T;
        }),
    );

    // passthrough struct
    assert_section_present(&generated, quote! { struct Bar ; }, "passthrough struct");
    // the task struct is re-emitted (its old attr removed, new attr prepended)
    assert_section_present(&generated, quote! { struct T ; }, "task struct");
    // the injected core assignment (u32 suffix comes from `parse_quote!(#core)`)
    assert_section_present(&generated, quote! { task }, "task attribute name");
    assert_section_present(&generated, quote! { core = 0u32 }, "injected core = 0u32");
    mod_app_shell(&generated);
}

// ===========================================================================
// Multi-core: coreless task w/ shared resource on core 1 is assigned core 1
// ===========================================================================

#[test]
fn codegen_multi_core_injects_inferred_core() {
    let generated = run_pass(
        multi_core_args(2),
        app_mod(quote! {
            #[shared(core = 1)]
            struct S {
                x: u32,
            }
            #[task(shared = [x])]
            struct T;
            #[task(core = 0)]
            struct U;
        }),
    );

    // shared resource struct is re-emitted unchanged
    assert_section_present(
        &generated,
        quote! { struct S { x : u32 , } },
        "shared struct",
    );
    // the inferred task: `core = 1u32` injected, `shared = [x]` preserved
    // (order between the two is non-deterministic -> assert each fragment)
    assert_section_present(&generated, quote! { struct T ; }, "inferred task struct");
    assert_section_present(&generated, quote! { core = 1u32 }, "injected core = 1u32");
    assert_section_present(
        &generated,
        quote! { shared = [x] },
        "preserved shared = [x]",
    );
    // the explicit-core task is preserved untouched (no u32 suffix on user literal)
    assert_section_present(&generated, quote! { struct U ; }, "explicit task struct");
    assert_section_present(
        &generated,
        quote! { core = 0 },
        "explicit core = 0 (no suffix)",
    );
    mod_app_shell(&generated);
}

/// Asserts the `mod app { ... }` wrapper is present.
fn mod_app_shell(generated: &str) {
    assert_section_present(generated, quote! { mod app }, "app module shell");
}
