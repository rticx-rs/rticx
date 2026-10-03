//! `cargo xbin sync --html`: a self-contained visualization of the system
//! view.
//!
//! The driver normalizes the emitted [`SystemView`] into a small presentation
//! model (cores with the cross-binary tasks they execute and spawn, pools with
//! their FIFO buffers, and the interaction pairs) and embeds it as JSON in a
//! static HTML document. The inline script lays the page out and draws the
//! arrows; the driver itself never depends on a browser.
//!
//! Pools are rendered one panel per pool (M6.9-T8): both directions of a dual
//! share a single distro pool, so a panel carries every FIFO of the dual at its
//! pool-relative offset, colored by its producer core. The bar is scaled to the
//! occupied extent rather than the whole budget, so the FIFOs stay readable
//! instead of collapsing into a sliver. A FIFO that names no pool (a project
//! whose manifests carry no capability table) falls back to a synthetic
//! per-direction panel so it stays visible.
//!
//! Keeping the normalization in Rust is deliberate: `system.json` only carries
//! cross-binary tasks today, but when it later also carries same-binary
//! cross-core tasks, only this module grows a category -- the embedded JSON
//! schema and the script stay generic.
//!
//! The emitted document is deterministic for a given view (the model is
//! serialized with `serde_json`, whose field order is declaration order), so it
//! can be asserted on in tests.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::Serialize;

use rticx_xbin_proto::{SystemView, fifo_size};

/// Renders the complete HTML document for `view`.
pub(crate) fn render_html(view: &SystemView) -> String {
    let model = ViewModel::from_view(view);
    let json = serde_json::to_string(&model).expect("the view model always serializes");
    // `<` is the only character that could prematurely close the embedding
    // `<script>` element; every value is otherwise plain ASCII. Escaping the
    // slash too keeps this safe even if a future field ever carries markup.
    let json = json.replace('<', "\\u003c");

    TEMPLATE
        .replacen("/*__XBIN_STYLE__*/", STYLE, 1)
        .replacen("/*__XBIN_SCRIPT__*/", SCRIPT, 1)
        .replacen("__XBIN_VIEW_JSON__", &json, 1)
}

const TEMPLATE: &str = include_str!("visualize/template.html");
const STYLE: &str = include_str!("visualize/style.css");
const SCRIPT: &str = include_str!("visualize/app.js");

/// The JSON payload embedded in the HTML document.
///
/// This is the seam for future task categories: add fields here, push them into
/// the core cards, and the script renders them without changes.
#[derive(Debug, Serialize)]
struct ViewModel {
    generation: String,
    topology_hash: String,
    layout_hash: String,
    cores: Vec<CoreCard>,
    pools: Vec<PoolPanel>,
    pairs: Vec<PairFilter>,
    /// Every cross-binary task, flat and ordered; the script groups them by
    /// `pair` to build the filter tree.
    tasks: Vec<TaskFilter>,
    stats: Stats,
}

#[derive(Debug, Serialize)]
struct Stats {
    cores: usize,
    tasks: usize,
    pools: usize,
    doorbells: usize,
}

/// One core box: its identity and the tasks it executes and spawns.
#[derive(Debug, Serialize)]
struct CoreCard {
    id: u32,
    app: String,
    local_index: u32,
    /// Cross-binary receivers executing on this core.
    executes: Vec<TaskCard>,
    /// Generated `cross_spawn` stubs this core originates.
    spawns: Vec<TaskCard>,
}

/// A task as it appears on one core.
#[derive(Debug, Serialize)]
struct TaskCard {
    id: u32,
    name: String,
    input_type: String,
    priority: u16,
    capacity: usize,
    /// The other endpoint of the task (spawner for a receiver, receiver for a
    /// stub).
    peer: u32,
    /// Unordered interaction key (`"min-max"`).
    pair: String,
    /// Ordered direction (`"source-target"`).
    direction: String,
    doorbell: Option<u32>,
}

/// One distro IPC pool (a *dual*) and the FIFOs of both directions inside it.
#[derive(Debug, Serialize)]
struct PoolPanel {
    /// Distro pool id; `None` for a synthetic panel covering FIFOs whose
    /// project bound no distro capability table (no `pool` in the view).
    id: Option<String>,
    core_a: u32,
    core_b: u32,
    pair: String,
    base_from_a: u32,
    base_from_b: u32,
    /// Bytes the distro reserves for both directions of the dual.
    budget: u32,
    /// Bytes occupied by both directions' FIFOs.
    used: u32,
    /// Scale the bar is drawn against: the occupied extent (at least `used`
    /// and the end of the last buffer), so the FIFOs fill the bar instead of
    /// collapsing into a sliver of the whole pool budget.
    extent: u32,
    buffers: Vec<BufferCard>,
}

