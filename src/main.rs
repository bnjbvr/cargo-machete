mod search_unused;

use crate::search_unused::find_unused;
use std::fs;
use walkdir::WalkDir;

struct PackageAnalysis {
    manifest: cargo_toml::Manifest,
    package_name: String,
    unused: Vec<String>,
}

impl PackageAnalysis {
    fn new(name: String, manifest: cargo_toml::Manifest) -> Self {
        Self {
            manifest,
            package_name: name,
            unused: Default::default(),
        }
    }
}

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
    for entry in WalkDir::new(cwd) {
        let entry = entry?;
        if entry.file_name() == "Cargo.toml" {
            let path = entry.into_path();
            match find_unused(&path) {
                Ok(Some(mut analysis)) => {
                    if analysis.unused.is_empty() {
                        continue;
                    }

                    println!("{} -- {}:", analysis.package_name, path.to_string_lossy());
                    for dep in &analysis.unused {
                        println!("\t{}", dep)
                    }

                    if fix {
                        for dep in analysis.unused {
                            analysis.manifest.dependencies.remove(&dep);
                        }
                        let serialized = toml::to_string(&analysis.manifest)?;
                        fs::write(path.clone(), serialized)?;
                    }
                }

                Ok(None) => {
                    println!(
                        "{} -- no package, must be a workspace",
                        path.to_string_lossy()
                    );
                }

                Err(err) => {
                    eprintln!("error when handling {}: {}", path.display(), err);
                }
            }
        }
    }

    eprintln!("Done!");

    Ok(())
}
