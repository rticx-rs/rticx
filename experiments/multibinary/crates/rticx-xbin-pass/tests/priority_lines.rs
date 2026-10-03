//! M6-T1 acceptance: build-phase priority-line validation.
//!
//! On one target core, each priority level belongs to exactly one origin
//! core. `sync` already rejects producer-vs-producer collisions through the
//! merge; the pass additionally sees the target application's raw
//! `#[sw_task]`/`#[async_task]` declarations at `build` and rejects a
//! cross-binary receiver that shares a line with them. The tests run the real
//! codegen pass over a three-application view (`app-m7` and `app-m5` produce
//! onto `app-m4`), snapshot the hard errors and prove the accepted
//! combinations still generate.

use std::path::{Path, PathBuf};

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use rticx_core::RticPass;
use rticx_xbin_pass::{XbinPass, XbinPassBackend};
use rticx_xbin_proto::{
    AppEntry, CoreEntry, DoorbellEntry, FieldEntry, FifoEntry, Hash64, PoolEntry, SystemView,
    TargetRef, TaskEntry, TypeEntry, TypeKind,
};

/// Code-generation backend of the fixtures.
struct TestBackend;

impl XbinPassBackend for TestBackend {
    fn backend(&self) -> syn::Expr {
        syn::parse_quote!(__rticx_xbin_backend())
    }

    fn rt_path(&self) -> syn::Path {
        syn::parse_quote!(rticx_xbin_rt)
    }

    fn ring_doorbell_fn(&self, source: u32, target: u32, mut template: syn::ItemFn) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().doorbell_send(#source, #target, task_id)
        });
        template
    }

    fn doorbell_interrupt(&self, target: u32, source: u32) -> syn::Ident {
        format_ident!("XbinRouter{source}To{target}")
    }

    fn read_doorbell_msg_fn(
        &self,
        target: u32,
        source: u32,
        mut template: syn::ItemFn,
    ) -> syn::ItemFn {
        template.block = syn::parse_quote!({
            __rticx_xbin_backend().take_message(#source, #target)
        });
        template
    }
}

/// One cross-binary task of the fixture view.
struct TaskSpec {
    name: &'static str,
    spawner: u32,
    receiver: u32,
    priority: u16,
    capacity: usize,
}

/// The pool id of a dual: `p{core_a}{core_b}` with `core_a < core_b`.
fn pool_id(first: u32, second: u32) -> String {
    let (a, b) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    format!("p{a}{b}")
}