/// One task FIFO inside a pool.
#[derive(Debug, Serialize)]
struct BufferCard {
    task_id: u32,
    task_name: String,
    input_type: String,
    /// Producer (spawner) global core id.
    source: u32,
    /// Consumer (receiver) global core id.
    target: u32,
    offset: u32,
    total_bytes: u64,
    elem_size: u32,
    depth: u32,
    capacity: usize,
    priority: u16,
    doorbell: Option<u32>,
    pair: String,
    direction: String,
}

/// One checkbox: an unordered core pair that can be isolated.
#[derive(Debug, Serialize)]
struct PairFilter {
    a: u32,
    b: u32,
    key: String,
}

/// One cross-binary task, as listed under its pair in the filter tree.
#[derive(Debug, Serialize)]
struct TaskFilter {
    id: u32,
    name: String,
    input_type: String,
    /// Ordered direction (`"source-target"`).
    direction: String,
    /// Unordered interaction key of its pair (`"min-max"`).
    pair: String,
    source: u32,
    target: u32,
    priority: u16,
    doorbell: Option<u32>,
}

impl ViewModel {
    fn from_view(view: &SystemView) -> Self {
        // Core identity, and every core id referenced anywhere (so a reference
        // to a core missing from `cores` still gets a box).
        let mut meta: BTreeMap<u32, (String, u32)> = BTreeMap::new();
        for core in &view.cores {
            meta.insert(core.global_id, (core.app.clone(), core.local_index));
        }

        let mut ids: BTreeSet<u32> = view.cores.iter().map(|core| core.global_id).collect();
        for task in &view.tasks {
            ids.insert(task.receiver_core);
            ids.insert(task.spawner_core);
        }
        for pool in &view.pools {
            ids.insert(pool.core_a);
            ids.insert(pool.core_b);
        }

        // Doorbell line per `(source, target, priority)` task line.
        let mut doorbells: BTreeMap<(u32, u32, u16), u32> = BTreeMap::new();
        for doorbell in &view.doorbells {
            doorbells.insert(
                (doorbell.source, doorbell.target, doorbell.priority),
                doorbell.line,
            );
        }

        let doorbell_of = |task: &rticx_xbin_proto::TaskEntry| {
            doorbells
                .get(&(task.spawner_core, task.receiver_core, task.priority))
                .copied()
        };

        let card = |task: &rticx_xbin_proto::TaskEntry, peer: u32, executes: bool| TaskCard {
            id: task.id,
            name: task.name.clone(),
            input_type: task.input_type.clone(),
            priority: task.priority,
            capacity: task.capacity,
            peer,
            pair: pair_key(task.spawner_core, task.receiver_core),
            direction: direction_key(task.spawner_core, task.receiver_core),
            doorbell: executes.then(|| doorbell_of(task)).flatten(),
        };

        let mut cores = Vec::with_capacity(ids.len());
        for id in &ids {
            let (app, local_index) = meta
                .get(id)
                .cloned()
                .unwrap_or_else(|| (String::from("?"), 0));

            let mut executes: Vec<TaskCard> = view
                .tasks
                .iter()
                .filter(|task| task.receiver_core == *id)
                .map(|task| card(task, task.spawner_core, true))
                .collect();
            executes.sort_by(|left, right| {
                (left.priority, &left.name).cmp(&(right.priority, &right.name))
            });

            let mut spawns: Vec<TaskCard> = view
                .tasks
                .iter()
                .filter(|task| task.spawner_core == *id)
                .map(|task| card(task, task.receiver_core, false))
                .collect();
            spawns.sort_by(|left, right| left.name.cmp(&right.name));

            cores.push(CoreCard {
                id: *id,
                app,
                local_index,
                executes,
                spawns,
            });
        }

        // One panel per distro pool (M6.9-T8): both directions of a dual share
        // a single pool, so the panel carries every FIFO of the dual at its
        // pool-relative offset. A FIFO naming no (or an unknown) pool falls
        // back to a synthetic per-direction panel so it stays visible.
        let mut pool_index: BTreeMap<&str, usize> = BTreeMap::new();
        for (index, pool) in view.pools.iter().enumerate() {
            pool_index.insert(pool.id.as_str(), index);
        }

        let mut per_pool: BTreeMap<usize, Vec<&rticx_xbin_proto::TaskEntry>> = BTreeMap::new();
        let mut unpooled: BTreeMap<(u32, u32), Vec<&rticx_xbin_proto::TaskEntry>> = BTreeMap::new();
        for task in &view.tasks {
            match task
                .fifo
                .pool
                .as_deref()
                .and_then(|id| pool_index.get(id).copied())
            {
                Some(index) => per_pool.entry(index).or_default().push(task),
                None => unpooled
                    .entry((task.fifo.source, task.fifo.target))
                    .or_default()
                    .push(task),
            }
        }

        let buffer_of = |task: &rticx_xbin_proto::TaskEntry| -> BufferCard {
            let total_bytes = fifo_size(task.fifo.elem_size, task.capacity).unwrap_or(u64::MAX);
            BufferCard {
                task_id: task.id,
                task_name: task.name.clone(),
                input_type: task.input_type.clone(),
                source: task.fifo.source,
                target: task.fifo.target,
                offset: task.fifo.offset,
                total_bytes,
                elem_size: task.fifo.elem_size,
                depth: task.fifo.depth,
                capacity: task.capacity,
                priority: task.priority,
                doorbell: doorbell_of(task),
                pair: pair_key(task.fifo.source, task.fifo.target),
                direction: direction_key(task.fifo.source, task.fifo.target),
            }
        };

        /// Sorts `buffers` by offset and returns the byte extent they occupy.
        fn finalize(buffers: &mut [BufferCard]) -> u64 {
            buffers.sort_by_key(|buffer| (buffer.offset, buffer.task_id));
            buffers
                .iter()
                .map(|buffer| u64::from(buffer.offset) + buffer.total_bytes)
                .max()
                .unwrap_or(0)
        }

        let mut panels: Vec<PoolPanel> = Vec::with_capacity(view.pools.len() + unpooled.len());
        let mut pairs: BTreeSet<(u32, u32)> = BTreeSet::new();

        for (index, pool) in view.pools.iter().enumerate() {
            let mut buffers: Vec<BufferCard> = per_pool
                .remove(&index)
                .unwrap_or_default()
                .into_iter()
                .map(buffer_of)
                .collect();
            let end = finalize(&mut buffers);
            let used = u64::from(pool.used).max(end);
            panels.push(PoolPanel {
                id: Some(pool.id.clone()),
                core_a: pool.core_a,
                core_b: pool.core_b,
                pair: pair_key(pool.core_a, pool.core_b),
                base_from_a: pool.base_from_a,
                base_from_b: pool.base_from_b,
                budget: pool.budget,
                used: u32::try_from(used).unwrap_or(u32::MAX),
                extent: u32::try_from(used.max(1)).unwrap_or(u32::MAX),
                buffers,
            });
            pairs.insert(ordered_pair(pool.core_a, pool.core_b));
        }

        for ((source, target), tasks) in unpooled {
            let mut buffers: Vec<BufferCard> = tasks.into_iter().map(buffer_of).collect();
            let end = finalize(&mut buffers);
            let extent = u32::try_from(end.max(1)).unwrap_or(u32::MAX);
            panels.push(PoolPanel {
                id: None,
                core_a: source,
                core_b: target,
                pair: pair_key(source, target),
                base_from_a: 0,
                base_from_b: 0,
                budget: extent,
                used: extent,
                extent,
                buffers,
            });
            pairs.insert(ordered_pair(source, target));
        }

        // A pair may also be implied by a task without a matching pool in the
        // view; keep the checkbox so the interaction can still be focused.
        for task in &view.tasks {
            pairs.insert(ordered_pair(task.spawner_core, task.receiver_core));
        }

        let filters = pairs
            .into_iter()
            .map(|(a, b)| PairFilter {
                a,
                b,
                key: pair_key(a, b),
            })
            .collect();

        let mut task_filters: Vec<TaskFilter> = view
            .tasks
            .iter()
            .map(|task| TaskFilter {
                id: task.id,
                name: task.name.clone(),
                input_type: task.input_type.clone(),
                direction: direction_key(task.spawner_core, task.receiver_core),
                pair: pair_key(task.spawner_core, task.receiver_core),
                source: task.spawner_core,
                target: task.receiver_core,
                priority: task.priority,
                doorbell: doorbell_of(task),
            })
            .collect();
        task_filters.sort_by(|left, right| {
            (left.source, left.target, &left.name).cmp(&(right.source, right.target, &right.name))
        });

        let pool_count = panels.len();
        ViewModel {
            generation: view.rticx_generation.clone(),
            topology_hash: hash_text(view.topology_hash),
            layout_hash: hash_text(view.layout_hash),
            cores,
            pools: panels,
            pairs: filters,
            tasks: task_filters,
            stats: Stats {
                cores: ids.len(),
                tasks: view.tasks.len(),
                pools: pool_count,
                doorbells: view.doorbells.len(),
            },
        }
    }
}

