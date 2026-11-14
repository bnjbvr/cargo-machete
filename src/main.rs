mod printers;
mod search_unused;

use crate::printers::json::JsonPrinter;
use crate::printers::stdout::StdoutPrinter;
use crate::printers::{AnalyzedPaths, Printer};
use crate::search_unused::find_unused;
use anyhow::{Context, bail};
use rayon::prelude::*;
use std::env::current_dir;
use std::path::{Component, Path};
use std::str::FromStr;
use std::{fs, path::PathBuf};
use toml_edit::{KeyMut, TableLike};

/// Use jemalloc on 64-bit musl since musl's default allocator is very slow when used from multiple
/// threads (e.g. when using rayon).
///
/// See https://github.com/BurntSushi/ripgrep/issues/1268#issuecomment-486432534
#[cfg(all(target_env = "musl", target_pointer_width = "64"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[derive(Clone, Copy)]
pub(crate) enum UseCargoMetadata {
    Yes,
    No,
}

#[cfg(test)]
impl UseCargoMetadata {
    fn all() -> &'static [Self] {
        &[Self::Yes, Self::No]
    }
}

#[derive(argh::FromArgs)]
#[argh(description = r#"
cargo-machete: Helps find unused dependencies in a fast yet imprecise way.

Exit code:
    0:  when no unused dependencies are found
    1:  when at least one unused (non-ignored) dependency is found
    2:  on error
"#)]
struct MacheteArgs {
    /// uses cargo-metadata to figure out the dependencies' names. May be useful if some
    /// dependencies are renamed from their own Cargo.toml file (e.g. xml-rs which gets renamed
    /// xml). Try it if you get false positives!
    #[argh(switch)]
    with_metadata: bool,

    /// don't analyze anything contained in *any* target/ directories encountered.
    ///
    /// Equivalent to `--skip-patterns target`.
    #[argh(switch)]
    skip_target_dir: bool,

    /// don't analyze anything under directories which paths *end* with the passed path component. For
    /// instance, passing "target" will ignore any found paths called "target", but not a path
    /// called e.g. "suffix-target".
    ///
    /// Absolute or relative paths are ignored here; use `--skip` for these instead instead.
    ///
    /// May be passed multiple times.
    #[argh(option)]
    skip_patterns: Vec<PathBuf>,

    /// don't analyze anything under the directories specified by this path.
    ///
    /// As opposed to `--skip-patterns`, this skips the exact directories specified with this flag.
    ///
    /// Relative paths are resolved against the current directory. A path without a prefix will be considered relative to the working directory, as if starting with `./`.
    ///
    /// May be passed multiple times.
    #[argh(option)]
    skip: Vec<PathBuf>,

    /// rewrite the Cargo.toml files to automatically remove unused dependencies.
    /// Note: all dependencies flagged by cargo-machete will be removed, including false positives.
    #[argh(switch)]
    fix: bool,

    /// also search in ignored files (.gitignore, .ignore, etc.) when searching for files.
    #[argh(switch)]
    no_ignore: bool,

    /// print version.
    #[argh(switch)]
    version: bool,

    /// suppress progress and informational output (inactive when --json is passed).
    #[argh(switch, short = 'q')]
    quiet: bool,

    /// output results as JSON for tooling integration.
    #[argh(switch)]
    json: bool,

    /// paths to directories that must be scanned.
    #[argh(positional, greedy)]
    paths: Vec<PathBuf>,
}

struct ResolveSkipPaths<'a> {
    skip_target_dir: bool,
    skip_patterns: &'a mut Vec<PathBuf>,
    skip: &'a mut Vec<PathBuf>,
}

fn resolve_skip_paths(cwd: &Path, args: ResolveSkipPaths<'_>) -> anyhow::Result<()> {
    // Handle skip patterns.
    for path in &*args.skip_patterns {
        if path.is_empty() {
            // This would lead to skipping everything, so let's make it a hard error instead.
            bail!("empty skip pattern");
        }

        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        {
            bail!(
                "passing an absolute or relative directory is not allowed with `--skip-patterns`, use `--skip` instead"
            );
        }
    }

    // Add a skip pattern if we're skipping the target dir.
    if args.skip_target_dir && !args.skip_patterns.iter().any(|p| p.as_os_str() == "target") {
        args.skip_patterns.push(PathBuf::from("target"));
    }

    // Canonicalize exact skip paths.
    {
        for raw in args.skip {
            let abs_path = fs::canonicalize(cwd.join(&raw))
                .with_context(|| format!("invalid --skip path {}", raw.display()))?;
            *raw = abs_path;
        }
    }

    Ok(())
}

