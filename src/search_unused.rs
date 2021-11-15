use std::{
    collections::HashSet,
    error,
    path::{Path, PathBuf},
};

use grep::{
    regex::RegexMatcher,
    searcher::{self, BinaryDetection, Searcher, SearcherBuilder, Sink},
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

/// Returns all the paths to the Rust source files for a crate contained at the given path.
fn collect_paths(dir_path: &Path, analysis: &PackageAnalysis) -> Vec<PathBuf> {
    let mut root_paths = HashSet::new();

    if let Some(path) = analysis
        .manifest
        .lib
        .as_ref()
        .and_then(|lib| lib.path.as_ref())
    {
        assert!(
            path.ends_with(".rs"),
            "paths provided by cargo_toml are to Rust files"
        );
        let mut path_buf = PathBuf::from(path);
        // Remove .rs extension.
        path_buf.pop();
        root_paths.insert(path_buf);
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
            assert!(
                path.ends_with(".rs"),
                "paths provided by cargo_toml are to Rust files"
            );
            let mut path_buf = PathBuf::from(path);
            // Remove .rs extension.
            path_buf.pop();
            root_paths.insert(path_buf);
        }
    }

    trace!("found root paths: {:?}", root_paths);

    if root_paths.is_empty() {
        // Assume "src/" if cargo_toml didn't find anything.
        root_paths.insert(dir_path.join("src"));
        trace!("adding src/ since paths was empty");
    }

    // Collect all final paths for the crate first.
    let paths: Vec<PathBuf> = root_paths
        .iter()
        .map(|root| WalkDir::new(dir_path.join(root)).into_iter())
        .flatten()
        .filter_map(|result| {
            let dir_entry = match result {
                Ok(dir_entry) => dir_entry,
                Err(err) => {
                    eprintln!("{}", err);
                    return None;
                }
            };
            if !dir_entry.file_type().is_file() {
                return None;
            }
            if dir_entry
                .path()
                .extension()
                .map_or(true, |ext| ext.to_string_lossy() != "rs")
            {
                return None;
            }
            Some(dir_path.join(dir_entry.path()))
        })
        .collect();

    trace!("found transitive paths: {:?}", paths);

    paths
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

    let paths = collect_paths(&dir_path, &analysis);

    // TODO extend to dev dependencies + build dependencies, and be smarter in the grouping of
    // searched paths
    for (name, _) in analysis.manifest.dependencies.iter() {
        let snaked = name.replace('-', "_");
        let pattern = make_regexp(&snaked);

        let matcher = RegexMatcher::new_line_matcher(&pattern)?;
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(false)
            .build();

        let mut found_once = false;
        for path in &paths {
            trace!("looking for {} in {}", pattern, path.to_string_lossy(),);
            match search_one(&mut searcher, &matcher, &**path) {
                Ok(true) => {
                    found_once = true;
                    break;
                }
                Ok(false) => {}
                Err(err) => {
                    eprintln!("{}: {}", path.display(), err);
                }
            }
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

impl Sink for StopAfterFirstMatch {
    type Error = Box<dyn error::Error>;

    fn matched(
        &mut self,
        _searcher: &searcher::Searcher,
        mat: &searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let mat = String::from_utf8(mat.bytes().to_vec())?;
        let mat = mat.trim();

        if mat.starts_with("//") || mat.starts_with("//!") {
            // Continue if seeing what resembles a comment or doc comment. Unfortunately we can't
            // do anything better because trying to figure whether we're within a (doc) comment
            // would require actual parsing of the Rust code.
            return Ok(true);
        }

        // Otherwise, we've found it: mark to true, and return false to indicate that we can stop
        // searching.
        self.found = true;
        Ok(false)
    }
}

trait Searchable: std::fmt::Debug {
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
) -> anyhow::Result<bool> {
    trace!("searching in {:?}", searchable);
    let mut sink = StopAfterFirstMatch::new();
    searchable
        .search(matcher, searcher, &mut sink)
        .map_err(|err| anyhow::anyhow!("when searching: {}", err))
        .map(|_| sink.found)
}

#[test]
fn test_regexp() -> anyhow::Result<()> {
    fn test_one(crate_name: &str, content: &str) -> anyhow::Result<bool> {
        let matcher = RegexMatcher::new_line_matcher(&make_regexp(crate_name))?;
        let mut searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(false)
            .build();
        search_one(&mut searcher, &matcher, content)
    }

    assert!(!test_one("log", "use da_force_luke;")?);
    assert!(!test_one("log", "use flog;")?);
    assert!(!test_one("log", "use log_once;")?);
    assert!(!test_one("log", "use log_once::info;")?);
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
    assert!(test_one("bitflags", "bitflags::bitflags! {")?);

    Ok(())
}

#[cfg(test)]
const TOP_LEVEL: &str = concat!(env!("CARGO_MANIFEST_DIR"));

#[test]
fn test_just_unused() -> anyhow::Result<()> {
    // a crate that simply does not use a dependency it refers to
    let analysis =
        find_unused(&PathBuf::from(TOP_LEVEL).join("./integration-tests/just-unused/Cargo.toml"))?
            .expect("no error during processing");
    assert_eq!(analysis.unused, &["log".to_string()]);

    Ok(())
}

#[test]
fn test_unused_transitive() -> anyhow::Result<()> {
    // lib1 has zero dependencies
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/unused-transitive/lib1/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    // lib2 effectively uses lib1
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/unused-transitive/lib2/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    // but top level references both lib1 and lib2, and only uses lib2
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/unused-transitive/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["lib1".to_string()]);

    Ok(())
}

#[test]
fn test_false_positive_macro_use() -> anyhow::Result<()> {
    // when a lib uses a dependency via a macro, there's no way we can find it by scanning the
    // source code.
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/false-positive-log/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["log".to_string()]);

    Ok(())
}

#[test]
fn test_with_bench() -> anyhow::Result<()> {
    // when a package has a bench file designated by binary name, it seems that `cargo_toml`
    // doesn't fill in a default path to the source code.
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-bench/bench/Cargo.toml"),
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    Ok(())
}