/// Orders a core pair so `a < b`.
fn ordered_pair(first: u32, second: u32) -> (u32, u32) {
    if first <= second {
        (first, second)
    } else {
        (second, first)
    }
}

/// Unordered interaction key, e.g. `"0-1"`.
fn pair_key(first: u32, second: u32) -> String {
    let (a, b) = ordered_pair(first, second);
    format!("{a}-{b}")
}

/// Ordered direction key, e.g. `"0-1"` for `0 -> 1`.
fn direction_key(source: u32, target: u32) -> String {
    format!("{source}-{target}")
}

/// Renders a [`rticx_xbin_proto::Hash64`] as its `0x…` text.
fn hash_text(hash: rticx_xbin_proto::Hash64) -> String {
    let mut out = String::new();
    let _ = write!(out, "{hash}");
    out
}

#[cfg(test)]
mod tests {
    use rticx_xbin_proto::{
        AppEntry, CoreEntry, DoorbellEntry, FifoEntry, Hash64, PoolEntry, SystemView, TargetKind,
        TargetRef, TaskEntry,
    };

    use super::*;

    fn target(name: &str) -> TargetRef {
        TargetRef {
            kind: TargetKind::Bin,
            name: name.to_string(),
        }
    }

    fn task(
        id: u32,
        name: &str,
        receiver: u32,
        spawner: u32,
        priority: u16,
        capacity: usize,
        offset: u32,
    ) -> TaskEntry {
        TaskEntry {
            id,
            name: name.to_string(),
            receiver_core: receiver,
            spawner_core: spawner,
            priority,
            capacity,
            input_type: "EncryptReq".to_string(),
            fifo: FifoEntry {
                source: spawner,
                target: receiver,
                pool: None,
                offset,
                elem_size: 12,
                depth: u32::try_from(capacity + 1).unwrap(),
            },
        }
    }

