//! `cargo xbin sync --html`: a self-contained visualization of the system
//! view.
//!
//! The driver normalizes the emitted [`SystemView`] into a small presentation
//! model (cores with the cross-binary tasks they execute and spawn, regions
//! with their FIFO buffers, and the interaction pairs) and embeds it as JSON in
//! a static HTML document. The inline script lays the page out and draws the
//! arrows; the driver itself never depends on a browser.
//!
//! Until M6.9-T8 replaces them with pool panels, the region panels are derived
//! by expanding each `system.json` pool into its two per-direction views.
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
    regions: Vec<RegionPanel>,
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
    regions: usize,
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

/// One `(source -> target)` shared-memory region and its FIFO allocation.
#[derive(Debug, Serialize)]
struct RegionPanel {
    source: u32,
    target: u32,
    pair: String,
    direction: String,
    base_from_source: u32,
    base_from_target: u32,
    size: u32,
    /// High-water mark of the allocated FIFOs, in bytes.
    used_bytes: u64,
    buffers: Vec<BufferCard>,
}

/// One task FIFO inside a region.
#[derive(Debug, Serialize)]
struct BufferCard {
    task_id: u32,
    task_name: String,
    input_type: String,
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

        // Transitional (M6.9-T5): `system.json` carries pools; until M6.9-T8
        // renders pool panels, expand each pool into its two per-direction
        // panels so the existing model and script stay unchanged.
        struct Direction {
            source: u32,
            target: u32,
            base_from_source: u32,
            base_from_target: u32,
            size: u32,
        }

        let mut directions: Vec<Direction> = Vec::with_capacity(view.pools.len() * 2);
        for pool in &view.pools {
            directions.push(Direction {
                source: pool.core_a,
                target: pool.core_b,
                base_from_source: pool.base_from_a,
                base_from_target: pool.base_from_b,
                size: pool.budget,
            });
            directions.push(Direction {
                source: pool.core_b,
                target: pool.core_a,
                base_from_source: pool.base_from_b,
                base_from_target: pool.base_from_a,
                size: pool.budget,
            });
        }
        directions.sort_by_key(|direction| (direction.source, direction.target));

        let mut panels = Vec::with_capacity(directions.len());
        let mut pairs: BTreeSet<(u32, u32)> = BTreeSet::new();
        for region in &directions {
            let mut buffers: Vec<&rticx_xbin_proto::TaskEntry> = view
                .tasks
                .iter()
                .filter(|task| {
                    task.fifo.source == region.source && task.fifo.target == region.target
                })
                .collect();
            buffers.sort_by_key(|task| task.fifo.offset);

            let mut used_bytes = 0u64;
            let mut cards = Vec::with_capacity(buffers.len());
            for task in buffers {
                let total_bytes = fifo_size(task.fifo.elem_size, task.capacity).unwrap_or(u64::MAX);
                used_bytes = used_bytes.max(u64::from(task.fifo.offset) + total_bytes);
                cards.push(BufferCard {
                    task_id: task.id,
                    task_name: task.name.clone(),
                    input_type: task.input_type.clone(),
                    offset: task.fifo.offset,
                    total_bytes,
                    elem_size: task.fifo.elem_size,
                    depth: task.fifo.depth,
                    capacity: task.capacity,
                    priority: task.priority,
                    doorbell: doorbell_of(task),
                    pair: pair_key(region.source, region.target),
                    direction: direction_key(region.source, region.target),
                });
            }

            pairs.insert(ordered_pair(region.source, region.target));
            panels.push(RegionPanel {
                source: region.source,
                target: region.target,
                pair: pair_key(region.source, region.target),
                direction: direction_key(region.source, region.target),
                base_from_source: region.base_from_source,
                base_from_target: region.base_from_target,
                size: region.size,
                used_bytes,
                buffers: cards,
            });
        }

        // A pair may also be implied by a task without a matching region in the
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

        let region_count = panels.len();
        ViewModel {
            generation: view.rticx_generation.clone(),
            topology_hash: hash_text(view.topology_hash),
            layout_hash: hash_text(view.layout_hash),
            cores,
            regions: panels,
            pairs: filters,
            tasks: task_filters,
            stats: Stats {
                cores: ids.len(),
                tasks: view.tasks.len(),
                regions: region_count,
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

    /// The three-application topology: producers 0 and 2 spawn onto 1.
    fn three_app_view() -> SystemView {
        SystemView {
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
        }
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
        assert!(json.contains("\"base_from_source\":805568512"));
    }

    #[test]
    fn normalizes_cores_tasks_and_regions() {
        let view = three_app_view();
        let model = ViewModel::from_view(&view);

        assert_eq!(model.stats.cores, 3);
        assert_eq!(model.stats.tasks, 2);
        assert_eq!(model.stats.regions, 4, "two pools, two directions each");
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

        // Each pool expands into its two per-direction panels; they keep both
        // base views and place the FIFO at its offset.
        let region = &model.regions[0];
        assert_eq!((region.source, region.target), (0, 1));
        assert_eq!(region.buffers[0].total_bytes, 64 + 3 * 12);
        assert_eq!(region.used_bytes, 100);
        assert_eq!(region.buffers[0].task_name, "EncryptTask");
        assert_eq!(region.buffers[0].input_type, "EncryptReq");
        assert_eq!(region.buffers[0].depth, 3);
        assert_eq!(region.buffers[0].capacity, 2);

        let aliased = model
            .regions
            .iter()
            .find(|region| (region.source, region.target) == (2, 1))
            .expect("the 2 -> 1 direction is present");
        assert_ne!(aliased.base_from_source, aliased.base_from_target);

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
    fn a_bidirectional_pair_yields_one_checkbox_and_two_regions() {
        let mut view = three_app_view();
        view.tasks = vec![
            task(1, "Forward", 1, 0, 3, 2, 0),
            task(2, "Back", 0, 1, 3, 1, 0),
        ];
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
        assert_eq!(model.regions.len(), 2);
        assert_eq!(model.regions[0].direction, "0-1");
        assert_eq!(model.regions[1].direction, "1-0");

        // Each direction puts the task on the correct side.
        assert_eq!(model.regions[0].buffers[0].task_name, "Forward");
        assert_eq!(model.regions[1].buffers[0].task_name, "Back");
    }

    #[test]
    fn a_declared_but_empty_pool_is_rendered_without_buffers() {
        let mut view = three_app_view();
        view.tasks = vec![task(1, "EncryptTask", 1, 0, 3, 2, 0)];
        view.pools.push(PoolEntry {
            id: "p12".to_string(),
            core_a: 1,
            core_b: 2,
            base_from_a: 0x3004_2000,
            base_from_b: 0x3004_2000,
            budget: 64,
            used: 0,
        });

        let model = ViewModel::from_view(&view);
        let empty = model
            .regions
            .iter()
            .find(|region| region.direction == "1-2")
            .expect("the declared pool's 1 -> 2 direction is present");
        assert!(empty.buffers.is_empty());
        assert_eq!(empty.used_bytes, 0);
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
