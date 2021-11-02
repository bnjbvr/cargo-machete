use std::{collections::HashSet, error, path::PathBuf};

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

    let mut paths = HashSet::new();
    if let Some(path) = analysis
        .manifest
        .lib
        .as_ref()
        .and_then(|lib| lib.path.as_ref())
    {
        paths.insert(path.clone());
    }

    for product in analysis
        .manifest
        .bin
        .iter()
        .chain(analysis.manifest.bench.iter())
        .chain(analysis.manifest.test.iter())
        .chain(analysis.manifest.example.iter())
    {
        if let Some(ref path) = product.path {
            paths.insert(path.clone());
        }
    }

    // TODO extend to dev dependencies + build dependencies, and be smarter in the grouping of
    // searched paths
    for (name, _) in analysis.manifest.dependencies.iter() {
        let snaked = name.replace('-', "_");

        // Look for:
        // use X:: / use X; / use X as / X:: / extern crate X;
        // TODO X:: could be YX::
        let pattern = format!(
            "use {snaked}(::|;| as)?|{snaked}::|extern crate {snaked}( |;)",
            snaked = snaked
        );

        let mut found_once = false;
        for path in &paths {
            let mut path = dir_path.join(path);
            // Remove the .rs suffix.
            path.pop();

            trace!("looking for {} in {}", pattern, path.to_string_lossy(),);
            match search(dir_path.join(path), &pattern) {
                Ok(found) => {
                    if found {
                        found_once = true;
                        break;
                    }
                }
                Err(err) => {
                    analysis.errors.push(err);
                    continue;
                }
            };
        }

        if !found_once {
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
            // TODO do something smarter! what about multiline strings containing //, etc.
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
        let dir_entry = match result {
            Ok(dir_entry) => dir_entry,
            Err(err) => {
                eprintln!("{}", err);
                continue;
            }
        };

        if !dir_entry.file_type().is_file() {
            continue;
        }

        if dir_entry
            .path()
            .extension()
            .map_or(true, |ext| ext.to_string_lossy() != "rs")
        {
            continue;
        }

        let mut sink = StopAfterFirstMatch::new();
        let result = searcher.search_path(&matcher, dir_entry.path(), &mut sink);

        if let Err(err) = result {
            eprintln!("{}: {}", dir_entry.path().display(), err);
        }

        if sink.found {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(test)]
const TOP_LEVEL: &str = concat!(env!("CARGO_MANIFEST_DIR"));

#[test]
fn test_just_unused() -> anyhow::Result<()> {
    // a crate that simply does not use a dependency it refers to
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./test_cases/just-unused/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["log".to_string()]);

    Ok(())
}

#[test]
fn test_unused_transitive() -> anyhow::Result<()> {
    // lib1 has zero dependencies
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./test_cases/unused-transitive/lib1/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    // lib2 effectively uses lib1
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./test_cases/unused-transitive/lib2/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    // but top level references both lib1 and lib2, and only uses lib2
    let analysis =
        find_unused(&PathBuf::from(TOP_LEVEL).join("./test_cases/unused-transitive/Cargo.toml"))?
            .expect("no error during processing");
    assert_eq!(analysis.unused, &["lib1".to_string()]);

    Ok(())
}

#[test]
fn test_macro_use() -> anyhow::Result<()> {
    // when a lib uses a dependency via a macro, there's no way we can find it by scanning the
    // source code.
    let analysis =
        find_unused(&PathBuf::from(TOP_LEVEL).join("./test_cases/false-positive-log/Cargo.toml"))?
            .expect("no error during processing");
    assert_eq!(analysis.unused, &["log".to_string()]);

    Ok(())
}
