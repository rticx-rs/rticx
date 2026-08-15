//! High-level orchestration of the `expand` and `restore` flows.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::beautify;
use crate::cargo_meta::{
    CheckOptions, Metadata, Target, clean_package, default_expansion_dir, resolve_target, run_check,
};
use crate::detect;

pub struct ExpandOptions<'a> {
    pub manifest_path: &'a Path,
    pub bin: Option<&'a str>,
    pub example: Option<&'a str>,
    pub file: Option<&'a Path>,
    pub target: Option<&'a str>,
    pub features: Option<&'a str>,
    pub all_features: bool,
    pub no_default_features: bool,
    pub expand_passes: Option<&'a Path>,
    pub output_dir: Option<&'a Path>,
    pub merge: bool,
    pub force: bool,
    pub cargo_args: &'a [String],
    pub verbose: bool,
}

pub struct ExpandReport {
    pub check_succeeded: bool,
    pub expansion_dir: PathBuf,
    pub expansion_files: Vec<PathBuf>,
    pub merged_file: Option<PathBuf>,
    pub backup_file: Option<PathBuf>,
}

pub fn expand(opts: &ExpandOptions<'_>) -> Result<ExpandReport, String> {
    let metadata = Metadata::load(opts.manifest_path)?;
    let package = metadata.root_package(opts.manifest_path)?;
    let target = resolve_target(package, opts.bin, opts.example)?;

    if opts.verbose {
        eprintln!(
            "[rticx-expand] target `{}` ({}), edition {}",
            target.name,
            target.src_path.display(),
            package.edition
        );
    }

    // The expansion directory is either the default (<target>/rticx-expand) or
    // an explicit override; the override is passed through to rticx-core.
    let expansion_dir = match opts.output_dir {
        Some(dir) => dir.to_path_buf(),
        None => default_expansion_dir(&metadata),
    };

    // Optional per-stage snapshot directory (must be absolute: the macro runs
    // with the crate directory as its working directory).
    let pass_dir = opts
        .expand_passes
        .map(|dir| std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()));
    if let Some(dir) = &pass_dir {
        clean_pass_snapshots(dir);
    }

    // Force the proc macro to run again: env vars are invisible to cargo
    // fingerprints, so a cached `cargo check` would not re-expand.
    clean_package(opts.manifest_path, &package.name)?;

    eprintln!("[rticx-expand] running `cargo check` (expansion enabled)…");
    let check_opts = CheckOptions {
        manifest_path: opts.manifest_path,
        target: opts.target,
        bin: opts.bin,
        example: opts.example,
        features: opts.features,
        all_features: opts.all_features,
        no_default_features: opts.no_default_features,
        extra_args: opts.cargo_args,
        pass_dir: pass_dir.as_deref(),
        output_dir: opts.output_dir,
    };
    let status = run_check(&check_opts)?;
    if !status.success() {
        eprintln!(
            "[rticx-expand] `cargo check` finished with errors — expanding as far as the pipeline reached."
        );
    }

    // Collect what the pipeline wrote.
    let stem_source = opts
        .file
        .map(Path::to_path_buf)
        .unwrap_or_else(|| target.src_path.clone());
    let stem = stem_source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned());

    let expansion_files = collect_expansion_files(&expansion_dir, stem.as_deref())?;
    if expansion_files.is_empty() {
        return Err(format!(
            "no expansion files were written to {}.\n\
             This means the `#[…::app]` macro never ran: either the crate failed \
             to compile before reaching it, or the macro itself panicked.\n\
             Review the `cargo check` output above; your sources were left untouched.",
            expansion_dir.display()
        ));
    }

    // Beautify everything the pipeline produced.
    for file in &expansion_files {
        let raw = std::fs::read_to_string(file)
            .map_err(|e| format!("failed to read {}: {e}", file.display()))?;
        let mut pretty = beautify::beautify(&raw);
        if file
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_expanded.rs"))
        {
            pretty = inject_module_allows(&pretty);
        }
        std::fs::write(file, pretty)
            .map_err(|e| format!("failed to write {}: {e}", file.display()))?;
        format_file(file, &package.edition);
    }

    // Beautify the per-stage snapshots so they diff cleanly.
    if let Some(dir) = &pass_dir {
        let pass_snapshots = beautify_pass_snapshots(dir, &package.edition);
        eprintln!(
            "[rticx-expand] stage snapshots in {} ({} files):",
            dir.display(),
            pass_snapshots.len()
        );
        for file in &pass_snapshots {
            eprintln!("  {}", file.display());
        }
        eprintln!(
            "[rticx-expand] diff consecutive snapshots to see each stage's changes, e.g.:\n  \
             diff -u {}/00_*.rs {}/01_*.rs",
            dir.display(),
            dir.display()
        );
    }

    let full = expansion_files
        .iter()
        .find(|f| {
            stem.as_ref().is_some_and(|s| {
                f.file_name().and_then(|n| n.to_str()) == Some(&format!("{s}_expanded.rs"))
            })
        })
        .cloned()
        .or_else(|| {
            // App module may live in a different file than the target src_path;
            // fall back to the only full expansion available.
            let mut fulls: Vec<_> = expansion_files
                .iter()
                .filter(|f| {
                    f.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with("_expanded.rs"))
                })
                .cloned()
                .collect();
            if fulls.len() == 1 { fulls.pop() } else { None }
        });

    let mut report = ExpandReport {
        check_succeeded: status.success(),
        expansion_dir,
        expansion_files,
        merged_file: None,
        backup_file: None,
    };

    // Default behavior: print the complete expanded file to stdout, like
    // `cargo expand`. With --merge, splice it into the user's source file.
    if !opts.merge {
        return print_expansions(opts, &report, full.as_deref(), target);
    }

    let Some(full) = full else {
        eprintln!(
            "[rticx-expand] no full expansion produced (the pipeline did not reach code generation);\
             intermediate files may still be available in {}.",
            report.expansion_dir.display()
        );
        return Ok(report);
    };

    let app_file = resolve_app_file(opts, target, &full)?.ok_or_else(|| {
        "could not locate the file containing the `#[…::app]` module; \
         pass it explicitly with --file."
            .to_string()
    })?;

    let expansion_text = std::fs::read_to_string(&full)
        .map_err(|e| format!("failed to read {}: {e}", full.display()))?;
    let (merged_file, backup_file) = merge_into(&app_file, &expansion_text, opts.force)?;
    format_merged_file(&merged_file, &package.edition);

    report.merged_file = Some(merged_file.clone());
    report.backup_file = Some(backup_file.clone());

    eprintln!(
        "[rticx-expand] merged expansion into {}",
        merged_file.display()
    );
    eprintln!("[rticx-expand]   backup saved to {}", backup_file.display());
    if !report.check_succeeded {
        eprintln!(
            "[rticx-expand] note: `cargo check` reported errors above. The merged file is still\
             useful for inspection/debugging, or fix the errors and re-expand."
        );
    }
    eprintln!(
        "[rticx-expand] to restore the original source: cargo rticx-expand restore --manifest-path {}",
        opts.manifest_path.display()
    );

    Ok(report)
}

