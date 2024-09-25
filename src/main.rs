mod search_unused;

use crate::search_unused::find_unused;
use anyhow::Context;
use rayon::prelude::*;
use std::path::Path;
use std::str::FromStr;
use std::{borrow::Cow, fs, path::PathBuf};

#[derive(Clone, Copy)]
pub(crate) enum UseCargoMetadata {
    Yes,
    No,
}

#[cfg(test)]
impl UseCargoMetadata {
    fn all() -> &'static [UseCargoMetadata] {
        &[UseCargoMetadata::Yes, UseCargoMetadata::No]
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

    /// also search in ignored files (.gitignore, .ignore, etc.) when searching for files.
    #[argh(switch)]
    no_ignore: bool,

    /// print version.
    #[argh(switch)]
    version: bool,

    /// paths to directories that must be scanned.
    #[argh(positional, greedy)]
    paths: Vec<PathBuf>,
}

struct CollectPathOptions {
    skip_target_dir: bool,
    respect_ignore_files: bool,
}

fn collect_paths(path: &Path, options: CollectPathOptions) -> Result<Vec<PathBuf>, ignore::Error> {
    // Find directory entries.
    let mut builder = ignore::WalkBuilder::new(path);

    builder.standard_filters(options.respect_ignore_files);

    if options.skip_target_dir {
        builder.filter_entry(|entry| !entry.path().ends_with("target"));
    }

    let walker = builder.build();

    // Keep only errors and `Cargo.toml` files (filter), then map correct paths into owned
    // `PathBuf`.
    walker
        .into_iter()
        .filter(|entry| {
            entry
                .as_ref()
                .map_or(true, |entry| entry.file_name() == "Cargo.toml")
        })
        .map(|res_entry| res_entry.map(|e| e.into_path()))
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
                .map(|path| path.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
    }

    let mut has_unused_dependencies = false;
    let mut walkdir_errors = Vec::new();

    for path in args.paths {
        let manifest_path_entries = match collect_paths(
            &path,
            CollectPathOptions {
                skip_target_dir: args.skip_target_dir,
                respect_ignore_files: !args.no_ignore,
            },
        ) {
            Ok(entries) => entries,
            Err(err) => {
                walkdir_errors.push(err);
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
                        eprintln!("error when handling {}: {}", manifest_path.display(), err);
                        None
                    }
                },
            )
            .collect::<Vec<_>>();

        // Display all the results.
        let location = match path.to_string_lossy() {
            Cow::Borrowed(".") => Cow::from("this directory"),
            pathstr => pathstr,
        };

        if results.is_empty() {
            println!("cargo-machete didn't find any unused dependencies in {location}. Good job!");
            continue;
        }

        println!("cargo-machete found the following unused dependencies in {location}:");
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

fn remove_dependencies(manifest: &str, dependencies_list: &[String]) -> anyhow::Result<String> {
    let mut manifest = toml_edit::DocumentMut::from_str(manifest)?;
    let dependencies = manifest
        .iter_mut()
        .find_map(|(k, v)| (v.is_table_like() && k == "dependencies").then_some(Some(v)))
        .flatten()
        .context("dependencies table is missing or empty")?
        .as_table_mut()
        .context("unexpected missing table, please report with a test case on https://github.com/bnjbvr/cargo-machete")?;

    for k in dependencies_list {
        dependencies
            .remove(k)
            .with_context(|| format!("Dependency {k} not found"))?;
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
            respect_ignore_files: false,
        },
    );
    assert!(entries.unwrap().is_empty());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_target_dir: false,
            respect_ignore_files: true,
        },
    );
    assert!(entries.unwrap().is_empty());

    let entries = collect_paths(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
        CollectPathOptions {
            skip_target_dir: false,
            respect_ignore_files: false,
        },
    );
    assert!(!entries.unwrap().is_empty());
}
