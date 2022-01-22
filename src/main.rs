mod search_unused;

use crate::search_unused::find_unused;
use rayon::prelude::*;
use std::fs;
use walkdir::WalkDir;

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let mut fix = false;

    let args = std::env::args();
    for arg in args {
        if arg == "--fix" || arg == "fix" {
            fix = true;
        }
    }

    eprintln!("Looking for crates in this directory and analyzing their dependencies...");

    let cwd = std::env::current_dir()?;

    // Find directory entries.
    let entries = WalkDir::new(cwd)
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
        .filter_map(|path| match find_unused(path) {
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

    eprintln!("Done!");

    Ok(())
}