/// Prints the complete expanded file to stdout (like `cargo expand`): the
/// expansion spliced into the user's source file, so surrounding code
/// (`#![no_std]`, imports, statics, trailing items) is preserved.
/// Informational messages go to stderr so stdout can be piped into a file.
fn print_expansions(
    opts: &ExpandOptions<'_>,
    report: &ExpandReport,
    full: Option<&Path>,
    target: &Target,
) -> Result<ExpandReport, String> {
    eprintln!(
        "[rticx-expand] expansions written to {}",
        report.expansion_dir.display()
    );

    let Some(full) = full else {
        return Err(
            "the pipeline did not reach code generation, so there is no full expansion.\n\
             Review the `cargo check` output above. If a compilation pass failed, re-run \
             with `--expand-passes <dir>` to inspect the intermediate snapshots."
                .to_string(),
        );
    };
    let expansion_text = std::fs::read_to_string(full)
        .map_err(|e| format!("failed to read {}: {e}", full.display()))?;
    match resolve_app_file(opts, target, full) {
        Ok(Some(app_file)) => {
            let original = std::fs::read_to_string(&app_file)
                .map_err(|e| format!("failed to read {}: {e}", app_file.display()))?;
            let merged_text = splice_expansion(&original, &expansion_text, &app_file)?;
            print_text(&merged_text);
            eprintln!(
                "[rticx-expand] printed the complete expanded file for {} (source untouched); \
                 use --merge to write it back into the source file.",
                app_file.display()
            );
        }
        Ok(None) => {
            eprintln!(
                "[rticx-expand] warning: could not locate the source file containing the \
                 `#[…::app]` module; printing the expansion module only. \
                 Pass the file with --file to print the complete file."
            );
            print_file(full)?;
        }
        Err(err) => {
            eprintln!("[rticx-expand] warning: {err}; printing the expansion module only.");
            print_file(full)?;
        }
    }

    Ok(ExpandReport {
        check_succeeded: report.check_succeeded,
        expansion_dir: report.expansion_dir.clone(),
        expansion_files: report.expansion_files.clone(),
        merged_file: None,
        backup_file: None,
    })
}

