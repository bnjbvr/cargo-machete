use std::collections::{HashMap, HashSet};
use std::{error, fs};
use std::{error::Error, path::PathBuf};

use grep::regex::RegexMatcher;
use grep::searcher::{BinaryDetection, SearcherBuilder};
use log::{debug, info, trace};
use walkdir::WalkDir;

#[derive(Debug)]
struct BoxedError {
    msg: String,
}

impl<T: Error> From<T> for BoxedError {
    fn from(err: T) -> Self {
        Self {
            msg: err.to_string(),
        }
    }
}

#[derive(serde::Deserialize)]
struct CargoUdepsPackage {
    normal: Vec<String>,
}

#[derive(serde::Deserialize)]
struct CargoUdepsOutput {
    success: bool,
    unused_deps: HashMap<String, CargoUdepsPackage>,
}

fn to_snake_case(name: &str) -> String {
    name.replace('-', "_")
}

fn handle_package(
    manifest_path: &PathBuf,
    fix: bool,
    no_false_positives: bool,
) -> Result<(), BoxedError> {
    let mut dir_path = manifest_path.clone();
    dir_path.pop();

    trace!("trying to open {}...", manifest_path.display());

    let mut manifest = cargo_toml::Manifest::from_path(manifest_path.clone())?;
    let package_name = match manifest.package {
        Some(ref package) => &package.name,
        None => return Ok(()),
    };

    debug!("handling {} ({})", package_name, dir_path.display());

    let mut to_remove = Vec::new();

    for (name, _) in manifest.dependencies.iter() {
        let snaked = to_snake_case(&name);
        // Look for:
        // use X:: / use X; / use X as / X:: / extern crate X;
        let pattern = format!(
            "use {snaked}(::|;| as)?|{snaked}::|extern crate {snaked}( |;)",
            snaked = snaked
        );

        trace!(
            "looking for {} in {}",
            pattern,
            manifest_path.to_string_lossy()
        );

        match search(dir_path.clone(), &pattern) {
            Ok(found) => {
                if !found {
                    debug!("{} might be unused", name);
                    to_remove.push(name.clone());
                }
            }
            Err(err) => {
                eprintln!("error: {}", err)
            }
        }
    }

    if to_remove.is_empty() {
        debug!("didn't find any unused dependency in quick search");
        return Ok(());
    }

    if no_false_positives {
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
            package_name,
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
            if !k.starts_with(&format!("{} ", package_name)) {
                continue;
            }
            udeps_set = Some(v.normal.into_iter().collect::<HashSet<_>>());
        }

        if let Some(udeps_set) = udeps_set {
            let our_set = to_remove.into_iter().collect::<HashSet<_>>();
            let inter_set = our_set.intersection(&udeps_set);
            if inter_set.clone().next().is_some() {
                println!("{}:", package_name);
                for entry in inter_set {
                    println!("  {}", entry);
                }
            }
        }

        return Ok(());
    }

    println!("{}:", package_name);
    for entry in to_remove {
        println!("  {}", entry);
        manifest.dependencies.remove(&entry);
    }

    if fix {
        info!("rewriting Cargo.toml");
        let serialized = toml::to_string(&manifest)?;
        fs::write(manifest_path, serialized)?;
    }

    return Ok(());
}

fn main() -> Result<(), BoxedError> {
    pretty_env_logger::init();

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
            if let Err(err) = handle_package(&path, fix, no_false_positives) {
                eprintln!("error when handling {}: {}", path.display(), err.msg);
            }
        }
    }

    Ok(())
}

struct StopAfterFirstMatch {
    found: bool,
}

impl StopAfterFirstMatch {
    fn new() -> Self {
        Self { found: false }
    }
}

impl grep::searcher::Sink for StopAfterFirstMatch {
    type Error = Box<dyn error::Error>;

    fn matched(
        &mut self,
        _searcher: &grep::searcher::Searcher,
        mat: &grep::searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let mat = String::from_utf8(mat.bytes().to_vec())?;
        let mat = mat.trim();
        if mat.starts_with("//") || mat.starts_with("//!") {
            // Continue if seeing a comment or doc comment.
            return Ok(true);
        }
        // Otherwise, we've found it: mark to true, and return false to indicate that we can stop
        // searching.
        self.found = true;
        Ok(false)
    }
}

fn search(path: PathBuf, text: &str) -> Result<bool, Box<dyn Error>> {
    let matcher = RegexMatcher::new_line_matcher(text)?;

    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .line_number(false)
        .build();

    for result in WalkDir::new(path) {
        let dent = match result {
            Ok(dent) => dent,
            Err(err) => {
                eprintln!("{}", err);
                continue;
            }
        };

        if !dent.file_type().is_file() {
            continue;
        }
        if dent
            .path()
            .extension()
            .map_or(true, |ext| ext.to_string_lossy() != "rs")
        {
            continue;
        }

        let mut sink = StopAfterFirstMatch::new();
        let result = searcher.search_path(&matcher, dent.path(), &mut sink);

        if let Err(err) = result {
            eprintln!("{}: {}", dent.path().display(), err);
        }

        if sink.found {
            return Ok(true);
        }
    }

    Ok(false)
}
