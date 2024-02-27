mod search_unused;

use crate::search_unused::find_unused;
use anyhow::{anyhow, bail, Context};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::{fs, path::PathBuf};
use toml_edit::{KeyMut, TableLike};
use walkdir::WalkDir;

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

impl From<UseCargoMetadata> for bool {
    fn from(v: UseCargoMetadata) -> Self {
        matches!(v, UseCargoMetadata::Yes)
    }
}

impl From<bool> for UseCargoMetadata {
    fn from(b: bool) -> Self {
        if b {
            Self::Yes
        } else {
            Self::No
        }
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

    /// rewrite the Cargo.toml files to automatically remove unused dependencies.
    /// Note: all dependencies flagged by cargo-machete will be removed, including false positives.
    #[argh(switch)]
    fix: bool,

    /// print version.
    #[argh(switch)]
    version: bool,

    /// paths to directories that must be scanned.
    #[argh(positional, greedy)]
    paths: Vec<PathBuf>,
}

fn collect_paths(path: &Path, skip_target_dir: bool) -> Result<Vec<PathBuf>, walkdir::Error> {
    // Find directory entries.
    let walker = WalkDir::new(path).into_iter();

    let manifest_path_entries = if skip_target_dir {
        walker
            .filter_entry(|entry| !entry.path().ends_with("target"))
            .collect()
    } else {
        walker.collect::<Vec<_>>()
    };

    // Keep only errors and `Cargo.toml` files (filter), then map correct paths into owned
    // `PathBuf`.
    manifest_path_entries
        .into_iter()
        .filter(|entry| match entry {
            Ok(entry) => entry.file_name() == "Cargo.toml",
            Err(_) => true,
        })
        .map(|res_entry| res_entry.map(walkdir::DirEntry::into_path))
        .collect()
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

    if args.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    if args.paths.is_empty() {
        eprintln!("Analyzing dependencies of crates in this directory...");
        args.paths.push(PathBuf::from("."));
    } else {
        eprintln!(
            "Analyzing dependencies of crates in {}...",
            args.paths
                .iter()
                .cloned()
                .map(|path| path.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
    }

    let mut has_unused_dependencies = false;
    let mut walkdir_errors = Vec::new();

    for path in args.paths {
        let manifest_path_entries = match collect_paths(&path, args.skip_target_dir) {
            Ok(entries) => entries,
            Err(err) => {
                walkdir_errors.push(err);
                continue;
            }
        };

        // Run analysis in parallel. This will spawn new rayon tasks when dependencies are effectively
        // used by any Rust crate.
        let results = manifest_path_entries
            .par_iter()
            .filter_map(|manifest_path| {
                match find_unused(manifest_path, args.with_metadata.into()) {
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
                        eprintln!("error when handling {}: {}", manifest_path.display(), err);
                        None
                    }
                }
            })
            .collect::<Vec<_>>();

        // Display all the results.
        if results.is_empty() {
            println!(
                "cargo-machete didn't find any unused dependencies in {}. Good job!",
                path.to_string_lossy()
            );
            continue;
        }

        println!(
            "cargo-machete found the following unused dependencies in {}:",
            path.to_string_lossy()
        );
        for (analysis, path) in results {
            println!("{} -- {}:", analysis.package_name, path.to_string_lossy());
            for dep in &analysis.unused {
                println!("\t{dep}");
                has_unused_dependencies = true; // any unused dependency is enough to set flag to true
            }

            for dep in &analysis.ignored_used {
                eprintln!("\t⚠️  {dep} was marked as ignored, but is actually used!");
            }

            if args.fix {
                let fixed = remove_dependencies(&fs::read_to_string(path)?, &analysis.unused)?;
                fs::write(path, fixed).expect("Cargo.toml write error");
            }
        }
    }

    if has_unused_dependencies {
        println!(
            "\n\
            If you believe cargo-machete has detected an unused dependency incorrectly,\n\
            you can add the dependency to the list of dependencies to ignore in the\n\
            `[package.metadata.cargo-machete]` section of the appropriate Cargo.toml.\n\
            For example:\n\
            \n\
            [package.metadata.cargo-machete]\n\
            ignored = [\"prost\"]"
        );

        if !args.with_metadata {
            println!(
                "\n\
                You can also try running it with the `--with-metadata` flag for better accuracy,\n\
                though this may modify your Cargo.lock files."
            );
        }

        println!();
    }

    eprintln!("Done!");

    if !walkdir_errors.is_empty() {
        anyhow::bail!(
            "Errors when walking over directories:\n{}",
            walkdir_errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    Ok(has_unused_dependencies)
}

// handle a superset of all dependency name dashed/underscored variants: re'\w[-_]'
fn dep_name_superset(dep_names: &[String]) -> HashSet<String> {
    let mut unused: HashSet<String> = dep_names.iter().cloned().collect();
    for dep in unused.clone() {
        unused.insert(dep.replace('-', "_"));
        unused.insert(dep.replace('_', "-"));
    }
    unused
}

fn remove_dependencies(manifest: &str, dependency_list: &[String]) -> anyhow::Result<String> {
    let mut manifest = toml_edit::Document::from_str(manifest)?;
    let missing_table_msg = "unexpected missing table, please report with a test case on https://github.com/bnjbvr/cargo-machete";
    let valid_tables = HashSet::from(["dependencies", "build-dependencies", "dev-dependencies"]);
    let dependency_superset = dep_name_superset(dependency_list);

    let mut dep_tables = manifest
        .iter_mut()
        // select for items that are tables with valid names
        .filter(|(k, v)| v.is_table_like() && valid_tables.contains(k.display_repr().as_ref()))
        .map(|(k, v)| {
            let table = v.as_table_like_mut().context(missing_table_msg)?;
            Ok((k, table))
        })
        .filter(|v| match v {
            Ok((_, table)) if dependency_superset.iter().any(|k| table.contains_key(k)) => true,
            Err(_) => true,
            Ok(_) => false,
        })
        .collect::<Result<Vec<(KeyMut, &mut dyn TableLike)>, anyhow::Error>>()?;

    for dep in dependency_list {
        // for now
        let mut removed_one = false;
        for (name, table) in &mut dep_tables {
            if table
                .remove(dep)
                .or_else(|| table.remove(dep.replace('_', "-").as_str()))
                .is_some()
            {
                removed_one = true;
                log::debug!("removed {name}.{dep}");
            } else {
                log::trace!("no match for {name}.{dep}");
            }
        }
        if !removed_one {
            let tables = dep_tables
                .iter()
                .map(|(k, _)| format!("{k}"))
                .collect::<Vec<String>>()
                .join(", ");
            bail!(anyhow!("{dep} not found").context(format!("tables: {tables}")));
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
            eprintln!("Error: {err:?}");
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
        true,
    );
    assert!(entries.unwrap().is_empty());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        false,
    );
    assert!(!entries.unwrap().is_empty());
}