/// Prints a file's content to stdout, guaranteeing a trailing newline.
fn print_file(path: &Path) -> Result<(), String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    print_text(&content);
    Ok(())
}

/// Prints text to stdout, guaranteeing a trailing newline.
fn print_text(text: &str) {
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
}

/// Resolves which user source file contains the `#[…::app]` module:
/// `--file`, then the target's source file, then a crate-wide search by the
/// expansion's file stem. Returns `Ok(None)` when the file cannot be located.
fn resolve_app_file(
    opts: &ExpandOptions<'_>,
    target: &Target,
    full: &Path,
) -> Result<Option<PathBuf>, String> {
    if let Some(file) = opts.file {
        return Ok(Some(file.to_path_buf()));
    }
    let expected = target.src_path.clone();
    if source_has_app_item(&expected)? {
        return Ok(Some(expected));
    }
    let stem = full
        .file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_suffix("_expanded"))
        .unwrap_or_default();
    let dir = crate_dir(opts.manifest_path);
    Ok(find_source_file(dir, stem))
}

/// True for per-stage snapshot file names (`NN_label.rs` with a two-or-more
/// digit pipeline index), e.g. `00_original.rs`, `01_SoftwareTasks.rs`.
fn is_snapshot_name(name: &str) -> bool {
    let Some(rest) = name.strip_suffix(".rs") else {
        return false;
    };
    let Some((digits, label)) = rest.split_once('_') else {
        return false;
    };
    digits.len() >= 2
        && digits.chars().all(|c| c.is_ascii_digit())
        && !label.is_empty()
        && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Removes snapshot files left over from a previous run so the directory only
/// reflects the latest pipeline. Warns when the directory also contains
/// unrelated files.
fn clean_pass_snapshots(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut removed = 0;
    let mut other = 0;
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if is_snapshot_name(&name) {
            if std::fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        } else {
            other += 1;
        }
    }
    if removed > 0 {
        eprintln!(
            "[rticx-expand] removed {removed} stale snapshot(s) from {}",
            dir.display()
        );
    }
    if other > 0 {
        eprintln!(
            "[rticx-expand] note: {} contains {other} non-snapshot file(s); consider a dedicated directory (e.g. target/passes).",
            dir.display()
        );
    }
}

/// Beautifies and formats the snapshot files in `dir` (in lexical order, which
/// equals pipeline order), returning their paths.
fn beautify_pass_snapshots(dir: &Path, edition: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_snapshot_name)
        })
        .collect();
    files.sort();
    for file in &files {
        let Ok(raw) = std::fs::read_to_string(file) else {
            continue;
        };
        let pretty = beautify::beautify(&raw);
        let _ = std::fs::write(file, pretty);
        format_file(file, edition);
    }
    files
}

/// All files the pipeline wrote for this target, beautified-friendly listing.
fn collect_expansion_files(dir: &Path, stem: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir);
    let Ok(entries) = entries else {
        return Ok(files);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let keep = match stem {
            Some(stem) => name.starts_with(&format!("{stem}_")),
            None => name.ends_with("_expanded.rs") || name.contains("_pass_"),
        };
        if keep && name.ends_with(".rs") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// True when `file` contains an `#[…::app]` module.
fn source_has_app_item(file: &Path) -> Result<bool, String> {
    let Ok(source) = std::fs::read_to_string(file) else {
        return Ok(false);
    };
    Ok(!detect::find_app_items(&source).is_empty())
}

/// Searches the crate (skipping `target/` and hidden dirs) for `<stem>.rs`.
fn find_source_file(dir: &Path, stem: &str) -> Option<PathBuf> {
    let wanted = format!("{stem}.rs");
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if name == "target" || name.starts_with('.') {
                    continue;
                }
                stack.push(path);
            } else if name == wanted {
                return Some(path);
            }
        }
    }
    None
}

