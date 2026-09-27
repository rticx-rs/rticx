//! Runtime of the fixture's mock distribution (M4-T1).
//!
//! This is the normal-library half of the mock distribution; the proc-macro
//! half lives in `xbin-mock-distro`. It owns the process-global [`MockSystem`]
//! that stands in for the project's shared-memory region and re-exports the
//! in-tree runtime items the generated code refers to, so an application only
//! depends on this crate and the generated `ipc-types`.
//!
//! Because the system is process-global, two fixture applications expanded
//! into one process (the M4-T2 end-to-end test) share the same regions while
//! each gets the backend handle of its own core through [`backend_for`].

use std::sync::LazyLock;

pub use rticx_xbin_mock::{MockBackend, MockSystem};
pub use rticx_xbin_rt as xbin_rt;

/// Software-task trait implemented by native cross-binary receivers.
///
/// A real distribution gets this trait from `rticx-sw-pass` (the `swtasks`
/// feature); the mock distribution binds no software pass, so it provides the
/// trait itself. The generated receiver `#[task(.., task_trait = RticSwTask)]`
/// and its dispatcher enforce the user's implementation. `Self::SpawnInput`
/// is the IDL type travelling through the task's FIFO.
pub trait RticSwTask {
    /// Spawn input of the task (a generated `ipc_types` message or enum).
    type SpawnInput;

    /// Executes one spawn of the task on the receiver core.
    fn exec(&mut self, input: Self::SpawnInput);
}

/// The fixture project's IPC region, in process memory.
///
/// The regions are declared with the same `(source, target)` pair and size as
/// `rticx.toml`; the mock ignores the declared addresses and uses the backing
/// array's own.
pub fn system() -> &'static MockSystem {
    static SYSTEM: LazyLock<MockSystem> = LazyLock::new(|| {
        let mut system = MockSystem::new();
        system
            .add_region(0, 1, 4096)
            .expect("the fixture declares the `0->1` region");
        system
    });
    &SYSTEM
}

/// Returns the backend handle of global core `core`.
pub fn backend_for(core: u32) -> MockBackend {
    system().backend(core)
}
