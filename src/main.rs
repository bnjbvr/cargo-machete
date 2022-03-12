mod search_unused;

use crate::search_unused::{find_unused, UseCargoMetadata};
use rayon::prelude::*;
use std::{fs, path::PathBuf};
use walkdir::WalkDir;

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let mut fix = false;
    let mut use_cargo_metadata = UseCargoMetadata::No;

    let mut path_str = Vec::new();
    let args = std::env::args();

    for (i, arg) in args.into_iter().enumerate() {
        // Ignore the binary name, and the "machete" command if ran as cargo subcommand.
        if i == 0 {
            continue;
        }
        if i == 1 && arg == "machete" {
            continue;
        }

        if arg == "--fix" || arg == "fix" {
            fix = true;
        } else if arg == "--with-metadata" {
            use_cargo_metadata = UseCargoMetadata::Yes;
        } else {
            path_str.push(arg);
        }
    }

    let paths = if path_str.is_empty() {
        eprintln!("Analyzing dependencies of crates in this directory...");
        vec![std::env::current_dir()?]
    } else {
        eprintln!(
            "Analyzing dependencies of crates in {}...",
            path_str.join(",")
        );
        path_str.into_iter().map(PathBuf::from).collect()
    };

    for path in paths {
        // Find directory entries.
        let entries = WalkDir::new(path)
            .into_iter()
            .filter_map(|entry| match entry {
                Ok(entry) if entry.file_name() == "Cargo.toml" => Some(entry.into_path()),
                Err(err) => {
                    eprintln!("error when walking over subdirectories: {}", err);
                    None
                }
                _ => None,
            })
            .collect::<Vec<_>>();

        // Run analysis in parallel. This will spawn new rayon tasks when dependencies are effectively
        // used by any Rust crate.
        let results = entries
            .par_iter()
            .filter_map(|path| match find_unused(path, use_cargo_metadata) {
                Ok(Some(analysis)) => {
                    if analysis.unused.is_empty() {
                        None
                    } else {
                        Some((analysis, path))
                    }
                }

                Ok(None) => {
                    log::info!(
                        "{} is a virtual manifest for a workspace",
                        path.to_string_lossy()
                    );
                    None
                }

                Err(err) => {
                    eprintln!("error when handling {}: {}", path.display(), err);
                    None
                }
            })
            .collect::<Vec<_>>();

        // Display all the results.
        for (mut analysis, path) in results {
            println!("{} -- {}:", analysis.package_name, path.to_string_lossy());
            for dep in &analysis.unused {
                println!("\t{}", dep)
            }

            if fix {
                for dep in analysis.unused {
                    analysis.manifest.dependencies.remove(&dep);
                }
                let serialized = toml::to_string(&analysis.manifest)
                    .expect("error when converting updated manifest to toml");
                fs::write(&path, serialized).expect("Cargo.toml write error");
            }
        }
    }

    eprintln!("Done!");

    Ok(())
}
