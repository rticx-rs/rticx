//! Canonical layout engine tests (M0-T3).
//!
//! The engine's output is compared against hand-written `#[repr(C)]` mirrors
//! compiled for the host: for the supported subset the canonical layout must
//! agree with `core::mem::{size_of, align_of, offset_of}`.

use core::mem::{align_of, offset_of, size_of};

use rticx_xbin_proto::idl::parse_idl_str;
use rticx_xbin_proto::layout::{Layout, Layouts, MAX_ALIGN};

fn layouts(source: &str) -> Layouts {
    let idl = parse_idl_str(source).expect("valid IDL");
    Layouts::of(&idl).expect("layout")
}

fn assert_message<const N: usize>(
    layouts: &Layouts,
    name: &str,
    size: usize,
    align: usize,
    field_offsets: [(&str, usize); N],
) {
    let message = layouts.message(name).expect("message layout");
    assert_eq!(message.layout.size, size, "`{name}` size");
    assert_eq!(message.layout.align, align, "`{name}` align");
    for (field_name, expected) in field_offsets {
        let field = message
            .fields
            .iter()
            .find(|field| field.name == field_name)
            .unwrap_or_else(|| panic!("`{name}` has no field `{field_name}`"));
        assert_eq!(field.offset, expected, "`{name}.{field_name}` offset");
    }
}

#[test]
fn max_align_is_four() {
    assert_eq!(MAX_ALIGN, 4);
}

#[test]
fn lays_out_primitives_in_declaration_order() {
    let layouts = layouts(
        r#"
        schema = 1

        [message.All]
        fields = { a = "u8", b = "u16", c = "u32", d = "i8", e = "i16", f = "i32", g = "f32" }
        "#,
    );

    // a@0 (1) -> b aligned to 2 @2 (2) -> c @4 (4) -> d @8 (1)
    // -> e aligned to 2 @10 (2) -> f @12 (4) -> g @16 (4) -> size 20.
    assert_message(
        &layouts,
        "All",
        20,
        4,
        [
            ("a", 0),
            ("b", 2),
            ("c", 4),
            ("d", 8),
            ("e", 10),
            ("f", 12),
            ("g", 16),
        ],
    );

    #[repr(C)]
    struct All {
        a: u8,
        b: u16,
        c: u32,
        d: i8,
        e: i16,
        f: i32,
        g: f32,
    }

    assert_eq!(
        layouts.message("All").unwrap().layout.size,
        size_of::<All>()
    );
    assert_eq!(
        layouts.message("All").unwrap().layout.align,
        align_of::<All>()
    );
    assert_eq!(
        layouts.message("All").unwrap().fields[3].offset,
        offset_of!(All, d)
    );
    assert_eq!(
        layouts.message("All").unwrap().fields[4].offset,
        offset_of!(All, e)
    );
    assert_eq!(
        layouts.message("All").unwrap().fields[6].offset,
        offset_of!(All, g)
    );
}

#[test]
fn nested_messages_arrays_and_enums_match_repr_c() {
    let layouts = layouts(
        r#"
        schema = 1

        [message.Point]
        fields = { x = "i32", y = "i32" }

        [message.Line]
        fields = { a = "Point", b = "Point", tag = "u8" }

        [message.Batch]
        fields = { count = "u8", xs = "[i16; 8]", state = "State" }

        [enum.State]
        variants = { Idle = 0, Busy = 1 }
        "#,
    );

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Line {
        a: Point,
        b: Point,
        tag: u8,
    }

    #[repr(u32)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    enum State {
        Idle = 0,
        Busy = 1,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Batch {
        count: u8,
        xs: [i16; 8],
        state: State,
    }

    assert_message(&layouts, "Point", 8, 4, [("x", 0), ("y", 4)]);
    // a@0, b@8, tag@16 -> trailing padding to 20.
    assert_message(&layouts, "Line", 20, 4, [("a", 0), ("b", 8), ("tag", 16)]);
    // count@0, xs aligned to 2 @2 (16 bytes), state aligned to 4 @20 -> 24.
    assert_message(
        &layouts,
        "Batch",
        24,
        4,
        [("count", 0), ("xs", 2), ("state", 20)],
    );

    assert_eq!(
        size_of::<Point>(),
        layouts.message("Point").unwrap().layout.size
    );
    assert_eq!(
        size_of::<Line>(),
        layouts.message("Line").unwrap().layout.size
    );
    assert_eq!(
        size_of::<Batch>(),
        layouts.message("Batch").unwrap().layout.size
    );
    assert_eq!(
        align_of::<Line>(),
        layouts.message("Line").unwrap().layout.align
    );
    assert_eq!(
        offset_of!(Line, tag),
        layouts.message("Line").unwrap().fields[2].offset
    );
    assert_eq!(
        offset_of!(Batch, xs),
        layouts.message("Batch").unwrap().fields[1].offset
    );
    assert_eq!(
        offset_of!(Batch, state),
        layouts.message("Batch").unwrap().fields[2].offset
    );
}

