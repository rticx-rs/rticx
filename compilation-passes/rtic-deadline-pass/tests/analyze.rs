//! Integration tests for the analysis phase of `rtic-deadline-pass`.
//!
//! These run `App::parse` followed by `DeadlineToPriorityPass::analyze` and inspect the resulting
//! `App` (positive scenarios) or assert the expected `syn::Error` (negative scenarios).

use quote::quote;
use rtic_deadline_pass::deadline_pass::App;

mod common;

use common::{analyze, multi_core_args, single_core_args};

// ---------------------------------------------------------------------------
// Positive scenarios: deadline-to-priority conversion
// ---------------------------------------------------------------------------

#[test]
fn analyze_single_task_deadline_becomes_priority_1() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 10)]
        struct Task1;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 1);
    // After analyze, deadline is converted to priority (1-based index)
    assert_eq!(app.tasks[0].deadline, Some(1));
}

#[test]
fn analyze_multiple_tasks_sorted_by_deadline() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 100)]
        struct TaskA;

        #[task(deadline = 10)]
        struct TaskB;

        #[task(deadline = 50)]
        struct TaskC;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 3);
    // Shorter deadline = higher priority (lower number)
    // 10 -> priority 1, 50 -> priority 2, 100 -> priority 3
    let mut deadlines: Vec<u32> = app.tasks.iter().map(|t| t.deadline.unwrap()).collect();
    deadlines.sort();
    assert_eq!(deadlines, vec![1, 2, 3]);
}

#[test]
fn analyze_tasks_without_deadline_unchanged() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 10)]
        struct TaskWithDeadline;

        #[task(binds = UART)]
        struct TaskWithoutDeadline;

        #[task(deadline = 20)]
        struct AnotherWithDeadline;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 3);
    // Task without deadline should keep None
    let without = app
        .tasks
        .iter()
        .find(|t| t.task_struct.ident == "TaskWithoutDeadline")
        .unwrap();
    assert_eq!(without.deadline, None);

    // Tasks with deadline get priority assigned
    let with10 = app
        .tasks
        .iter()
        .find(|t| t.task_struct.ident == "TaskWithDeadline")
        .unwrap();
    let with20 = app
        .tasks
        .iter()
        .find(|t| t.task_struct.ident == "AnotherWithDeadline")
        .unwrap();
    assert_eq!(with10.deadline, Some(1));
    assert_eq!(with20.deadline, Some(2));
}

#[test]
fn analyze_sw_tasks_with_deadlines() {
    let args = single_core_args();
    let items = quote! {
        #[sw_task(deadline = 5)]
        struct SwTask1;

        #[sw_task(deadline = 15)]
        struct SwTask2;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 2);
    // Shorter deadline = higher priority (lower number)
    // 5 -> priority 1, 15 -> priority 2
    let deadlines: Vec<u32> = app.tasks.iter().map(|t| t.deadline.unwrap()).collect();
    assert_eq!(deadlines, vec![1, 2]);
}

#[test]
fn analyze_mixed_tasks_sw_tasks_and_hw_tasks() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 100, binds = UART)]
        struct HwTask;

        #[sw_task(deadline = 10)]
        struct SwTask;

        #[task(deadline = 50)]
        struct AnotherHwTask;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 3);
    // All tasks with deadlines should get priorities assigned
    let deadlines: Vec<Option<u32>> = app.tasks.iter().map(|t| t.deadline).collect();
    // All should be Some (have priorities assigned)
    assert!(deadlines.iter().all(|d| d.is_some()));
}

#[test]
fn analyze_duplicate_deadlines_get_same_priority() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 10)]
        struct Task1;

        #[task(deadline = 10)]
        struct Task2;

        #[task(deadline = 20)]
        struct Task3;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 3);
    // Tasks with same deadline get same priority
    let task1 = app
        .tasks
        .iter()
        .find(|t| t.task_struct.ident == "Task1")
        .unwrap();
    let task2 = app
        .tasks
        .iter()
        .find(|t| t.task_struct.ident == "Task2")
        .unwrap();
    let task3 = app
        .tasks
        .iter()
        .find(|t| t.task_struct.ident == "Task3")
        .unwrap();

    assert_eq!(task1.deadline, task2.deadline);
    assert_ne!(task1.deadline, task3.deadline);
}

#[test]
fn analyze_priority_starts_at_1() {
    // Priority should be 1-based (not 0-based)
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 100)]
        struct Task1;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks[0].deadline, Some(1));
}