struct CollectPathOptions {
    /// Patterns to skip in directories.
    skip_patterns: Vec<PathBuf>,

    /// Absolute or relative directories to skip.
    exact_skip_paths: Vec<PathBuf>,

    /// Should we ignore files as specified in .gitignore (in the target directory, or any parent),
    /// and `.ignore`?
    respect_ignore_files: bool,

    // As an override to the above `respect_ignore_files`, should we use `.gitignore` overall?
    //
    // This is used only in testing, to avoid reading this repository's `.gitignore` file for
    // testing the `collect_paths()` function.
    override_respect_git_ignore: Option<bool>,
}

fn collect_paths(path: &Path, options: CollectPathOptions) -> anyhow::Result<Vec<PathBuf>> {
    // Find directory entries.
    let mut builder = ignore::WalkBuilder::new(path);

    builder.standard_filters(options.respect_ignore_files);

    if let Some(val) = options.override_respect_git_ignore {
        builder.git_ignore(val);
    }

    let exact_skip_paths = options.exact_skip_paths;
    let skip_patterns = options.skip_patterns;

    // Exact skip paths are matched exactly, so entries must be compared through the canonical form
    // of the walk root. Entries are always `path.join(relative)`, so we only canonicalize the root
    // once instead of every entry.
    let canonical_root = if !exact_skip_paths.is_empty() {
        Some(
            fs::canonicalize(path).map_err(|err| ignore::Error::WithPath {
                path: path.to_path_buf(),
                err: Box::new(ignore::Error::Io(err)),
            })?,
        )
    } else {
        None
    };

    // If the walk root is a skipped directory, or lies anywhere under one, everything it contains
    // is skipped. `Path::starts_with` compares whole components, so skipping `foo` doesn't affect
    // a root at `foobar`.
    if let Some(canonical_root) = &canonical_root
        && exact_skip_paths
            .iter()
            .any(|skip| canonical_root.starts_with(skip))
    {
        bail!("found no Cargo.toml in {}", path.display());
    }

    if !skip_patterns.is_empty() || !exact_skip_paths.is_empty() {
        // Entries are built from the walk root as given (possibly non-canonical, e.g. `src/..`),
        // so the relative part must be stripped with it, not with the canonical root.
        let walk_root = path.to_path_buf();
        builder.filter_entry(move |entry| {
            let entry_path = entry.path();

            for last_component in &skip_patterns {
                if entry_path.ends_with(last_component) {
                    return false;
                }
            }

            if let Some(canonical_root) = &canonical_root {
                let rel = entry_path.strip_prefix(&walk_root).unwrap_or(entry_path);
                let canonical_entry = canonical_root.join(rel);
                // The root itself isn't under any skipped path (checked above), so prune the exact
                // matching child directories.
                if exact_skip_paths.contains(&canonical_entry) {
                    return false;
                }
            }

            true
        });
    }

    let walker = builder.build();

    // Keep only errors and `Cargo.toml` files (filter), then map correct paths into owned
    // `PathBuf`.
    let entries = walker
        .into_iter()
        .filter(|entry| {
            entry
                .as_ref()
                .map_or(true, |entry| entry.file_name() == "Cargo.toml")
        })
        .map(|res_entry| res_entry.map(|e| e.into_path()))
        .collect::<Result<Vec<_>, _>>()?;

    if entries.is_empty() {
        bail!("found no Cargo.toml in {}", path.display());
    }

    Ok(entries)
}

/// Return true if this is run as `cargo machete`, false otherwise (`cargo-machete`, `cargo run -- ...`)
fn running_as_cargo_cmd() -> bool {
    // If run under Cargo in general, a `CARGO` environment variable is set.
    //
    // But this is also set when running with `cargo run`, which we don't want to break! In that
    // latter case, another set of cargo variables are defined, which aren't defined when just
    // running as `cargo machete`. Picked `CARGO_PKG_NAME` as one of those variables.
    //
    // So we're running under cargo if `CARGO` is defined, but not `CARGO_PKG_NAME`.
    std::env::var("CARGO").is_ok() && std::env::var("CARGO_PKG_NAME").is_err()
}

