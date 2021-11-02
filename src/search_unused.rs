use std::{error, path::PathBuf};

use grep::{
    regex::RegexMatcher,
    searcher::{BinaryDetection, SearcherBuilder},
};
use log::{debug, trace};
use walkdir::WalkDir;

use crate::PackageAnalysis;

pub(crate) fn find_unused(manifest_path: &PathBuf) -> anyhow::Result<Option<PackageAnalysis>> {
    let mut dir_path = manifest_path.clone();
    dir_path.pop();

    trace!("trying to open {}...", manifest_path.display());

    let manifest = cargo_toml::Manifest::from_path(manifest_path.clone())?;
    let package_name = match manifest.package {
        Some(ref package) => &package.name,
        None => return Ok(None),
    };

    debug!("handling {} ({})", package_name, dir_path.display());

    let mut analysis = PackageAnalysis::new(package_name.clone(), manifest);

    for (name, _) in analysis.manifest.dependencies.iter() {
        let snaked = name.replace('-', "_");

        // Look for:
        // use X:: / use X; / use X as / X:: / extern crate X;
        // TODO X:: could be YX::
        let pattern = format!(
            "use {snaked}(::|;| as)?|{snaked}::|extern crate {snaked}( |;)",
            snaked = snaked
        );

        trace!(
            "looking for {} in {}",
            pattern,
            manifest_path.to_string_lossy()
        );

        let found = match search(dir_path.clone(), &pattern) {
            Ok(found) => found,
            Err(err) => {
                analysis.errors.push(err);
                continue;
            }
        };

        if !found {
            debug!("{} might be unused", name);
            analysis.unused.push(name.clone());
        }
    }

    return Ok(Some(analysis));
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

fn search(path: PathBuf, text: &str) -> anyhow::Result<bool> {
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
