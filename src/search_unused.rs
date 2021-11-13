use std::{
    collections::HashSet,
    error,
    path::{Path, PathBuf},
};

use grep::{
    regex::RegexMatcher,
    searcher::{BinaryDetection, Searcher, SearcherBuilder},
};
use log::{debug, trace};
use walkdir::WalkDir;

use crate::PackageAnalysis;

fn make_regexp(crate_name: &str) -> String {
    // Breaking down this regular expression:
    // - `use {name}(::|;| as)`: matches `use foo;`, `use foo::bar`, `use foo as bar;`.
    // - `(^|\\W)({name})::`: matches `foo::X`, but not `barfoo::X`.
    // - `extern crate {name}( |;)`: matches `extern crate foo`, or `extern crate foo as bar`.
    format!(
        "use {name}(::|;| as)|(^|\\W)({name})::|extern crate {name}( |;)",
        name = crate_name
    )
}

pub(crate) fn find_unused(manifest_path: &Path) -> anyhow::Result<Option<PackageAnalysis>> {
    let mut dir_path = manifest_path.to_path_buf();
    dir_path.pop();

    trace!("trying to open {}...", manifest_path.display());

    let manifest = cargo_toml::Manifest::from_path(manifest_path)?;
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

        let pattern = make_regexp(&snaked);

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

    Ok(Some(analysis))
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

enum SearchOneResult {
    Found(bool),
    Error(Box<dyn std::error::Error>),
}

trait Searchable {
    fn search(
        &self,
        matcher: &RegexMatcher,
        searcher: &mut Searcher,
        sink: &mut StopAfterFirstMatch,
    ) -> Result<(), Box<dyn error::Error>>;
}

impl Searchable for &str {
    #[inline]
    fn search(
        &self,
        matcher: &RegexMatcher,
        searcher: &mut Searcher,
        sink: &mut StopAfterFirstMatch,
    ) -> Result<(), Box<dyn error::Error>> {
        searcher.search_reader(matcher, self.as_bytes(), sink)
    }
}

impl Searchable for &Path {
    #[inline]
    fn search(
        &self,
        matcher: &RegexMatcher,
        searcher: &mut Searcher,
        sink: &mut StopAfterFirstMatch,
    ) -> Result<(), Box<dyn error::Error>> {
        searcher.search_path(matcher, self, sink)
    }
}

#[inline]
fn search_one<S: Searchable>(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    searchable: S,
) -> SearchOneResult {
    let mut sink = StopAfterFirstMatch::new();
    if let Err(err) = searchable.search(matcher, searcher, &mut sink) {
        SearchOneResult::Error(err)
    } else {
        SearchOneResult::Found(sink.found)
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

        match search_one(&mut searcher, &matcher, dir_entry.path()) {
            SearchOneResult::Found(found) => {
                if found {
                    return Ok(true);
                }
            }
            SearchOneResult::Error(err) => {
                eprintln!("{}: {}", dir_entry.path().display(), err);
            }
        }
    }

    Ok(false)
}

#[test]
fn test_regexp() -> anyhow::Result<()> {
    fn test_one(crate_name: &str, content: &str) -> anyhow::Result<bool> {
        let matcher = RegexMatcher::new_line_matcher(&make_regexp(crate_name))?;

        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(false)
            .build();

        if let SearchOneResult::Found(val) = search_one(&mut searcher, &matcher, content) {
            Ok(val)
        } else {
            unreachable!()
        }
    }

    assert!(!test_one("log", "use da_force_luke;")?);
    assert!(!test_one("log", "use flog;")?);
    assert!(!test_one("log", "use log_once;")?);
    assert!(!test_one("log", "use flog::flag;")?);
    assert!(!test_one("log", "flog::flag;")?);

    assert!(test_one("log", "use log;")?);
    assert!(test_one("log", "use log::{self};")?);
    assert!(test_one("log", "use log::*;")?);
    assert!(test_one("log", "use log::info;")?);
    assert!(test_one("log", "use log as logging;")?);
    assert!(test_one("log", "extern crate log;")?);
    assert!(test_one("log", "extern crate log as logging")?);
    assert!(test_one("log", r#"log::info!("fyi")"#)?);

    Ok(())
}

#[cfg(test)]
const TOP_LEVEL: &str = concat!(env!("CARGO_MANIFEST_DIR"));

#[test]
fn test_just_unused() -> anyhow::Result<()> {
    // a crate that simply does not use a dependency it refers to
    let analysis =
        find_unused(&PathBuf::from(TOP_LEVEL).join("./test_cases/just-unused/Cargo.toml"))?
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