    /// The three-application topology: producers 0 and 2 spawn onto 1, with
    /// distro pools for the `{0,1}` and `{1,2}` duals.
    fn three_app_view() -> SystemView {
        let mut view = SystemView {
            schema_version: 2,
            rticx_generation: "0.2".to_string(),
            topology_hash: Hash64::new(0x1234),
            layout_hash: Hash64::new(0x5678),
            apps: vec![AppEntry {
                package: "app-m7".to_string(),
                target: target("m7"),
                source_hash: Hash64::new(0xaaaa),
                core_ids: vec![0],
                external_cores: vec![1],
            }],
            cores: vec![
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
                CoreEntry {
                    global_id: 1,
                    physical_core: 1,
                    app: "app-m4".to_string(),
                    local_index: 0,
                },
            ],
            types: Vec::new(),
            tasks: vec![
                task(1, "EncryptTask", 1, 0, 3, 2, 0),
                task(2, "SensorTask", 1, 2, 4, 1, 0),
            ],
            pools: vec![
                PoolEntry {
                    id: "p01".to_string(),
                    core_a: 0,
                    core_b: 1,
                    base_from_a: 0x3004_0000,
                    base_from_b: 0x3004_0000,
                    budget: 128,
                    used: 100,
                },
                PoolEntry {
                    id: "p12".to_string(),
                    core_a: 1,
                    core_b: 2,
                    base_from_a: 0x2004_1000,
                    base_from_b: 0x3004_1000,
                    budget: 128,
                    used: 100,
                },
            ],
            doorbells: vec![
                DoorbellEntry {
                    source: 0,
                    target: 1,
                    priority: 3,
                    line: 0,
                },
                DoorbellEntry {
                    source: 2,
                    target: 1,
                    priority: 4,
                    line: 1,
                },
            ],
        };
        view.tasks[0].fifo.pool = Some("p01".to_string());
        view.tasks[1].fifo.pool = Some("p12".to_string());
        view
    }

