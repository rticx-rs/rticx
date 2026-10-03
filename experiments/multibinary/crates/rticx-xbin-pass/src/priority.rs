//! Build-phase priority-line validation (M6-T1).
//!
//! Plan §4/§8: on one target core, each priority level belongs to exactly one
//! origin core. The driver's `sync` merge enforces the rule across the
//! cross-binary receivers of the whole project (producer-vs-producer,
//! `MergeError::PriorityConflict`); the target application's own
//! `#[sw_task]`/`#[async_task]` declarations only exist at build time, so
//! this check extends the same rule to them:
//!
//! - a cross-binary receiver that would share its `(target core, priority)`
//!   line with a core-local software/async task, with an in-app `spawn_by`
//!   task, or with a receiver from a different producer core is a hard
//!   compile error on the receiver declaration, naming the conflicting task;
//! - receivers from the same producer core may share a line (they drain
//!   through the same dispatcher), and the same priority on different target
//!   cores is independent.
//!
//! The check never shifts priorities; the user moves the task. `sync` cannot
//! see local tasks, which is why this runs at `build` only.

use std::collections::BTreeMap;

use proc_macro2::Span;
use rticx_xbin_proto::{AppEntry, SystemView};

use crate::parse::{LocalTaskKind, ModuleDecls};

/// One task occupying a `(target core, priority)` line.
struct LineTask<'a> {
    /// Task name.
    name: &'a str,
    /// Global id of the core allowed to spawn the task (its origin).
    origin_core: u32,
    /// Span of the declaration, for error reporting.
    span: Span,
    /// `Some(kind)` for a local declaration, `None` for a cross-binary
    /// receiver.
    kind: Option<LocalTaskKind>,
}

impl LineTask<'_> {
    /// Whether this entry is a cross-binary receiver.
    fn is_cross(&self) -> bool {
        self.kind.is_none()
    }
}

/// Rejects a cross-binary receiver that shares its priority line with a task
/// from another origin core (M6-T1).
///
/// `decls.local_tasks` carry the application's own `#[sw_task]`/`#[async_task]`
/// declarations (parsed by [`crate::parse::parse_module`]); the cross
/// receivers come from the synced view, filtered to the application's cores.
pub(crate) fn validate_priority_lines(
    view: &SystemView,
    application: &AppEntry,
    decls: &ModuleDecls,
) -> syn::Result<()> {
    let mut lines: BTreeMap<(u32, u16), Vec<LineTask<'_>>> = BTreeMap::new();

    for task in &decls.local_tasks {
        lines
            .entry((task.target_core, task.priority))
            .or_default()
            .push(LineTask {
                name: &task.name,
                origin_core: task.origin_core,
                span: task.span,
                kind: Some(task.kind),
            });
    }

    for task in &view.tasks {
        if !application.core_ids.contains(&task.receiver_core) {
            continue;
        }
        lines
            .entry((task.receiver_core, task.priority))
            .or_default()
            .push(LineTask {
                name: &task.name,
                origin_core: task.spawner_core,
                span: decls
                    .receiver_spans
                    .get(&task.name)
                    .copied()
                    .unwrap_or_else(Span::call_site),
                kind: None,
            });
    }

    for ((target, priority), tasks) in &lines {
        for (index, task) in tasks.iter().enumerate() {
            if !task.is_cross() {
                continue;
            }
            let Some(other) = tasks.iter().enumerate().find_map(|(other_index, other)| {
                (other_index != index && other.origin_core != task.origin_core).then_some(other)
            }) else {
                continue;
            };
            return Err(conflict(*target, *priority, task, other));
        }
    }
    Ok(())
}

/// Builds the hard error of one priority-line collision, anchored at the
/// cross-binary receiver and naming the conflicting declaration.
fn conflict(
    target: u32,
    priority: u16,
    receiver: &LineTask<'_>,
    other: &LineTask<'_>,
) -> syn::Error {
    let other = match other.kind {
        Some(kind) => format!(
            "the {} `{}` (spawned by global core {})",
            kind.label(),
            other.name,
            other.origin_core
        ),
        None => format!(
            "the cross-binary receiver `{}` (spawned by global core {})",
            other.name, other.origin_core
        ),
    };
    syn::Error::new(
        receiver.span,
        format!(
            "cross-binary receiver `{}` is scheduled at priority {priority} on global core \
             {target}, but {other} already owns that priority line; on one target core each \
             priority level belongs to exactly one origin core — move `{}` to another priority",
            receiver.name, receiver.name
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rticx_xbin_proto::{AppEntry, Hash64, SystemView, TargetRef, TaskEntry};

    fn application(core_ids: &[u32]) -> AppEntry {
        AppEntry {
            package: "app".to_string(),
            target: TargetRef::bin("app"),
            source_hash: Hash64::ZERO,
            core_ids: core_ids.to_vec(),
            external_cores: Vec::new(),
        }
    }

    fn local(name: &str, priority: u16, target: u32, origin: u32) -> crate::parse::LocalTaskDecl {
        crate::parse::LocalTaskDecl {
            name: name.to_string(),
            kind: LocalTaskKind::Software,
            priority,
            target_core: target,
            origin_core: origin,
            span: Span::call_site(),
        }
    }

    fn decls(local_tasks: Vec<crate::parse::LocalTaskDecl>) -> ModuleDecls {
        ModuleDecls {
            local_tasks,
            ..Default::default()
        }
    }

    fn view_with(tasks: Vec<(u32, u32, u16)>) -> SystemView {
        let mut view = SystemView::empty("0.2");
        view.tasks = tasks
            .into_iter()
            .enumerate()
            .map(|(index, (receiver, spawner, priority))| TaskEntry {
                id: index as u32 + 1,
                name: format!("Cross{index}"),
                receiver_core: receiver,
                spawner_core: spawner,
                priority,
                capacity: 1,
                input_type: "Msg".to_string(),
                fifo: rticx_xbin_proto::FifoEntry {
                    source: spawner,
                    target: receiver,
                    pool: None,
                    offset: 0,
                    elem_size: 4,
                    depth: 2,
                },
            })
            .collect();
        view
    }

    #[test]
    fn a_lone_receiver_line_passes() {
        let view = view_with(vec![(1, 0, 3)]);
        validate_priority_lines(&view, &application(&[1]), &decls(Vec::new()))
            .expect("a single origin per line");
    }

    #[test]
    fn a_local_task_collision_is_rejected() {
        let view = view_with(vec![(1, 0, 3)]);
        let error = validate_priority_lines(
            &view,
            &application(&[1]),
            &decls(vec![local("Local", 3, 1, 1)]),
        )
        .expect_err("a core-local task owns the line")
        .to_string();
        assert!(error.contains("cross-binary receiver `Cross0`"), "{error}");
        assert!(
            error.contains("the software task `Local` (spawned by global core 1)"),
            "{error}"
        );
    }

    #[test]
    fn a_different_target_does_not_conflict() {
        let view = view_with(vec![(1, 0, 3)]);
        validate_priority_lines(
            &view,
            &application(&[1]),
            &decls(vec![local("Local", 3, 0, 0)]),
        )
        .expect("different target cores are independent");
    }

    #[test]
    fn receivers_from_different_producers_conflict() {
        let view = view_with(vec![(1, 0, 3), (1, 2, 3)]);
        let error = validate_priority_lines(&view, &application(&[1]), &decls(Vec::new()))
            .expect_err("two producer cores must not share a line")
            .to_string();
        assert!(
            error.contains("the cross-binary receiver `Cross1` (spawned by global core 2)"),
            "{error}"
        );
    }
}
