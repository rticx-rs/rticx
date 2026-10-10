//! Topology resolution from the nearest `rticx.toml`.
//!
//! `rticx.toml` is the single user-facing source of project topology. The pass
//! resolves the compiled application's [`AppTopology`] by walking up from
//! `CARGO_MANIFEST_DIR` to the nearest project file (the same walk-up codegen
//! mode uses), so the application's `core_id` no longer has to be repeated in
//! `#[app]`. Distribution macros use [`resolve_app_topology`] for their own
//! backend binding.
//!
//! A binary owns exactly one core: [`AppTopology`] is the application's global
//! core id. A standalone xbin distribution with no `rticx.toml` above it keeps
//! the [`AppTopology::identity`] fallback (`core_id = 0`): resolution returns
//! `Ok(None)` and the caller falls back. A project that exists but does not
//! list the application is a hard error.

use std::path::PathBuf;

use rticx_xbin_proto::parse_project_file;

/// The resolved topology of one application: the single global core id the
/// binary runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppTopology {
    core_id: u32,
}

impl AppTopology {
    /// Creates a topology for the application owning the global core
    /// `core_id`.
    pub fn new(core_id: u32) -> Self {
        Self { core_id }
    }

    /// The standalone single-core topology (`core_id = 0`).
    ///
    /// Used when no `rticx.toml` governs the application, so a standalone
    /// distribution keeps working unchanged.
    pub fn identity() -> Self {
        Self { core_id: 0 }
    }

    /// The application's single, globally unique core id.
    pub fn core_id(self) -> u32 {
        self.core_id
    }
}

/// An [`AppTopology`] together with the `rticx.toml` it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTopology {
    /// The resolved topology.
    pub topology: AppTopology,
    /// Path of the `rticx.toml` the topology was read from.
    pub manifest_path: PathBuf,
}

/// Resolves the topology of `package` from the nearest `rticx.toml` above
/// `CARGO_MANIFEST_DIR`.
///
/// Returns `Ok(None)` when no project is found (the caller falls back to
/// [`AppTopology::identity`]). `target` is the Cargo target name
/// (`CARGO_BIN_NAME`) when the compiler provides one; rust-analyzer's
/// proc-macro server does not, in which case the package alone selects the
/// application (`rticx.toml` rejects two applications sharing a package).
///
/// An error is returned when a project exists but does not list the
/// application, or when the project file cannot be read or parsed.
pub fn resolve_app_topology(
    package: &str,
    target: Option<&str>,
) -> syn::Result<Option<ResolvedTopology>> {
    let Some(root) = crate::project_root_from_env() else {
        return Ok(None);
    };
    let manifest_path = root.join(crate::PROJECT_MANIFEST);
    let config = parse_project_file(&manifest_path).map_err(|error| {
        crate::error(format!(
            "failed to parse the nearest `rticx.toml` (`{}`): {error}",
            manifest_path.display()
        ))
    })?;

    let application = config
        .applications()
        .iter()
        .find(|application| {
            application.package() == package
                && target.is_none_or(|target| application.target().name() == target)
        })
        .ok_or_else(|| {
            crate::error(format!(
                "the application `{package}` is not listed in the nearest `rticx.toml` (`{}`); \
                 add an `[[application]]` entry for it",
                manifest_path.display()
            ))
        })?;

    Ok(Some(ResolvedTopology {
        topology: AppTopology::new(application.core_id()),
        manifest_path,
    }))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use super::{AppTopology, resolve_app_topology};

    /// Serializes the tests that mutate the process environment.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Saves and restores `CARGO_MANIFEST_DIR` around a test.
    struct EnvGuard {
        saved: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(value: &Path) -> Self {
            let saved = std::env::var_os("CARGO_MANIFEST_DIR");
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe { std::env::set_var("CARGO_MANIFEST_DIR", value) };
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: the process environment is guarded by ENV_LOCK.
            unsafe {
                match &self.saved {
                    Some(value) => std::env::set_var("CARGO_MANIFEST_DIR", value),
                    None => std::env::remove_var("CARGO_MANIFEST_DIR"),
                }
            }
        }
    }

    fn write_project(dir: &Path, source: &str) -> PathBuf {
        let path = dir.join(crate::PROJECT_MANIFEST);
        std::fs::write(&path, source).expect("write rticx.toml");
        path
    }

    #[test]
    fn identity_is_the_standalone_single_core() {
        let topology = AppTopology::identity();
        assert_eq!(topology.core_id(), 0);
    }

    #[test]
    fn new_keeps_the_declared_core_id() {
        let topology = AppTopology::new(4);
        assert_eq!(topology.core_id(), 4);
    }

    #[test]
    fn resolves_the_nearest_project() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        // An outer project and a nearer one; the nearest must win.
        write_project(
            root,
            "schema = 1\n\
             [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\ncore_id = 9\n",
        );
        let inner = root.join("app");
        std::fs::create_dir_all(&inner).expect("inner dir");
        let manifest_path = write_project(
            &inner,
            "schema = 1\n\
             [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\ncore_id = 0\n",
        );
        let start = inner.join("src");
        std::fs::create_dir_all(&start).expect("start dir");

        let _env = EnvGuard::set(&start);
        let resolved = resolve_app_topology("app-m7", Some("m7"))
            .expect("resolution succeeds")
            .expect("a project is found");
        assert_eq!(resolved.topology.core_id(), 0, "nearest wins");
        assert_eq!(resolved.manifest_path, manifest_path);
    }

    #[test]
    fn resolves_by_package_without_a_target() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let dir = tempfile::tempdir().expect("tempdir");
        write_project(
            dir.path(),
            "schema = 1\n\
             [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\ncore_id = 0\n",
        );

        let _env = EnvGuard::set(dir.path());
        let resolved = resolve_app_topology("app-m7", None)
            .expect("resolution succeeds")
            .expect("a project is found");
        assert_eq!(resolved.topology.core_id(), 0);
    }

    #[test]
    fn missing_application_is_an_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest_path = write_project(
            dir.path(),
            "schema = 1\n\
             [[application]]\npackage = \"app-m7\"\ntarget = { kind = \"bin\", name = \"m7\" }\ncore_id = 0\n",
        );

        let _env = EnvGuard::set(dir.path());
        let error = resolve_app_topology("ghost", None)
            .expect_err("the package is not listed")
            .to_string();
        assert_eq!(
            error,
            format!(
                "the application `ghost` is not listed in the nearest `rticx.toml` (`{}`); \
                 add an `[[application]]` entry for it",
                manifest_path.display()
            )
        );
    }

    #[test]
    fn no_project_resolves_to_none() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("app").join("src");
        std::fs::create_dir_all(&nested).expect("nested dirs");

        let _env = EnvGuard::set(&nested);
        assert!(
            resolve_app_topology("app-m7", Some("m7"))
                .expect("no project is not an error")
                .is_none()
        );
    }
}