/// Runs `cargo-machete`.
/// Returns Ok with a bool whether any unused dependencies were found, or Err on errors.
fn run_machete() -> anyhow::Result<bool> {
    pretty_env_logger::init();

    let mut args: MacheteArgs = if running_as_cargo_cmd() {
        argh::cargo_from_env()
    } else {
        argh::from_env()
    };

    let printer: Box<dyn Printer> = if args.json {
        Box::new(JsonPrinter)
    } else {
        Box::new(StdoutPrinter {
            quiet: args.quiet,
            with_metadata: args.with_metadata,
        })
    };

    if args.version {
        printer.print_version(env!("CARGO_PKG_VERSION"))?;
        std::process::exit(0);
    }

    if args.paths.is_empty() {
        printer.print_paths(AnalyzedPaths::CurrentDir);
    } else {
        printer.print_paths(AnalyzedPaths::Paths(&args.paths));
    }

    if args.paths.is_empty() {
        args.paths.push(PathBuf::from("."));
    }

    resolve_skip_paths(
        &current_dir().context("couldn't read the current directory")?,
        ResolveSkipPaths {
            skip_target_dir: args.skip_target_dir,
            skip_patterns: &mut args.skip_patterns,
            skip: &mut args.skip,
        },
    )?;

    let mut has_unused_dependencies = false;

    // Be resilient in case we can't collect some paths due to errors: don't abort execution, so we
    // can run analysis for paths which didn't cause errors.
    let mut path_collection_errors = Vec::new();

    for path in args.paths {
        let manifest_path_entries = match collect_paths(
            &path,
            CollectPathOptions {
                exact_skip_paths: args.skip.clone(),
                skip_patterns: args.skip_patterns.clone(),
                respect_ignore_files: !args.no_ignore,
                override_respect_git_ignore: None,
            },
        ) {
            Ok(entries) => entries,
            Err(err) => {
                path_collection_errors.push(err);
                continue;
            }
        };

        let with_metadata = if args.with_metadata {
            UseCargoMetadata::Yes
        } else {
            UseCargoMetadata::No
        };

        // Run analysis in parallel. This will spawn new rayon tasks when dependencies are effectively
        // used by any Rust crate.
        let results = manifest_path_entries
            .par_iter()
            .filter_map(
                |manifest_path| match find_unused(manifest_path, with_metadata) {
                    Ok(Some(analysis)) => {
                        if analysis.unused.is_empty() {
                            None
                        } else {
                            Some((analysis, manifest_path))
                        }
                    }

                    Ok(None) => {
                        log::info!(
                            "{} is a virtual manifest for a workspace",
                            manifest_path.to_string_lossy()
                        );
                        None
                    }

                    Err(err) => {
                        eprintln!("error when handling {}: {:#}", manifest_path.display(), err);
                        None
                    }
                },
            )
            .collect::<Vec<_>>();

        has_unused_dependencies = has_unused_dependencies || !results.is_empty();

        // Display all the results.
        printer.print_results(&path, &results)?;

        if args.fix {
            for (analysis, path) in &results {
                let fixed = remove_dependencies(&fs::read_to_string(path)?, &analysis.unused)?;
                fs::write(path, fixed).expect("Cargo.toml write error");
            }
        }
    }

    printer.print_tail(has_unused_dependencies);

    // Report errors, if some happened during path collections.
    if !path_collection_errors.is_empty() {
        anyhow::bail!(
            "Errors when walking over directories:\n{}",
            path_collection_errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    Ok(has_unused_dependencies)
}

/// Returns dependency tables from top level and target sources.
fn get_dependency_tables(
    kv_iter: toml_edit::IterMut<'_>,
    top_level: bool,
) -> anyhow::Result<Vec<(KeyMut<'_>, &mut dyn TableLike)>> {
    let mut matched_tables = Vec::new();
    for (k, v) in kv_iter {
        match k.get() {
            "dependencies" | "build-dependencies" | "dev-dependencies" => {
                let table = v.as_table_like_mut().context(k.to_string())?;
                matched_tables.push((k, table));
            }
            // handle dependency tables inside target triples,
            // ex: `target.'cfg(unix)'.dependencies`
            // https://doc.rust-lang.org/cargo/reference/config.html#configuration-format
            "target" if top_level => {
                let target_table = v.as_table_like_mut().context("target")?;
                for (_, triple_table) in target_table
                    .iter_mut()
                    .filter(|(k, _)| k.starts_with("cfg("))
                {
                    if let Some(t) = triple_table.as_table_like_mut() {
                        let mut triple_deps = get_dependency_tables(t.iter_mut(), false)?;
                        matched_tables.append(&mut triple_deps);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(matched_tables)
}

fn remove_dependencies(manifest: &str, dependency_list: &[String]) -> anyhow::Result<String> {
    let mut manifest = toml_edit::DocumentMut::from_str(manifest)?;

    let mut matched_tables = get_dependency_tables(manifest.iter_mut(), true)?;

    for dep in dependency_list {
        let mut removed_one = false;
        for (name, table) in &mut matched_tables {
            if table.remove(dep).is_some() {
                removed_one = true;
                log::debug!("removed {name}.{dep}");
            } else {
                log::trace!("no match for {name}.{dep}");
            }
        }
        if !removed_one {
            let tables = matched_tables
                .iter()
                .map(|(k, _)| k.to_string())
                .collect::<Vec<String>>()
                .join(", ");
            bail!("{dep} not found in tables:\n\t{tables}");
        }
    }

    let serialized = manifest.to_string();
    Ok(serialized)
}

fn main() {
    let exit_code = match run_machete() {
        Ok(false) => 0,
        Ok(true) => 1,
        Err(err) => {
            eprintln!("Error: {err}");
            2
        }
    };

    std::process::exit(exit_code);
}

#[cfg(test)]
const TOP_LEVEL: &str = concat!(env!("CARGO_MANIFEST_DIR"));

#[test]
fn test_ignore_target() {
    let mut skip_patterns = Vec::new();
    let mut exact_skip_paths = Vec::new();
    resolve_skip_paths(
        &PathBuf::from("."),
        ResolveSkipPaths {
            skip_target_dir: true,
            skip_patterns: &mut skip_patterns,
            skip: &mut exact_skip_paths,
        },
    )
    .unwrap();

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_patterns,
            exact_skip_paths,
            respect_ignore_files: false,
            override_respect_git_ignore: Some(false),
        },
    );
    assert!(entries.is_err());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_patterns: Vec::new(),
            exact_skip_paths: Vec::new(),
            respect_ignore_files: true,
            override_respect_git_ignore: Some(false),
        },
    );
    assert!(entries.is_err());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_patterns: Vec::new(),
            exact_skip_paths: Vec::new(),
            respect_ignore_files: false,
            override_respect_git_ignore: Some(false),
        },
    );
    assert!(!entries.unwrap().is_empty());
}

#[test]
fn test_no_cargo_toml_error() {
    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/empty-dir/"),
        CollectPathOptions {
            skip_patterns: Vec::new(),
            exact_skip_paths: Vec::new(),
            respect_ignore_files: false,
            override_respect_git_ignore: Some(false),
        },
    );
    let err = entries.unwrap_err();
    assert!(err.to_string().contains("found no Cargo.toml"));
}