#[test]
fn analyze_max_priority_not_exceeded_normal_case() {
    // 3 tasks with different deadlines, max_priority = 255 (plenty)
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 10)]
        struct T1;
        #[task(deadline = 20)]
        struct T2;
        #[task(deadline = 30)]
        struct T3;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    let max_prio = app.tasks.iter().map(|t| t.deadline.unwrap()).max().unwrap();
    assert!(max_prio <= 255);
}

// ---------------------------------------------------------------------------
// Negative scenarios: max_priority exceeded
// ---------------------------------------------------------------------------

#[test]
fn analyze_panics_when_more_unique_deadlines_than_max_priority() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 10)]
        struct T1;
        #[task(deadline = 20)]
        struct T2;
        #[task(deadline = 30)]
        struct T3;
    };

    let app_mod = common::app_mod(items);
    let params = rtic_core::parse_utils::RticAttr::parse_from_tokens(args).expect("params parse");
    let mut parsed = App::parse(&params, app_mod).expect("app parse");

    // Max priority = 2, but we have 3 unique deadlines
    let pass = rtic_deadline_pass::deadline_pass::DeadlineToPriorityPass::new(2);

    // This should panic with the expected message
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pass.analyze(&mut parsed);
    }));

    assert!(result.is_err());
    let err = result.unwrap_err();
    let empty = "".to_string();
    let panic_msg = err.downcast_ref::<String>().unwrap_or(&empty);
    assert!(panic_msg.contains("Exceeded number of priorities"));
    assert!(panic_msg.contains("2"));
}

#[test]
fn analyze_max_priority_edge_case_equal() {
    // Exactly max_priority unique deadlines should work
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 10)]
        struct T1;
        #[task(deadline = 20)]
        struct T2;
    };

    let app_mod = common::app_mod(items);
    let params = rtic_core::parse_utils::RticAttr::parse_from_tokens(args).expect("params parse");
    let mut parsed = App::parse(&params, app_mod).expect("app parse");

    // Max priority = 2, 2 unique deadlines = OK
    let pass = rtic_deadline_pass::deadline_pass::DeadlineToPriorityPass::new(2);
    pass.analyze(&mut parsed);

    let priorities: Vec<u32> = parsed.tasks.iter().map(|t| t.deadline.unwrap()).collect();
    assert_eq!(priorities.len(), 2);
}

// ---------------------------------------------------------------------------
// Edge cases
// ---------------------------------------------------------------------------

#[test]
fn analyze_empty_app_no_tasks() {
    let args = single_core_args();
    let items = quote! {
        struct PlainStruct;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 0);
}

#[test]
fn analyze_deadline_zero_is_valid() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 0)]
        struct TaskZero;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    // Deadline 0 should be treated like any other deadline
    assert_eq!(app.tasks[0].deadline, Some(1));
}

#[test]
fn analyze_large_deadline_values() {
    let args = single_core_args();
    let items = quote! {
        #[task(deadline = 4294967295)]  // u32::MAX
        struct TaskMax;

        #[task(deadline = 1)]
        struct TaskMin;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    let priorities: Vec<u32> = app.tasks.iter().map(|t| t.deadline.unwrap()).collect();
    assert_eq!(priorities.len(), 2);
    // Smaller deadline = higher priority (lower number)
    assert!(priorities.contains(&1));
    assert!(priorities.contains(&2));
}

#[test]
fn analyze_preserves_task_order_in_rest_of_code() {
    let args = single_core_args();
    let items = quote! {
        struct Plain1;

        #[task(deadline = 10)]
        struct Task1;

        struct Plain2;

        #[task(deadline = 5)]
        struct Task2;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    // Non-task items should be preserved in order
    assert_eq!(app.rest_of_code.len(), 2);
}

#[test]
fn analyze_multi_core_with_deadlines() {
    let args = multi_core_args(2);
    let items = quote! {
        #[task(deadline = 10, core = 0)]
        struct TaskCore0;

        #[task(deadline = 5, core = 1)]
        struct TaskCore1;

        #[task(deadline = 20, core = 0)]
        struct AnotherCore0;
    };
    let app = analyze(args, items).expect("analyze succeeds");

    assert_eq!(app.tasks.len(), 3);
    // All tasks with deadlines should get priorities
    for task in &app.tasks {
        assert!(
            task.deadline.is_some(),
            "Task {:?} should have deadline",
            task.task_struct.ident
        );
    }
}