/// Replaces the `#[…::app] mod …` item in `original` with `expansion`,
/// preserving everything before it (inner attributes, imports, statics, …)
/// and after it (trailing items).
fn splice_expansion(original: &str, expansion: &str, source_file: &Path) -> Result<String, String> {
    let items = detect::find_app_items(original);
    let item = match items.as_slice() {
        [] => {
            return Err(format!(
                "no `#[…::app]` module found in {}. Is the application already expanded?\
                 Use `cargo rticx-expand restore` to go back, or point at the right file with --file.",
                source_file.display()
            ));
        }
        [item] => *item,
        _ => {
            return Err(format!(
                "found {} `#[…::app]` modules in {}; expand one application per file.",
                items.len(),
                source_file.display()
            ));
        }
    };
    let mut merged = String::with_capacity(original.len() + expansion.len());
    merged.push_str(&original[..item.attr_start]);
    merged.push_str(expansion);
    if !expansion.ends_with('\n') {
        merged.push('\n');
    }
    merged.push_str(&original[item.end..]);
    Ok(merged)
}

/// Splices the expansion in place of the `#[…::app] mod …` item, keeping a
/// backup of the original. Returns (merged file, backup file).
fn merge_into(file: &Path, expansion: &str, force: bool) -> Result<(PathBuf, PathBuf), String> {
    let original = std::fs::read_to_string(file)
        .map_err(|e| format!("failed to read {}: {e}", file.display()))?;
    let merged = splice_expansion(&original, expansion, file)?;

    let backup = PathBuf::from(format!("{}.old", file.display()));
    if backup.exists() && !force {
        return Err(format!(
            "backup {} already exists. Restore it first (`cargo rticx-expand restore`) or pass --force to overwrite.",
            backup.display()
        ));
    }
    std::fs::write(&backup, &original)
        .map_err(|e| format!("failed to write backup {}: {e}", backup.display()))?;
    std::fs::write(file, &merged)
        .map_err(|e| format!("failed to write {}: {e}", file.display()))?;

    Ok((file.to_path_buf(), backup))
}

/// rustc exempts proc-macro-generated code from several deny-by-default /
/// warn-by-default lints (notably the edition-2024 `static_mut_refs` deny),
/// so the same code written out by hand would fail or warn. Inject explicit
/// allows inside the generated module so the merged file behaves exactly like
/// the macro expansion. Idempotent.
fn inject_module_allows(text: &str) -> String {
    if text.contains("rticx-expand:") {
        return text.to_string();
    }
    let Some(brace) = text.find('{') else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len() + 200);
    out.push_str(&text[..=brace]);
    out.push_str(
        "\n#![allow(static_mut_refs, unused_imports, unused_variables, dead_code, non_camel_case_types)] \
         // rticx-expand: rustc exempts macro-generated code from these lints; written-out code needs explicit allows\n",
    );
    out.push_str(&text[brace + 1..]);
    out
}

/// Directory containing `manifest_path`, or `.` when it has no parent.
fn crate_dir(manifest_path: &Path) -> &Path {
    manifest_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Runs `rustfmt` on a single file. Failures are warnings, not errors: the
/// file stays valid Rust either way.
fn format_file(file: &Path, edition: &str) {
    let status = Command::new("rustfmt")
        .args(["--edition", edition])
        .arg(file)
        .status();
    match status {
        Ok(status) if status.success() => {}
        _ => eprintln!(
            "[rticx-expand] warning: rustfmt failed on {}; file left unformatted.",
            file.display()
        ),
    }
}

/// Formats the merged source file after splicing.
pub fn format_merged_file(file: &Path, edition: &str) {
    format_file(file, edition);
}

pub struct RestoreOptions<'a> {
    pub manifest_path: &'a Path,
    pub file: Option<&'a Path>,
    pub remove_expansions: bool,
    pub verbose: bool,
}

