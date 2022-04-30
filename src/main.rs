mod search_unused;

use crate::search_unused::{find_unused, UseCargoMetadata};
use rayon::prelude::*;
use std::{fs, path::PathBuf};
use walkdir::WalkDir;

struct MacheteArgs {
    fix: bool,
    use_cargo_metadata: UseCargoMetadata,
    paths: Vec<PathBuf>,
}

const HELP: &str = r#"cargo-machete: Helps find unused dependencies in a fast yet imprecise way.

Example usage: cargo-machete [PATH1] [PATH2] [--flags]?

Flags:

    --help / -h: displays this help message.

    --with-metadata: uses cargo-metadata to figure out the dependencies' names. May be useful if
                     some dependencies are renamed from their own Cargo.toml file (e.g. xml-rs
                     which gets renamed xml). Try it if you get false positives!

    --fix: (beta) rewrite the Cargo.toml files to automatically remove unused dependencies.
           Warning: this will likely entirely rewrite every dependency specification as a separate
           block, and include more Cargo.toml fields in a way that's usually not desirable.

Exit code:

    0:  when no unused dependencies are found
    1:  when at least one unused (non-ignored) dependency is found
    2:  on error
"#;

fn parse_args() -> anyhow::Result<MacheteArgs> {
    let mut fix = false;
    let mut use_cargo_metadata = UseCargoMetadata::No;

    let mut path_str = Vec::new();
    let args = std::env::args();

    for (i, arg) in args.into_iter().enumerate() {
        // Ignore the binary name...
        if i == 0 {
            continue;
        }
        // ...and the "machete" command if ran as cargo subcommand.
        if i == 1 && arg == "machete" {
            continue;
        }

        if arg == "help" || arg == "-h" || arg == "--help" {
            eprintln!("{}", HELP);
            std::process::exit(0);
        }

        if arg == "--fix" {
            fix = true;
        } else if arg == "--with-metadata" {
            use_cargo_metadata = UseCargoMetadata::Yes;
        } else if arg.starts_with('-') {
            anyhow::bail!("invalid parameter {arg}. Usage:\n{HELP}");
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

    Ok(MacheteArgs {
        fix,
        use_cargo_metadata,
        paths,
    })
}

/// Runs `cargo-machete`.
/// Returns Ok with a bool whether any unused dependencies were found, or Err on errors.
fn run_machete() -> anyhow::Result<bool> {
    pretty_env_logger::init();

    let mut has_unused_dependencies = false;
    let args = parse_args()?;

    for path in args.paths {
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
            .filter_map(|path| match find_unused(path, args.use_cargo_metadata) {
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
                println!("\t{}", dep);
                has_unused_dependencies = true; // any unused dependency is enough to set flag to true
            }

            if args.fix {
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

    Ok(has_unused_dependencies)
}

fn main() {
    let exit_code = match run_machete() {
        Ok(false) => 0,
        Ok(true) => 1,
        Err(_) => 2,
    };

    std::process::exit(exit_code);
}
