mod printers;
mod search_unused;

use crate::printers::json::JsonPrinter;
use crate::printers::stdout::StdoutPrinter;
use crate::printers::{AnalyzedPaths, Printer};
use crate::search_unused::find_unused;
use anyhow::{Context, bail};
use rayon::prelude::*;
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

    /// don't analyze anything contained in any target/ directories encountered.
    #[argh(switch)]
    skip_target_dir: bool,

    /// don't analyze anything under directories matching this path. Relative paths match on
    /// trailing path components, like --skip-target-dir does for target/, so `--skip tests` skips
    /// every `tests` directory found. Absolute paths and paths containing `..` are resolved
    /// against the current directory and skip exactly that directory. May be passed multiple
    /// times.
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

struct CollectPathOptions {
    /// Should we avoid scanning `target` directories?
    skip_target_dir: bool,

    /// Directories to skip, see [`SkipPath`].
    skip_paths: Vec<SkipPath>,

    /// Should we ignore files as specified in .gitignore (in the target directory, or any parent),
    /// and `.ignore`?
    respect_ignore_files: bool,

    // As an override to the above `respect_ignore_files`, should we use `.gitignore` overall?
    //
    // This is used only in testing, to avoid reading this repository's `.gitignore` file for
    // testing the `collect_path()` function.
    override_respect_git_ignore: Option<bool>,
}

fn collect_paths(path: &Path, options: CollectPathOptions) -> anyhow::Result<Vec<PathBuf>> {
    // Find directory entries.
    let mut builder = ignore::WalkBuilder::new(path);

    builder.standard_filters(options.respect_ignore_files);

    if let Some(val) = options.override_respect_git_ignore {
        builder.git_ignore(val);
    }

    let skip_paths = options.skip_paths;
    let skip_target_dir = options.skip_target_dir;

    // Anchored skip paths are absolute and canonical, so entries must be compared through the
    // canonical form of the walk root. Entries are always `path.join(relative)`, so we only
    // canonicalize the root once instead of every entry.
    let canonical_root = if skip_paths
        .iter()
        .any(|skip| matches!(skip, SkipPath::Anchored(_)))
    {
        Some(
            fs::canonicalize(path).map_err(|err| ignore::Error::WithPath {
                path: path.to_path_buf(),
                err: Box::new(ignore::Error::Io(err)),
            })?,
        )
    } else {
        None
    };

    if skip_target_dir || !skip_paths.is_empty() {
        let root = path.to_path_buf();
        builder.filter_entry(move |entry| {
            let entry_path = entry.path();
            if skip_target_dir && entry_path.ends_with("target") {
                return false;
            }
            !skip_paths.iter().any(|skip| match skip {
                SkipPath::Suffix(suffix) => entry_path.ends_with(suffix),
                SkipPath::Anchored(abs) => canonical_root.as_ref().is_some_and(|canonical_root| {
                    let rel = entry_path.strip_prefix(&root).unwrap_or(entry_path);
                    canonical_root.join(rel) == *abs
                }),
            })
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

/// A directory to prune from the walk, as given with `--skip`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SkipPath {
    /// A plain relative path like `tests` or `crates/foo`: skips every directory whose path ends
    /// with these components, wherever it appears under the analyzed paths.
    Suffix(PathBuf),
    /// An absolute path, or a relative one containing `..`: resolved against the current
    /// directory, and skips exactly that directory.
    Anchored(PathBuf),
}

impl SkipPath {
    /// Interprets a user-provided `--skip` value, resolving anchored paths against `base`.
    ///
    /// Returns `Ok(None)` for paths that would match everything (e.g. `.`), and an error if an
    /// anchored path doesn't exist.
    fn resolve(raw: &Path, base: &Path) -> anyhow::Result<Option<Self>> {
        let anchored =
            raw.is_absolute() || raw.components().any(|c| matches!(c, Component::ParentDir));

        if anchored {
            // Note: if raw is absolute, `base.join(raw)` is equivalent to `raw`, so this works for
            // both absolute and relative paths containing `..`.
            let abs = fs::canonicalize(base.join(raw))
                .with_context(|| format!("invalid --skip path {}", raw.display()))?;
            return Ok(Some(SkipPath::Anchored(abs)));
        }

        // Drop `.` components so `--skip ./tests` behaves like `--skip tests`, and `--skip .`
        // doesn't end up matching every entry.
        let suffix: PathBuf = raw
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        if suffix.as_os_str().is_empty() {
            return Ok(None);
        }
        Ok(Some(SkipPath::Suffix(suffix)))
    }
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

    let skip_paths = {
        let cwd = std::env::current_dir().context("couldn't read the current directory")?;
        let mut skip_paths = Vec::with_capacity(args.skip.len());
        for raw in &args.skip {
            skip_paths.extend(SkipPath::resolve(raw, &cwd)?);
        }
        skip_paths
    };

    let mut has_unused_dependencies = false;

    // Be resilient in case we can't collect some paths due to errors: don't abort execution, so we
    // can run analysis for paths which didn't cause errors.
    let mut path_collection_errors: Vec<anyhow::Error> = Vec::new();

    for path in args.paths {
        let manifest_path_entries = match collect_paths(
            &path,
            CollectPathOptions {
                skip_target_dir: args.skip_target_dir,
                skip_paths: skip_paths.clone(),
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
    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_target_dir: true,
            skip_paths: Vec::new(),
            respect_ignore_files: false,
            override_respect_git_ignore: Some(false),
        },
    );
    assert!(entries.is_err());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_target_dir: false,
            skip_paths: Vec::new(),
            respect_ignore_files: true,
            override_respect_git_ignore: Some(false),
        },
    );
    assert!(entries.is_err());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_target_dir: false,
            skip_paths: Vec::new(),
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
            skip_target_dir: false,
            skip_paths: Vec::new(),
            respect_ignore_files: false,
            override_respect_git_ignore: Some(false),
        },
    );
    let err = entries.unwrap_err();
    assert!(err.to_string().contains("found no Cargo.toml"));
}

