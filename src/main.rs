use log::{debug, info};
use std::collections::{HashMap, HashSet};
use std::fs;
use walkdir::WalkDir;

use crate::search_unused::find_unused;

mod search_unused;

#[derive(serde::Deserialize)]
struct CargoUdepsPackage {
    normal: Vec<String>,
}

#[derive(serde::Deserialize)]
struct CargoUdepsOutput {
    success: bool,
    unused_deps: HashMap<String, CargoUdepsPackage>,
}

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

fn compare_with_cargo_udeps(our_analysis: &mut PackageAnalysis) -> anyhow::Result<()> {
    debug!("checking with cargo-udeps...");

    // Run cargo-udeps!
    let mut cmd = std::process::Command::new("cargo");
    cmd.args([
        "+nightly",
        "udeps",
        "--all-features",
        "--output",
        "json",
        "-p",
        &our_analysis.package_name,
    ]);
    let output = cmd.output()?;
    let output_str = String::from_utf8(output.stdout)?;
    let analysis: CargoUdepsOutput = serde_json::from_str(&output_str)?;

    if analysis.success {
        debug!("cargo-udeps didn't find any unused dependency");
        return Ok(());
    }

    let mut udeps_set = None;
    for (k, v) in analysis.unused_deps {
        if !k.starts_with(&our_analysis.package_name) {
            continue;
        }
        udeps_set = Some(v.normal.into_iter().collect::<HashSet<_>>());
    }

    if let Some(udeps_set) = udeps_set {
        let our_set: HashSet<String> = our_analysis.unused.iter().cloned().collect::<HashSet<_>>();
        let inter_set = our_set.intersection(&udeps_set);
        our_analysis.unused = inter_set.into_iter().cloned().collect();
    }

    return Ok(());
}

fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();

    println!("Analyzing crates and their dependencies...");

    let mut fix = false;
    let mut no_false_positives = false;
    let args = std::env::args();
    for arg in args {
        if arg == "--fix" || arg == "fix" {
            fix = true;
        }
        if arg == "--check" || arg == "check" {
            no_false_positives = true;
        }
    }

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

                    if no_false_positives {
                        compare_with_cargo_udeps(&mut analysis)?;
                    }

                    println!("{} -- found unused dependencies:", analysis.package_name);
                    for dep in &analysis.unused {
                        println!("\t{}", dep)
                    }

                    if fix {
                        info!("rewriting Cargo.toml");
                        for dep in analysis.unused {
                            analysis.manifest.dependencies.remove(&dep);
                            let serialized = toml::to_string(&analysis.manifest)?;
                            fs::write(path.clone(), serialized)?;
                        }
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
