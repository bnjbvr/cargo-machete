mod search_unused;

use crate::search_unused::find_unused;
use anyhow::Context;
use clap::Parser;
use rayon::prelude::*;
use std::path::Path;
use std::str::FromStr;
use std::{fs, path::PathBuf};
use walkdir::WalkDir;

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

impl From<UseCargoMetadata> for bool {
    fn from(v: UseCargoMetadata) -> bool {
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

#[derive(Parser)]
/// cargo-machete: Helps find unused dependencies in a fast yet imprecise way.
///
/// Exit Code:
///
///     0:  when no unused dependencies are found
///
///     1:  when at least one unused (non-ignored) dependency is found
///
///     2:  on error
struct MacheteArgs {
    /// Paths to the directories that should be scanned
    paths: Vec<PathBuf>,

    /// uses cargo-metadata to figure out the dependencies' names.
    ///
    /// May be useful if some dependencies are renamed from their own Cargo.toml
    /// file
    ///
    /// (e.g. xml-rs which gets renamed to xml).
    ///
    /// Try it if you get false positives!
    #[arg(short = 'w', long)]
    with_metadata: bool,

    /// don't analyze anything contained in any target/ directories encountered.
    #[arg(short = 's', long)]
    skip_target_dir: bool,

    /// rewrite the Cargo.toml files to automatically remove unused dependencies.
    ///
    /// Note: all dependencies flagged by cargo-machete will be removed, including false positives.
    #[arg(short = 'f', long)]
    fix: bool,

    /// Print the current version.
    #[arg(short = 'v', long)]
    version: bool,
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
        .map(|res_entry| res_entry.map(|e| e.into_path()))
        .collect()
}

/// Runs `cargo-machete`.
/// Returns Ok with a bool whether any unused dependencies were found, or Err on errors.
fn run_machete() -> anyhow::Result<bool> {
    let mut machete = MacheteArgs::parse();
    pretty_env_logger::init();

    if machete.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    if machete.paths.is_empty() {
        eprintln!("Analyzing dependencies of crates in this directory...");
        machete.paths.push(std::env::current_dir()?);
    } else {
        eprintln!(
            "Analyzing dependencies of crates  in {}...",
            machete
                .paths
                .iter()
                .cloned()
                .map(|path| path.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
    }

    let mut has_unused_dependencies = false;
    let mut walkdir_errors = Vec::new();

    for path in machete.paths {
        let manifest_path_entries = match collect_paths(&path, machete.skip_target_dir) {
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
                match find_unused(manifest_path, machete.with_metadata.into()) {
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
                        eprintln!("Error when handling {}: {}", manifest_path.display(), err);
                        None
                    }
                }
            })
            .collect::<Vec<_>>();

        // Display all the results.
        if results.is_empty() {
            println!(
                "Machete didn't find any unused dependencies in {}.\nGood job :D",
                path.to_string_lossy()
            );
            continue;
        }

        println!(
            "Machete found the following unused dependencies in {}:",
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

            if machete.fix {
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

        if !machete.with_metadata {
            println!(
                "\n\
                You can also try running it with the `--with-metadata` flag for better accuracy,\n\
                though this may modify your Cargo.lock files."
            );
        }

        println!()
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
    let mut manifest = toml_edit::Document::from_str(manifest)?;
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