#[test]
fn test_skip_exact_paths() {
    let fixture = PathBuf::from(TOP_LEVEL).join("integration-tests/skip-sub-dir");

    // Collects manifests under `root`, with `--skip` value.
    let collect = |root: &Path, cwd: &Path, skip_paths_str: &[&str]| {
        let mut skip = skip_paths_str.into_iter().map(PathBuf::from).collect();

        resolve_skip_paths(
            cwd,
            ResolveSkipPaths {
                skip_target_dir: false,
                skip_patterns: &mut Vec::new(),
                skip: &mut skip,
            },
        )
        .unwrap();

        collect_paths(
            root,
            CollectPathOptions {
                skip_patterns: Vec::new(),
                exact_skip_paths: skip,
                respect_ignore_files: false,
                override_respect_git_ignore: Some(false),
            },
        )
    };
    let has_skipped =
        |entries: &[PathBuf]| entries.iter().any(|p| p.ends_with("skipped/Cargo.toml"));

    // Without any skip patterns, both the top-level and the nested `skipped` manifest are collected.
    {
        let entries = collect(&fixture, &fixture, &[]).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(has_skipped(&entries));
    }

    // Exact paths are correctly matched.
    {
        let entries = collect(&fixture, &fixture, &["./skipped"]).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!has_skipped(&entries));
    }

    // `--skip .` skips the current directory, including children, so this leads to no collected
    // Cargo.toml, hence an error.
    {
        let error = collect(&fixture, &fixture, &["."]).unwrap_err();
        assert!(error.to_string().contains("found no Cargo.toml"));
    }

    // `--skip .` matches all directories when run from a source sub-dir; `.` is resolved to the
    // current working directory.
    {
        let cwd = fixture.join("src");
        let entries = collect(&fixture, &cwd, &["."]).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(has_skipped(&entries));
    }

    // `--skip .` matches some directories when run from an actual Rust package; `.` is resolved to
    // the current working directory.
    {
        let cwd = fixture.join("skipped");
        let entries = collect(&fixture, &cwd, &["."]).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!has_skipped(&entries));
    }

    // Skipped paths are resolved against the current working directory, not the root.
    // This is running from `skip-sub-dir/src`, `cargo machete --skip ../skipped ..`.
    {
        let cwd = fixture.join("src");
        let entries = collect(&cwd.join(".."), &cwd, &["../skipped"]).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!has_skipped(&entries));
    }

    // Walking a directory that lies under a skipped one skips it entirely.
    // This is running from `skip-sub-dir/skipped`, `cargo machete --skip .. .`.
    {
        let cwd = fixture.join("skipped");
        let error = collect(&cwd, &cwd, &[".."]).unwrap_err();
        assert!(error.to_string().contains("found no Cargo.toml"));
    }

    // Same, with a walk root nested deeper than the skipped directory, and given in a
    // non-canonical form: running from `skip-sub-dir`, `cargo machete --skip . skipped/src/..`.
    {
        let root = fixture.join("skipped/src/..");
        let error = collect(&root, &fixture, &["."]).unwrap_err();
        assert!(error.to_string().contains("found no Cargo.toml"));
    }
}

