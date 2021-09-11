use std::{error, fs};
use std::{error::Error, path::PathBuf};

use grep::regex::RegexMatcher;
use grep::searcher::{BinaryDetection, SearcherBuilder};
use log::info;
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

fn to_snake_case(name: &str) -> String {
    name.replace('-', "_")
}

fn handle_one(manifest_path: PathBuf, fix: bool) -> Result<(), BoxedError> {
    let mut dir_path = manifest_path.clone();
    dir_path.pop();

    let mut manifest = cargo_toml::Manifest::from_path(manifest_path.clone())?;
    let package_name = match manifest.package {
        Some(ref package) => &package.name,
        None => return Ok(()),
    };

    info!("handling {}", package_name);

    let mut to_remove = Vec::new();

    for (name, _) in manifest.dependencies.iter() {
        let snaked = to_snake_case(&name) + "::";
        info!(
            "looking for {} in {}",
            snaked,
            manifest_path.to_string_lossy()
        );
        match search(dir_path.clone(), &snaked) {
            Ok(found) => {
                if !found {
                    info!("remove {}", name);
                    to_remove.push(name.clone());
                }
            }
            Err(err) => {
                eprintln!("error: {}", err)
            }
        }
    }

    if !to_remove.is_empty() {
        println!("{} ({}):", package_name, dir_path.to_string_lossy());

        for entry in to_remove {
            println!("  {}", entry);
            manifest.dependencies.remove(&entry);
        }

        if fix {
            info!("rewriting Cargo.toml");
            let serialized = toml::to_string(&manifest)?;
            fs::write(manifest_path, serialized)?;
        }
    }

    Ok(())
}

fn main() -> Result<(), BoxedError> {
    pretty_env_logger::init();

    let mut fix = false;
    let args = std::env::args();
    for arg in args {
        if arg == "--fix" || arg == "fix" {
            fix = true;
        }
    }

    let cwd = std::env::current_dir()?;
    for entry in WalkDir::new(cwd) {
        let entry = entry?;
        if entry.file_name() == "Cargo.toml" {
            handle_one(entry.into_path(), fix)?;
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
            return Ok(true);
        }
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