#[test]
fn nested_arrays_of_messages_and_primitives() {
    let layouts = layouts(
        r#"
        schema = 1

        [message.Point]
        fields = { x = "i32", y = "i32" }

        [message.Hold]
        fields = { pair = "[Point; 2]", bytes = "[[u8; 2]; 3]" }
        "#,
    );

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Hold {
        pair: [Point; 2],
        bytes: [[u8; 2]; 3],
    }

    // pair@0 (16), bytes@16 (6, align 1) -> 22 rounded up to 24.
    assert_message(&layouts, "Hold", 24, 4, [("pair", 0), ("bytes", 16)]);
    assert_eq!(
        size_of::<Hold>(),
        layouts.message("Hold").unwrap().layout.size
    );
    assert_eq!(
        align_of::<Hold>(),
        layouts.message("Hold").unwrap().layout.align
    );
}

#[test]
fn enum_layout_is_repr_u32() {
    let layouts = layouts(
        r#"
        schema = 1

        [enum.State]
        variants = { Idle = 0, Busy = 7 }
        "#,
    );

    let state = layouts.get_enum("State").expect("enum layout");
    assert_eq!(state.layout.size, 4);
    assert_eq!(state.layout.align, 4);
    assert_eq!(state.variants.len(), 2);
    assert_eq!(state.variants[0].name, "Idle");
    assert_eq!(state.variants[0].discriminant, 0);
    assert_eq!(state.variants[1].name, "Busy");
    assert_eq!(state.variants[1].discriminant, 7);
}

#[test]
fn zero_sized_arrays_are_rejected_before_layout() {
    let error = parse_idl_str("schema = 1\n[message.M]\nfields = { a = \"[u8; 0]\" }\n")
        .expect_err("zero-sized array");
    assert!(
        error
            .to_string()
            .contains("array length must be at least 1")
    );
}

#[test]
fn oversized_arrays_report_a_layout_overflow() {
    // 2^62 * 4 bytes overflows `usize` on any target.
    let idl =
        parse_idl_str("schema = 1\n[message.M]\nfields = { a = \"[u32; 4611686018427387904]\" }\n")
            .expect("valid IDL");
    let error = Layouts::of(&idl).expect_err("layout overflow");
    assert!(
        error
            .to_string()
            .contains("layout of `M` overflows the address space"),
        "unexpected error: {error}"
    );
}

#[test]
fn layout_of_empty_idl_is_empty() {
    let layouts = layouts("schema = 1\n");
    assert!(layouts.messages().is_empty());
    assert!(layouts.enums().is_empty());
}

#[test]
fn field_layout_reports_type_size_and_align() {
    let layouts = layouts(
        r#"
        schema = 1

        [message.M]
        fields = { single = "u32", list = "[u16; 3]" }
        "#,
    );
    let message = layouts.message("M").expect("message layout");

    assert_eq!(message.fields[0].layout, Layout { size: 4, align: 4 });
    assert_eq!(message.fields[1].layout, Layout { size: 6, align: 2 });
    assert_eq!(message.layout, Layout { size: 12, align: 4 });
}