#[test]
fn test_skip_patterns() {
    let fixture = PathBuf::from(TOP_LEVEL).join("integration-tests/skip-sub-dir");

    // Collects manifests under `root`, with `--skip-patterns` value.
    let collect = |root: &Path, skip_patterns_str: &[&str]| -> Vec<PathBuf> {
        let mut skip_patterns = skip_patterns_str.into_iter().map(PathBuf::from).collect();

        resolve_skip_paths(
            root,
            ResolveSkipPaths {
                skip_target_dir: false,
                skip_patterns: &mut skip_patterns,
                skip: &mut Vec::new(),
            },
        )
        .unwrap();

        collect_paths(
            root,
            CollectPathOptions {
                skip_patterns,
                exact_skip_paths: Vec::new(),
                respect_ignore_files: false,
                override_respect_git_ignore: Some(false),
            },
        )
        .unwrap()
    };
    let has_skipped =
        |entries: &[PathBuf]| entries.iter().any(|p| p.ends_with("skipped/Cargo.toml"));

    // Without any skip patterns, both the top-level and the nested `skipped` manifest are collected.
    {
        let entries = collect(&fixture, &[]);
        assert_eq!(entries.len(), 2);
        assert!(has_skipped(&entries));
    }

    // Plain relative paths match on trailing components.
    for skip in ["skipped", "skipped/", "skip-sub-dir/skipped"] {
        let entries = collect(&fixture, &[skip]);
        assert_eq!(entries.len(), 1, "--skip-patterns {skip}");
        assert!(!has_skipped(&entries), "--skip-patterns {skip}");
    }
}

#[test]
fn test_invalid_skip_patterns() {
    let run = |skip_patterns: &[&str]| {
        let mut skip_patterns = skip_patterns.into_iter().map(PathBuf::from).collect();
        let cwd = PathBuf::from("unused");
        resolve_skip_paths(
            &cwd,
            ResolveSkipPaths {
                skip_target_dir: false,
                skip_patterns: &mut skip_patterns,
                skip: &mut Vec::new(),
            },
        )
    };

    let err = run(&[""]).unwrap_err();
    assert!(err.to_string().contains("empty"));

    for path in &["./current", "../parent", "/tmp"] {
        let err = run(&[path]).unwrap_err();
        assert!(err.to_string().contains("absolute or relative directory"));
    }
}

#[test]
fn test_remove_dependencies() {
    let manifest = PathBuf::from(TOP_LEVEL).join("./integration-tests/multi-key-dep/Cargo.toml");
    let stripped_manifest = remove_dependencies(
        &std::fs::read_to_string(manifest).unwrap(),
        &["cc".to_string(), "log-once".to_string(), "rand".to_string()],
    )
    .unwrap();
    assert_eq!(
        stripped_manifest,
        r#"[package]
name = "multi-key-dep"
version = "0.1.0"
edition = "2021"

[dependencies]
log = "0.4.14"

[target.'cfg(unix)'.dependencies]

[dev-dependencies]

[build-dependencies]
"#
    );
}