    #[test]
    fn embeds_the_view_model_as_json() {
        let view = three_app_view();
        let html = render_html(&view);

        // The document pulls in the style, the script and the JSON payload.
        assert!(html.contains("#stage"), "style is inlined");
        assert!(html.contains("xbin-view"), "the JSON script tag is present");
        assert!(
            html.contains("function drawArrows"),
            "the script is inlined"
        );

        let json = extract_json(&html);
        assert!(json.contains("\"generation\":\"0.2\""));
        assert!(json.contains("\"topology_hash\":\"0x0000000000001234\""));
        assert!(json.contains("\"name\":\"EncryptTask\""));
        assert!(json.contains("\"name\":\"SensorTask\""));
        assert!(json.contains("\"id\":\"p01\""));
        assert!(json.contains("\"base_from_a\":805568512"));
        assert!(json.contains("\"budget\":128"));
    }

    #[test]
    fn normalizes_cores_tasks_and_pools() {
        let view = three_app_view();
        let model = ViewModel::from_view(&view);

        assert_eq!(model.stats.cores, 3);
        assert_eq!(model.stats.tasks, 2);
        assert_eq!(model.stats.pools, 2, "one panel per distro pool");
        assert_eq!(model.stats.doorbells, 2);

        // Cores are ordered by global id.
        assert_eq!(
            model.cores.iter().map(|core| core.id).collect::<Vec<_>>(),
            [0, 1, 2]
        );

        let receiver = &model.cores[1];
        assert_eq!(receiver.app, "app-m4");
        assert_eq!(
            receiver
                .executes
                .iter()
                .map(|task| (task.name.as_str(), task.priority, task.peer))
                .collect::<Vec<_>>(),
            [("EncryptTask", 3, 0), ("SensorTask", 4, 2)],
            "receivers are ordered by priority"
        );
        assert_eq!(receiver.executes[0].doorbell, Some(0));
        assert_eq!(receiver.executes[1].doorbell, Some(1));
        assert!(receiver.spawns.is_empty());

        let producer = &model.cores[0];
        assert_eq!(producer.app, "app-m7");
        assert!(producer.executes.is_empty());
        assert_eq!(
            producer
                .spawns
                .iter()
                .map(|task| task.name.as_str())
                .collect::<Vec<_>>(),
            ["EncryptTask"]
        );
        assert_eq!(
            producer.spawns[0].doorbell, None,
            "a producer stub does not carry the consumer's doorbell"
        );

        // One panel per pool; the `p01` panel carries the `0 -> 1` FIFO at its
        // pool-relative offset and keeps both cores' views.
        let p01 = &model.pools[0];
        assert_eq!(
            (p01.core_a, p01.core_b, p01.id.as_deref()),
            (0, 1, Some("p01"))
        );
        assert_eq!(p01.buffers.len(), 1);
        assert_eq!(p01.buffers[0].task_name, "EncryptTask");
        assert_eq!(p01.buffers[0].input_type, "EncryptReq");
        assert_eq!((p01.buffers[0].source, p01.buffers[0].target), (0, 1));
        assert_eq!(p01.buffers[0].total_bytes, 64 + 3 * 12);
        assert_eq!(p01.buffers[0].depth, 3);
        assert_eq!(p01.buffers[0].capacity, 2);
        assert_eq!(p01.extent, 100, "the bar scales to the occupied extent");

        // The `p12` panel aliases the two cores' views and carries the
        // `2 -> 1` FIFO.
        let p12 = &model.pools[1];
        assert_eq!((p12.core_a, p12.core_b), (1, 2));
        assert_ne!(p12.base_from_a, p12.base_from_b);
        assert_eq!((p12.buffers[0].source, p12.buffers[0].target), (2, 1));

        // Two unordered interaction pairs.
        assert_eq!(
            model
                .pairs
                .iter()
                .map(|pair| pair.key.as_str())
                .collect::<Vec<_>>(),
            ["0-1", "1-2"]
        );
    }

    #[test]
    fn exposes_a_flat_task_filter_list() {
        let view = three_app_view();
        let model = ViewModel::from_view(&view);

        let tasks: Vec<_> = model
            .tasks
            .iter()
            .map(|task| {
                (
                    task.name.as_str(),
                    task.input_type.as_str(),
                    task.direction.as_str(),
                    task.pair.as_str(),
                    task.priority,
                    task.doorbell,
                )
            })
            .collect();
        assert_eq!(
            tasks,
            [
                ("EncryptTask", "EncryptReq", "0-1", "0-1", 3, Some(0)),
                ("SensorTask", "EncryptReq", "2-1", "1-2", 4, Some(1)),
            ],
            "tasks are ordered by (source, target, name) and carry their pair"
        );
    }