#[test]
fn test_skip_paths() {
    let fixture = PathBuf::from(TOP_LEVEL).join("integration-tests/skip-sub-dir");

    // Collects manifests under `root`, with `--skip` values resolved as if run from `cwd`.
    let collect = |root: &Path, cwd: &Path, skips: &[&str]| -> Vec<PathBuf> {
        let skip_paths = skips
            .iter()
            .filter_map(|raw| SkipPath::resolve(Path::new(raw), cwd).unwrap())
            .collect();
        collect_paths(
            root,
            CollectPathOptions {
                skip_target_dir: false,
                skip_paths,
                respect_ignore_files: false,
                override_respect_git_ignore: Some(false),
            },
        )
        .unwrap()
    };
    let has_skipped =
        |entries: &[PathBuf]| entries.iter().any(|p| p.ends_with("skipped/Cargo.toml"));

    // Without any skip paths, both the top-level and the nested `skipped` manifest are collected.
    {
        let entries = collect(&fixture, &fixture, &[]);
        assert_eq!(entries.len(), 2);
        assert!(has_skipped(&entries));
    }

    // Plain relative paths match on trailing components; `./` and `.` components are ignored.
    for skip in ["skipped", "./skipped", "skipped/", "skip-sub-dir/skipped"] {
        let entries = collect(&fixture, &fixture, &[skip]);
        assert_eq!(entries.len(), 1, "--skip {skip}");
        assert!(!has_skipped(&entries), "--skip {skip}");
    }

    // `--skip .` doesn't match everything.
    {
        let entries = collect(&fixture, &fixture, &["."]);
        assert_eq!(entries.len(), 2);
        assert!(has_skipped(&entries));
    }

    // Absolute paths skip exactly that directory.
    {
        let abs = fixture.join("skipped");
        let entries = collect(&fixture, &fixture, &[abs.to_str().unwrap()]);
        assert_eq!(entries, vec![fixture.join("Cargo.toml")]);
    }

    // Paths with `..` are resolved against the current directory, also when the walk root is
    // itself relative: from `skip-sub-dir/src`, `cargo machete --skip ../skipped ..`.
    {
        let cwd = fixture.join("src");
        let entries = collect(&cwd.join(".."), &cwd, &["../skipped"]);
        assert_eq!(entries.len(), 1);
        assert!(!has_skipped(&entries));
    }

    // `..` paths are resolved to one exact directory instead of being matched as a suffix, so
    // scanning `../..` with `--skip ../tests` can't prune an unrelated `../../tests`.
    assert_eq!(
        SkipPath::resolve(Path::new("../skipped"), &fixture.join("src")).unwrap(),
        Some(SkipPath::Anchored(
            fs::canonicalize(fixture.join("skipped")).unwrap()
        ))
    );

    // Anchored paths that don't exist are reported instead of being silently ignored.
    assert!(SkipPath::resolve(Path::new("../does-not-exist"), &fixture).is_err());
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