pub fn restore(opts: &RestoreOptions<'_>) -> Result<(), String> {
    let crate_dir = crate_dir(opts.manifest_path);

    let backups: Vec<(PathBuf, PathBuf)> = match opts.file {
        Some(file) => {
            let old = PathBuf::from(format!("{}.old", file.display()));
            if !old.exists() {
                return Err(format!("no backup found at {}", old.display()));
            }
            vec![(file.to_path_buf(), old)]
        }
        None => {
            let mut found = Vec::new();
            collect_backups(crate_dir, &mut found);
            found
        }
    };

    if backups.is_empty() {
        return Err(format!(
            "no `*.rs.old` backups found under {}. Nothing to restore.",
            crate_dir.display()
        ));
    }
    if backups.len() > 1 {
        return Err(format!(
            "found {} backups under {}: {}. Specify the merged file with --file.",
            backups.len(),
            crate_dir.display(),
            backups
                .iter()
                .map(|(f, _)| f.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }

    let (file, backup) = &backups[0];
    if file.exists() {
        std::fs::remove_file(file)
            .map_err(|e| format!("failed to remove {}: {e}", file.display()))?;
    }
    std::fs::rename(backup, file).map_err(|e| {
        format!(
            "failed to restore {} from {}: {e}",
            file.display(),
            backup.display()
        )
    })?;
    println!("[rticx-expand] restored {}", file.display());

    if opts.remove_expansions
        && let Ok(metadata) = Metadata::load(opts.manifest_path)
    {
        let dir = default_expansion_dir(&metadata);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("failed to remove {}: {e}", dir.display()))?;
            println!("[rticx-expand] removed {}", dir.display());
        }
    }

    println!("[rticx-expand] you can now rebuild with the original RTICX sources.");
    Ok(())
}

fn collect_backups(dir: &Path, found: &mut Vec<(PathBuf, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect_backups(&path, found);
        } else if name.ends_with(".rs.old") {
            let original = PathBuf::from(name.trim_end_matches(".old"));
            found.push((dir.join(original), path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPANSION: &str = "pub mod my_app {\n    pub fn entry() {}\n}\n";

    #[test]
    fn splice_preserves_prefix_code() {
        let original = "#![no_std]\n#![no_main]\n\nuse defmt_rtt as _;\nuse panic_halt as _;\n\n#[unsafe(link_section = \".boot2\")]\n#[used]\npub static BOOT2: [u8; 256] = [0; 256];\n\n#[rticx_rp2040::app(device = pac)]\npub mod my_app {\n    #[init]\n    fn init() {}\n}\n";
        let merged = splice_expansion(original, EXPANSION, Path::new("main.rs")).unwrap();
        assert!(
            merged.starts_with(
                "#![no_std]\n#![no_main]\n\nuse defmt_rtt as _;\nuse panic_halt as _;"
            )
        );
        assert!(merged.contains("pub static BOOT2: [u8; 256] = [0; 256];"));
        assert!(merged.contains(EXPANSION));
        assert!(!merged.contains("#[rticx_rp2040::app"));
    }

    #[test]
    fn splice_preserves_suffix_code() {
        let original = "#[rticx::app()]\npub mod my_app {\n    fn init() {}\n}\n\n// a trailing item\npub fn helper() -> u32 {\n    42\n}\n";
        let merged = splice_expansion(original, EXPANSION, Path::new("main.rs")).unwrap();
        assert!(merged.contains(EXPANSION));
        assert!(merged.ends_with("pub fn helper() -> u32 {\n    42\n}\n"));
        assert!(!merged.contains("#[rticx::app"));
    }

    #[test]
    fn recognizes_snapshot_file_names() {
        assert!(is_snapshot_name("00_original.rs"));
        assert!(is_snapshot_name("01_SoftwareTasks.rs"));
        assert!(is_snapshot_name("12_core.rs"));
        assert!(is_snapshot_name("03_FailingPass_input.rs"));
        assert!(!is_snapshot_name("0_original.rs"));
        assert!(!is_snapshot_name("01_.rs"));
        assert!(!is_snapshot_name("01_weird-name.rs"));
        assert!(!is_snapshot_name("main.rs"));
        assert!(!is_snapshot_name("01_pass.rs.old"));
    }

    #[test]
    fn splice_errors_without_app_module() {
        let original = "fn main() {}\n";
        let err = splice_expansion(original, EXPANSION, Path::new("main.rs")).unwrap_err();
        assert!(err.contains("no `#[…::app]` module found"));
    }

    #[test]
    fn splice_errors_on_multiple_app_modules() {
        let original = "#[rticx::app()] mod a {}\n#[rticx::app()] mod b {}\n";
        let err = splice_expansion(original, EXPANSION, Path::new("main.rs")).unwrap_err();
        assert!(err.contains("found 2 `#[…::app]` modules"));
    }
}
