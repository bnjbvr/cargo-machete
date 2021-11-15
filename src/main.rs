mod search_unused;

#[cfg(feature = "cargo_udeps")]
mod cargo_udeps;

use crate::search_unused::find_unused;
use log::info;
use std::fs;
use walkdir::WalkDir;

struct PackageAnalysis {
    manifest: cargo_toml::Manifest,
    package_name: String,
    unused: Vec<String>,
    errors: Vec<anyhow::Error>,
}

impl PackageAnalysis {
    fn new(name: String, manifest: cargo_toml::Manifest) -> Self {
        Self {
            manifest,
            package_name: name,
            unused: Default::default(),
            errors: Default::default(),
        }
    }
}

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    let mut fix = false;

    #[cfg(feature = "cargo_udeps")]
    let mut check_udeps = false;

    let args = std::env::args();
    for arg in args {
        if arg == "--fix" || arg == "fix" {
            fix = true;
        }

        if arg == "--check" || arg == "check" {
            #[cfg(feature = "cargo_udeps")]
            {
                check_udeps = true;
            }

            #[cfg(not(feature = "cargo_udeps"))]
            {
                eprintln!("--check only works if compiling with cargo_udeps feature");
                std::process::exit(-1);
            }
        }
    }

    println!("Analyzing crates and their dependencies...");

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

                    #[cfg(feature = "cargo_udeps")]
                    if check_udeps {
                        crate::cargo_udeps::compare(&mut analysis)?;
                    }

                    println!("{} -- {}:", analysis.package_name, path.to_string_lossy());

                    for dep in &analysis.unused {
                        println!("\t{}", dep)
                    }

                    if fix {
                        info!("rewriting Cargo.toml");
                        for dep in analysis.unused {
                            analysis.manifest.dependencies.remove(&dep);
                        }
                        let serialized = toml::to_string(&analysis.manifest)?;
                        fs::write(path.clone(), serialized)?;
                    }
                }

                Ok(None) => {
                    println!("{} -- didn't find any package", path.to_string_lossy());
                }

                Err(err) => {
                    eprintln!("error when handling {}: {}", path.display(), err);
                }
            }
        }
    }

    Ok(())
}