/// The three-application view: `app-m7` (global core 0) and `app-m5` (global
/// core 2) produce onto `app-m4` (`core_ids`, global core 1 by default).
fn system_view(core_ids: &[u32], tasks: &[TaskSpec], external_cores: &[u32]) -> SystemView {
    let mut view = SystemView::empty("0.2");
    view.apps = vec![
        AppEntry {
            package: "app-m7".to_string(),
            target: TargetRef::bin("m7"),
            source_hash: Hash64::ZERO,
            core_ids: vec![0],
            external_cores: vec![1],
        },
        AppEntry {
            package: "app-m5".to_string(),
            target: TargetRef::bin("m5"),
            source_hash: Hash64::ZERO,
            core_ids: vec![2],
            external_cores: vec![1],
        },
        AppEntry {
            package: "app-m4".to_string(),
            target: TargetRef::bin("m4"),
            source_hash: Hash64::ZERO,
            core_ids: core_ids.to_vec(),
            external_cores: external_cores.to_vec(),
        },
    ];
    view.cores = vec![
        CoreEntry {
            global_id: 0,
            physical_core: 0,
            app: "app-m7".to_string(),
            local_index: 0,
        },
        CoreEntry {
            global_id: 2,
            physical_core: 2,
            app: "app-m5".to_string(),
            local_index: 0,
        },
    ];
    for (local, &global) in core_ids.iter().enumerate() {
        view.cores.push(CoreEntry {
            global_id: global,
            physical_core: global,
            app: "app-m4".to_string(),
            local_index: local as u32,
        });
    }

    view.types = vec![TypeEntry {
        name: "EncryptReq".to_string(),
        kind: TypeKind::Message,
        size: 12,
        align: 4,
        fields: vec![
            FieldEntry {
                name: "addr".to_string(),
                ty: "u32".to_string(),
                offset: 0,
            },
            FieldEntry {
                name: "len".to_string(),
                ty: "u32".to_string(),
                offset: 4,
            },
            FieldEntry {
                name: "key".to_string(),
                ty: "u32".to_string(),
                offset: 8,
            },
        ],
        variants: Vec::new(),
    }];

    let mut sorted: Vec<&TaskSpec> = tasks.iter().collect();
    sorted.sort_by_key(|task| task.name);
    view.tasks = sorted
        .iter()
        .enumerate()
        .map(|(index, task)| TaskEntry {
            id: index as u32 + 1,
            name: task.name.to_string(),
            receiver_core: task.receiver,
            spawner_core: task.spawner,
            priority: task.priority,
            capacity: task.capacity,
            input_type: "EncryptReq".to_string(),
            fifo: FifoEntry {
                source: task.spawner,
                target: task.receiver,
                pool: Some(pool_id(task.spawner, task.receiver)),
                offset: 0,
                elem_size: 12,
                depth: task.capacity as u32 + 1,
            },
        })
        .collect();

    // The distro pool of each used dual (the pass does not read the pools
    // themselves yet; they keep the fixture a valid schema-2 view).
    let mut pools: Vec<PoolEntry> = Vec::new();
    for task in tasks {
        let (core_a, core_b) = if task.spawner <= task.receiver {
            (task.spawner, task.receiver)
        } else {
            (task.receiver, task.spawner)
        };
        if pools
            .iter()
            .any(|pool| (pool.core_a, pool.core_b) == (core_a, core_b))
        {
            continue;
        }
        let base = 0x3004_0000 + 0x1000 * pools.len() as u32;
        pools.push(PoolEntry {
            id: pool_id(core_a, core_b),
            core_a,
            core_b,
            base_from_a: base,
            base_from_b: base,
            budget: 4096,
            used: 0,
        });
    }
    view.pools = pools;

    // One doorbell per distinct `(source, target, priority)` line, numbered
    // per target core in `(source, priority)` order, like `alloc::doorbells`.
    let mut lines: Vec<(u32, u32, u16)> = tasks
        .iter()
        .map(|task| (task.receiver, task.spawner, task.priority))
        .collect();
    lines.sort_unstable();
    lines.dedup();
    let mut next_line = std::collections::BTreeMap::new();
    view.doorbells = lines
        .into_iter()
        .map(|(target, source, priority)| {
            let line = next_line.entry(target).or_insert(0u32);
            let entry = DoorbellEntry {
                source,
                target,
                priority,
                line: *line,
            };
            *line += 1;
            entry
        })
        .collect();
    view.doorbells
        .sort_by_key(|doorbell| (doorbell.source, doorbell.target, doorbell.priority));

    view
}

/// Receiver `#[app]` arguments of `app-m4` for `core_ids` (one cross line per
/// `(source, priority)` pair of `tasks`, in order).
fn receiver_args(core_ids: &[u32], ipc: TokenStream) -> TokenStream {
    let cores = core_ids.len() as u32;
    let ids = core_ids.iter().map(|&id| quote!(#id)).collect::<Vec<_>>();
    quote!(
        device = mypac,
        cores = #cores,
        core_ids = [#(#ids),*],
        external_cores = [0, 2],
        ipc_dispatchers = #ipc
    )
}

/// Writes the fixture view with `app-m4`'s source hash recorded, as `cargo
/// xbin sync` would (M3-T4).
fn write_system(
    core_ids: &[u32],
    tasks: &[TaskSpec],
    external_cores: &[u32],
    args: &TokenStream,
    app_mod: &syn::ItemMod,
) -> (tempfile::TempDir, PathBuf) {
    let mut view = system_view(core_ids, tasks, external_cores);
    let application = view
        .apps
        .iter_mut()
        .find(|application| application.package == "app-m4")
        .expect("fixture app-m4");
    application.source_hash = rticx_xbin_pass::app_source_hash(args, app_mod);
    view.seal();

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("system.json");
    std::fs::write(&path, view.to_json()).expect("system.json");
    (dir, path)
}

/// Runs the codegen pass over `app-m4`.
fn run(
    path: &Path,
    args: TokenStream,
    app_mod: syn::ItemMod,
) -> syn::Result<(TokenStream, syn::ItemMod)> {
    XbinPass::with_system(path, "app-m4", "m4")
        .with_backend(TestBackend)
        .run_pass(args, app_mod)
}

/// The `EncryptTask`/`SensorTask` receivers of the fixture view.
fn cross_tasks() -> Vec<TaskSpec> {
    vec![
        TaskSpec {
            name: "EncryptTask",
            spawner: 0,
            receiver: 1,
            priority: 3,
            capacity: 2,
        },
        TaskSpec {
            name: "SensorTask",
            spawner: 2,
            receiver: 1,
            priority: 4,
            capacity: 2,
        },
    ]
}

/// The receiver module with both cross receivers, plus `extra` local items.
fn receiver_module(extra: TokenStream) -> syn::ItemMod {
    let extra: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, capacity = 2, spawn_by = 0)]
            struct EncryptTask;

            impl RticSwTask for EncryptTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(priority = 4, capacity = 2, spawn_by = 2)]
            struct SensorTask;

            impl RticSwTask for SensorTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #extra
        }
    };
    extra
}

