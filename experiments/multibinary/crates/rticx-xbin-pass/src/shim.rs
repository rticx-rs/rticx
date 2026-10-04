//! Phase-1 sender shims (M7-T2).
//!
//! Phase 1 (`cargo xbin sync`) runs `cargo check` with the cross-binary pass in
//! **metadata mode** to collect every application's receiver declarations. The
//! real sender stubs (`pub struct <Task>;` plus `Task::cross_spawn`) are derived
//! in phase 2 from the merged `system.json`; at phase-1 time that view does not
//! exist yet, so an application that calls a sender stub in its own source
//! would reference a type that has not been generated.
//!
//! Metadata mode therefore emits a **permissive shim** for every
//! `Task::cross_spawn` target the application source mentions: a
//! `pub struct <Task>;` with a generic `cross_spawn` that accepts any input.
//! The shim exists only so the phase-1 `cargo check` type-checks; phase 2 never
//! emits it and generates the real, view-derived stub instead, which enforces
//! the task's existence, producer core and input type. Missing or misspelled
//! tasks are therefore still hard errors at `cargo xbin build` / plain
//! `cargo build`.
//!
//! A shim is skipped when the `#[app]` module already defines the name, so the
//! software pass's own generated `cross_spawn` (an in-app cross-core task) is
//! never shadowed. References are found by scanning the module's token tree for
//! `Ident :: Ident(cross_spawn)`, which covers both `Task::cross_spawn(..)` and
//! fully qualified `module::Task::cross_spawn(..)` calls.

use std::collections::BTreeSet;

use proc_macro2::{TokenStream, TokenTree};
use quote::{ToTokens, format_ident};
use syn::{Item, ItemMod};

/// The associated function a cross-binary sender stub exposes.
const CROSS_SPAWN_FN: &str = "cross_spawn";

/// Returns the permissive phase-1 sender stubs of `app_mod` (M7-T2).
///
/// One stub is generated for every `Task::cross_spawn` target referenced by the
/// module that is not already defined there. The result is deterministic (task
/// names in ascending order).
pub(crate) fn generate_sender_shims(app_mod: &ItemMod) -> Vec<Item> {
    let defined = defined_names(app_mod);
    let mut items = Vec::new();
    for name in referenced_cross_spawn_targets(app_mod) {
        if defined.contains(&name) {
            continue;
        }
        let ident = format_ident!("{name}");
        let stub_doc = format!(
            "Permissive phase-1 shim of the cross-binary sender stub `{name}` (M7-T2): it lets \
             `cargo xbin sync` type-check source that calls `{name}::cross_spawn` before the \
             synced system view exists. Phase 2 never emits this shim; it generates the real, \
             view-derived stub, which enforces the task's existence, producer core and input type."
        );
        let spawn_doc = format!(
            "Shim of `{name}::cross_spawn` (M7-T2): the generic input accepts any spawn value; \
             phase 2 replaces the call with the typed stub from `system.json`."
        );
        items.push(syn::parse_quote! {
            #[doc = #stub_doc]
            #[doc(hidden)]
            #[allow(dead_code)]
            pub struct #ident;
        });
        items.push(syn::parse_quote! {
            impl #ident {
                #[doc = #spawn_doc]
                #[allow(dead_code)]
                pub fn cross_spawn<T>(input: T) -> Result<(), Option<T>> {
                    Err(Some(input))
                }
            }
        });
    }
    items
}

/// Collects the names of the items the `#[app]` module already defines.
///
/// A referenced task name that is already defined is a local software/async task
/// (or any user item); the software pass generates its `cross_spawn`, so no shim
/// may shadow it.
fn defined_names(app_mod: &ItemMod) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let Some((_, items)) = &app_mod.content else {
        return names;
    };
    for item in items {
        let name = match item {
            Item::Struct(item) => Some(item.ident.to_string()),
            Item::Enum(item) => Some(item.ident.to_string()),
            Item::Union(item) => Some(item.ident.to_string()),
            Item::Type(item) => Some(item.ident.to_string()),
            Item::Trait(item) => Some(item.ident.to_string()),
            Item::Fn(item) => Some(item.sig.ident.to_string()),
            Item::Const(item) => Some(item.ident.to_string()),
            Item::Static(item) => Some(item.ident.to_string()),
            _ => None,
        };
        if let Some(name) = name {
            names.insert(name);
        }
    }
    names
}

/// The `Task::cross_spawn` targets referenced anywhere in `app_mod`.
fn referenced_cross_spawn_targets(app_mod: &ItemMod) -> BTreeSet<String> {
    let mut targets = BTreeSet::new();
    collect_cross_spawn_targets(app_mod.to_token_stream(), &mut targets);
    targets
}

/// Recursively scans `stream` for the `Ident :: Ident(cross_spawn)` pattern.
///
/// The identifier before the last `::` is the task type name (the real
/// `cross_spawn` is an inherent associated function, so it is always reached
/// through the type). Groups are descended into, so calls nested in function
/// bodies and blocks are found.
fn collect_cross_spawn_targets(stream: TokenStream, targets: &mut BTreeSet<String>) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    for window in tokens.windows(4) {
        let [
            TokenTree::Ident(target),
            TokenTree::Punct(first),
            TokenTree::Punct(second),
            TokenTree::Ident(method),
        ] = window
        else {
            continue;
        };
        if first.as_char() == ':' && second.as_char() == ':' && method == CROSS_SPAWN_FN {
            targets.insert(target.to_string());
        }
    }
    for token in &tokens {
        if let TokenTree::Group(group) = token {
            collect_cross_spawn_targets(group.stream(), targets);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    #[test]
    fn finds_direct_and_qualified_targets() {
        let app_mod: ItemMod = syn::parse_quote! {
            mod app {
                fn start() {
                    let _ = PingTask::cross_spawn(input);
                    let _ = app::PongTask::cross_spawn(input);
                }
                fn flush() {
                    // A method call and a same-named function are not targets.
                    thing.cross_spawn(input);
                    cross_spawn(input);
                }
            }
        };
        let targets = referenced_cross_spawn_targets(&app_mod);
        assert_eq!(
            targets.into_iter().collect::<Vec<_>>(),
            ["PingTask", "PongTask"]
        );
    }

    #[test]
    fn skips_names_defined_in_the_module() {
        let app_mod: ItemMod = syn::parse_quote! {
            mod app {
                pub struct EncryptTask;

                fn start() {
                    let _ = EncryptTask::cross_spawn(input);
                    let _ = PingTask::cross_spawn(input);
                }
            }
        };
        let shims = generate_sender_shims(&app_mod);
        let rendered = quote!(#(#shims)*).to_string();
        assert!(rendered.contains("pub struct PingTask"), "{rendered}");
        assert!(!rendered.contains("pub struct EncryptTask"), "{rendered}");
    }

    #[test]
    fn emits_a_generic_cross_spawn_stub() {
        let app_mod: ItemMod = syn::parse_quote! {
            mod app {
                fn start() {
                    let _ = PingTask::cross_spawn(input);
                }
            }
        };
        let shims = generate_sender_shims(&app_mod);
        let rendered = quote!(#(#shims)*).to_string();
        assert!(rendered.contains("pub struct PingTask"), "{rendered}");
        assert!(rendered.contains("fn cross_spawn"), "{rendered}");
        assert!(
            rendered.contains("Result < () , Option < T > >"),
            "{rendered}"
        );
    }

    #[test]
    fn no_references_generate_nothing() {
        let app_mod: ItemMod = syn::parse_quote! {
            mod app {
                fn idle() {}
            }
        };
        assert!(generate_sender_shims(&app_mod).is_empty());
    }
}