    #[test]
    fn a_bidirectional_pair_yields_one_panel_with_both_buffers() {
        let mut view = three_app_view();
        view.tasks = vec![
            task(1, "Forward", 1, 0, 3, 2, 0),
            task(2, "Back", 0, 1, 3, 1, 0),
        ];
        view.tasks[0].fifo.pool = Some("p01".to_string());
        view.tasks[1].fifo.pool = Some("p01".to_string());
        view.pools = vec![PoolEntry {
            id: "p01".to_string(),
            core_a: 0,
            core_b: 1,
            base_from_a: 0x3004_0000,
            base_from_b: 0x1004_0000,
            budget: 128,
            used: 100,
        }];
        view.doorbells = vec![
            DoorbellEntry {
                source: 0,
                target: 1,
                priority: 3,
                line: 0,
            },
            DoorbellEntry {
                source: 1,
                target: 0,
                priority: 3,
                line: 0,
            },
        ];

        let model = ViewModel::from_view(&view);
        assert_eq!(
            model
                .pairs
                .iter()
                .map(|pair| pair.key.as_str())
                .collect::<Vec<_>>(),
            ["0-1"],
            "both directions of one unordered pair share a checkbox"
        );
        assert_eq!(
            model.pools.len(),
            1,
            "both directions share the dual's single panel"
        );
        let pool = &model.pools[0];
        assert_eq!(pool.buffers.len(), 2);
        assert_eq!(pool.buffers[0].task_name, "Forward");
        assert_eq!(pool.buffers[0].direction, "0-1");
        assert_eq!(pool.buffers[1].task_name, "Back");
        assert_eq!(pool.buffers[1].direction, "1-0");
    }

    #[test]
    fn a_declared_but_empty_pool_is_rendered_without_buffers() {
        let mut view = three_app_view();
        view.tasks = vec![task(1, "EncryptTask", 1, 0, 3, 2, 0)];
        view.tasks[0].fifo.pool = Some("p01".to_string());
        view.pools[1].used = 0; // `p12` is declared but unused.

        let model = ViewModel::from_view(&view);
        let empty = model
            .pools
            .iter()
            .find(|pool| pool.id.as_deref() == Some("p12"))
            .expect("the declared `p12` panel is present");
        assert!(empty.buffers.is_empty());
        assert_eq!(empty.used, 0);
        assert_eq!(empty.extent, 1, "a non-zero scale avoids a divide-by-zero");
    }

    #[test]
    fn unpooled_fifos_fall_back_to_a_direction_panel() {
        let mut view = three_app_view();
        // The task keeps `fifo.pool == None` while no capability table is
        // bound, exactly like a project without distro pools.
        view.tasks = vec![task(1, "EncryptTask", 1, 0, 3, 2, 0)];
        view.pools = Vec::new();

        let model = ViewModel::from_view(&view);
        assert_eq!(model.pools.len(), 1);
        let panel = &model.pools[0];
        assert!(panel.id.is_none(), "unpooled FIFOs get a synthetic panel");
        assert_eq!((panel.core_a, panel.core_b), (0, 1));
        assert_eq!(panel.buffers.len(), 1);
        assert_eq!(panel.buffers[0].task_name, "EncryptTask");
        assert_eq!(panel.extent, 100);
    }

    #[test]
    fn the_document_is_deterministic() {
        let view = three_app_view();
        assert_eq!(render_html(&view), render_html(&view));
    }

    #[test]
    fn escapes_angle_brackets_in_the_embedded_json() {
        let mut view = three_app_view();
        view.tasks[0].name = "Task</script><script>alert(1)".to_string();
        let html = render_html(&view);

        assert!(
            !html.contains("</script><script>alert(1)"),
            "the payload cannot close the embedding script element"
        );
        assert!(html.contains("\\u003c/script"));
    }

    /// Extracts the text of the embedded `application/json` script.
    fn extract_json(html: &str) -> String {
        let marker = "id=\"xbin-view\">";
        let start = html.find(marker).expect("the JSON script tag") + marker.len();
        let end = html[start..]
            .find("</script>")
            .expect("the JSON script is closed")
            + start;
        html[start..end].to_string()
    }
}