#[test]
fn multi_source_receivers_generate_on_disjoint_lines() {
    let args = receiver_args(&[1], quote!([IRQ0, IRQ1]));
    let app_mod = receiver_module(quote!());
    let (_dir, path) = write_system(&[1], &cross_tasks(), &[0, 2], &args, &app_mod);

    let (_, module) = run(&path, args, app_mod).expect("two producer cores are accepted");
    let generated = module.to_token_stream().to_string();

    // One dispatcher per `(source, priority)` line and one router per pair.
    assert!(
        generated.contains("__RticxXbinDispatcher0To1P3"),
        "{generated}"
    );
    assert!(
        generated.contains("__RticxXbinDispatcher2To1P4"),
        "{generated}"
    );
    assert!(generated.contains("__RticxXbinRouter0To1"), "{generated}");
    assert!(generated.contains("__RticxXbinRouter2To1"), "{generated}");
}

#[test]
fn receiver_from_another_producer_on_one_line_is_rejected() {
    let tasks = vec![
        TaskSpec {
            name: "AlphaTask",
            spawner: 0,
            receiver: 1,
            priority: 3,
            capacity: 1,
        },
        TaskSpec {
            name: "BetaTask",
            spawner: 2,
            receiver: 1,
            priority: 3,
            capacity: 1,
        },
    ];
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, spawn_by = 0)]
            struct AlphaTask;

            impl RticSwTask for AlphaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(priority = 3, spawn_by = 2)]
            struct BetaTask;

            impl RticSwTask for BetaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };
    // Two distinct lines need two pool entries before the priority check can
    // reject them (the pool is validated first, M6.5-T1).
    let args = receiver_args(&[1], quote!([IRQ0, IRQ1]));
    let (_dir, path) = write_system(&[1], &tasks, &[0, 2], &args, &app_mod);

    let error = run(&path, args, app_mod)
        .expect_err("producer-vs-producer collisions are rejected at build too")
        .to_string();
    assert!(
        error.contains("the cross-binary receiver `BetaTask` (spawned by global core 2)"),
        "{error}"
    );
    assert!(
        error.contains("priority level belongs to exactly one origin core"),
        "{error}"
    );
}

#[test]
fn a_receiver_colliding_with_a_core_local_sw_task_is_rejected() {
    let args = receiver_args(&[1], quote!([IRQ0, IRQ1]));
    let app_mod = receiver_module(quote! {
        #[sw_task(priority = 3)]
        struct LocalTask;

        impl RticSwTask for LocalTask {
            type SpawnInput = u32;
            fn exec(&mut self, _input: u32) {}
        }
    });
    let (_dir, path) = write_system(&[1], &cross_tasks(), &[0, 2], &args, &app_mod);

    let error = run(&path, args, app_mod)
        .expect_err("a core-local task owns the line")
        .to_string();
    assert!(
        error.contains("cross-binary receiver `EncryptTask` is scheduled at priority 3"),
        "{error}"
    );
    assert!(
        error.contains("the software task `LocalTask` (spawned by global core 1)"),
        "{error}"
    );
}

#[test]
fn a_receiver_colliding_with_an_async_task_is_rejected() {
    let args = receiver_args(&[1], quote!([IRQ0, IRQ1]));
    let app_mod = receiver_module(quote! {
        #[async_task(priority = 4)]
        struct AsyncTask;
    });
    let (_dir, path) = write_system(&[1], &cross_tasks(), &[0, 2], &args, &app_mod);

    let error = run(&path, args, app_mod)
        .expect_err("an async task owns the line")
        .to_string();
    assert!(
        error.contains("cross-binary receiver `SensorTask` is scheduled at priority 4"),
        "{error}"
    );
    assert!(
        error.contains("the async task `AsyncTask` (spawned by global core 1)"),
        "{error}"
    );
}

#[test]
fn a_receiver_colliding_with_an_in_app_spawn_by_task_is_rejected() {
    let args = receiver_args(&[1], quote!([IRQ0, IRQ1]));
    // `spawn_by = 1` is this application's own global core, so the task is an
    // in-app cross-core task with origin core 1 (M5.5 global resolution).
    let app_mod = receiver_module(quote! {
        #[sw_task(priority = 3, spawn_by = 1)]
        struct InAppTask;

        impl RticSwTask for InAppTask {
            type SpawnInput = u32;
            fn exec(&mut self, _input: u32) {}
        }
    });
    let (_dir, path) = write_system(&[1], &cross_tasks(), &[0, 2], &args, &app_mod);

    let error = run(&path, args, app_mod)
        .expect_err("an in-app spawn_by task owns the line")
        .to_string();
    assert!(
        error.contains("cross-binary receiver `EncryptTask` is scheduled at priority 3"),
        "{error}"
    );
    assert!(
        error.contains("the software task `InAppTask` (spawned by global core 1)"),
        "{error}"
    );
}

#[test]
fn receivers_from_the_same_producer_share_a_line() {
    let tasks = vec![
        TaskSpec {
            name: "AlphaTask",
            spawner: 0,
            receiver: 1,
            priority: 3,
            capacity: 1,
        },
        TaskSpec {
            name: "BetaTask",
            spawner: 0,
            receiver: 1,
            priority: 3,
            capacity: 1,
        },
    ];
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(priority = 3, spawn_by = 0)]
            struct AlphaTask;

            impl RticSwTask for AlphaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(priority = 3, spawn_by = 0)]
            struct BetaTask;

            impl RticSwTask for BetaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };
    let args = receiver_args(&[1], quote!([IRQ0]));
    let (_dir, path) = write_system(&[1], &tasks, &[0, 2], &args, &app_mod);

    let (_, module) = run(&path, args, app_mod).expect("one producer shares one line");
    let generated = module.to_token_stream().to_string();
    assert!(
        generated.contains("__RticxXbinLine0To1P3") && generated.contains("__RticxXbinRouter0To1"),
        "{generated}"
    );
    assert!(
        !generated.contains("__RticxXbinRouter2To1"),
        "no other pair is generated: {generated}"
    );
}

#[test]
fn the_same_priority_on_different_target_cores_is_accepted() {
    // `app-m4` is dual-core here: the receivers run on different global cores
    // (1 and 7), so the same priority is independent on each.
    let tasks = vec![
        TaskSpec {
            name: "AlphaTask",
            spawner: 0,
            receiver: 1,
            priority: 3,
            capacity: 1,
        },
        TaskSpec {
            name: "BetaTask",
            spawner: 2,
            receiver: 7,
            priority: 3,
            capacity: 1,
        },
    ];
    let app_mod: syn::ItemMod = syn::parse_quote! {
        mod app {
            trait RticSwTask {
                type SpawnInput;
                fn exec(&mut self, input: Self::SpawnInput);
            }

            #[sw_task(core = 0, priority = 3, spawn_by = 0)]
            struct AlphaTask;

            impl RticSwTask for AlphaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }

            #[sw_task(core = 1, priority = 3, spawn_by = 2)]
            struct BetaTask;

            impl RticSwTask for BetaTask {
                type SpawnInput = ipc_types::EncryptReq;
                fn exec(&mut self, _input: Self::SpawnInput) {}
            }
        }
    };
    let args = receiver_args(&[1, 7], quote!([[IRQ0], [IRQ1]]));
    let (_dir, path) = write_system(&[1, 7], &tasks, &[0, 2], &args, &app_mod);

    run(&path, args, app_mod).expect("different target cores do not conflict");
}

#[test]
fn metadata_mode_does_not_validate_local_priority_lines() {
    // `sync` cannot see the application's local tasks, so the metadata pass
    // accepts the same collision the build-phase check rejects: the driver
    // catches producer-vs-producer conflicts only (plan §8).
    let app_mod = receiver_module(quote! {
        #[sw_task(priority = 3)]
        struct LocalTask;

        impl RticSwTask for LocalTask {
            type SpawnInput = u32;
            fn exec(&mut self, _input: u32) {}
        }
    });
    let dir = tempfile::tempdir().expect("tempdir");
    XbinPass::with_manifest(dir.path(), "app-m4", "m4")
        .with_backend(TestBackend)
        .run_pass(receiver_args(&[1], quote!([IRQ0, IRQ1])), app_mod)
        .expect("metadata mode records the manifest without the build-phase check");
}
