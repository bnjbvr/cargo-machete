use cargo_metadata::CargoOpt;
use grep::{
    regex::{RegexMatcher, RegexMatcherBuilder},
    searcher::{self, BinaryDetection, Searcher, SearcherBuilder, Sink},
};
use log::{debug, trace};
use rayon::prelude::*;
use std::{
    collections::HashSet,
    error,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

mod meta {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    pub struct PackageMetadata {
        #[serde(rename = "cargo-machete")]
        pub cargo_machete: Option<Ignored>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct Ignored {
        pub ignored: Vec<String>,
    }
}

pub(crate) struct PackageAnalysis {
    metadata: Option<cargo_metadata::Metadata>,
    pub manifest: cargo_toml::Manifest<meta::PackageMetadata>,
    pub package_name: String,
    pub unused: Vec<String>,
}

impl PackageAnalysis {
    fn new(
        package_name: String,
        cargo_path: &Path,
        manifest: cargo_toml::Manifest<meta::PackageMetadata>,
        with_cargo_metadata: bool,
    ) -> anyhow::Result<Self> {
        let metadata = if with_cargo_metadata {
            Some(
                cargo_metadata::MetadataCommand::new()
                    .features(CargoOpt::AllFeatures)
                    .manifest_path(cargo_path)
                    //.other_options(["--frozen".to_owned()]) // TODO causes errors in cargo-metadata
                    .exec()?,
            )
        } else {
            None
        };

        Ok(Self {
            metadata,
            manifest,
            package_name,
            unused: Default::default(),
        })
    }
}

fn make_regexp(name: &str) -> String {
    // Breaking down this regular expression: given a line,
    // - `use {name}(::|;| as)`: matches `use foo;`, `use foo::bar`, `use foo as bar;`.
    // - `(^|\\W)({name})::`: matches `foo::X`, but not `barfoo::X`. Note the `^` refers to the
    // beginning of the line (because of multi-line mode), not the beginning of the input.
    // - `extern crate {name}( |;)`: matches `extern crate foo`, or `extern crate foo as bar`.
    // - `use \\{{\\s((?s).*(?-s)){name}\\s*as\\s*((?s).*(?-s))\\}};`: The Terrible One: tries to
    // match compound use as statements, as in `use { X as Y };`, with possibly multiple-lines in
    // between. Will match the first `};` that it finds, which *should* be the end of the use
    // statement, but oh well.
    format!(
        "use {name}(::|;| as)|(^|\\W)({name})::|extern crate {name}( |;)|use \\{{\\s[^;]*{name}\\s*as\\s*[^;]*\\}};"
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
        root_paths.insert(PathBuf::from("src"));
        trace!("adding src/ since paths was empty");
    }

    // Collect all final paths for the crate first.
    let paths: Vec<PathBuf> = root_paths
        .iter()
        .flat_map(|root| WalkDir::new(dir_path.join(root)).into_iter())
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
            Some(dir_entry.path().to_owned())
        })
        .collect();

    trace!("found transitive paths: {:?}", paths);

    paths
}

struct Search {
    matcher: RegexMatcher,
    searcher: Searcher,
    sink: StopAfterFirstMatch,
}

impl Search {
    fn new(crate_name: &str) -> anyhow::Result<Self> {
        let snaked = crate_name.replace('-', "_");
        let pattern = make_regexp(&snaked);
        let matcher = RegexMatcherBuilder::new()
            .multi_line(true)
            .build(&pattern)?;

        let searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .multi_line(true)
            .line_number(false)
            .build();

        // Sanity-check: the matcher must allow multi-line searching.
        debug_assert!(searcher.multi_line_with_matcher(&matcher));

        let sink = StopAfterFirstMatch::new();

        Ok(Self {
            matcher,
            searcher,
            sink,
        })
    }

    fn search_path(&mut self, path: &Path) -> Result<bool, anyhow::Error> {
        self.searcher
            .search_path(&self.matcher, path, &mut self.sink)
            .map_err(|err| anyhow::anyhow!("when searching: {}", err))
            .map(|_| self.sink.found)
    }

    #[cfg(test)]
    fn search_string(&mut self, s: &str) -> Result<bool, anyhow::Error> {
        self.searcher
            .search_reader(&self.matcher, s.as_bytes(), &mut self.sink)
            .map_err(|err| anyhow::anyhow!("when searching: {}", err))
            .map(|_| self.sink.found)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum UseCargoMetadata {
    Yes,
    No,
}

#[cfg(test)]
impl UseCargoMetadata {
    fn all() -> &'static [UseCargoMetadata] {
        &[UseCargoMetadata::Yes, UseCargoMetadata::No]
    }
}

impl From<UseCargoMetadata> for bool {
    fn from(v: UseCargoMetadata) -> bool {
        matches!(v, UseCargoMetadata::Yes)
    }
}

pub(crate) fn find_unused(
    manifest_path: &Path,
    with_cargo_metadata: UseCargoMetadata,
) -> anyhow::Result<Option<PackageAnalysis>> {
    let mut dir_path = manifest_path.to_path_buf();
    dir_path.pop();

    trace!("trying to open {}...", manifest_path.display());

    let manifest = cargo_toml::Manifest::from_path_with_metadata(manifest_path)?;
    let package_name = match manifest.package {
        Some(ref package) => package.name.clone(),
        None => return Ok(None),
    };

    debug!("handling {} ({})", package_name, dir_path.display());

    let mut analysis = PackageAnalysis::new(
        package_name.clone(),
        manifest_path,
        manifest,
        with_cargo_metadata.into(),
    )?;

    let paths = collect_paths(&dir_path, &analysis);

    // TODO extend to dev dependencies + build dependencies, and be smarter in the grouping of
    // searched paths
    let mut dependencies_names: Vec<_> = if let Some(resolve) = analysis
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.resolve.as_ref())
    {
        resolve
            .nodes
            .iter()
            .find(|node| {
                // e.g. "aa 0.1.0 (path+file:///tmp/aa)"
                node.id
                    .repr
                    .split(' ')
                    .next() // e.g. "aa"
                    .map_or(false, |node_package_name| node_package_name == package_name)
            })
            .expect("the current package must be in the dependency graph")
            .deps
            .iter()
            .map(|node_dep| node_dep.name.clone())
            .collect()
    } else {
        analysis.manifest.dependencies.keys().cloned().collect()
    };

    if let Some(meta) = analysis
        .manifest
        .package
        .as_ref()
        .unwrap()
        .metadata
        .as_ref()
    {
        if let Some(cargo_machete) = meta.cargo_machete.as_ref() {
            for ignore in cargo_machete.ignored.iter() {
                if let Some(pos) = dependencies_names.iter().position(|x| x == ignore) {
                    dependencies_names.remove(pos);
                }
            }
        }
    }

    analysis.unused = dependencies_names
        .into_par_iter()
        .filter_map(|name| {
            let mut search = Search::new(&name).expect("constructing grep context ");

            let mut found_once = false;
            for path in &paths {
                trace!("looking for {} in {}", name, path.to_string_lossy(),);
                match search.search_path(path) {
                    Ok(true) => {
                        found_once = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(err) => {
                        eprintln!("{}: {}", path.display(), err);
                    }
                };
            }

            (!found_once).then(|| name)
        })
        .collect();

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

#[test]
fn test_regexp() -> anyhow::Result<()> {
    fn test_one(crate_name: &str, content: &str) -> anyhow::Result<bool> {
        let mut search = Search::new(crate_name)?;
        search.search_string(content)
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

    assert!(test_one(
        "bitflags",
        r#"
use std::fmt;
bitflags::macro! {
"#
    )?);

    // Compound `use as` statements. Here come the nightmares...
    assert!(test_one("log", "use { log as logging };")?);
    assert!(!test_one("lol", "use { log as logging };")?);

    assert!(test_one(
        "log",
        r#"
use {
    log as logging
};
"#
    )?);

    assert!(test_one(
        "log",
        r#"
use { log as
logging
};
"#
    )?);

    assert!(test_one(
        "log",
        r#"
use { log
    as
        logging
};
"#
    )?);

    assert!(test_one(
        "log",
        r#"
use {
    x::{ y },
    log as logging,
};
"#
    )?);

    // Regex must stop at the first };
    assert!(!test_one(
        "log",
        r#"
use {
    x as y
};
type logging = u64;
fn main() {
    let func = |log: u32| {
        log as logging
    };
    func(42);
}
"#
    )?);

    Ok(())
}

#[cfg(test)]
const TOP_LEVEL: &str = concat!(env!("CARGO_MANIFEST_DIR"));

#[cfg(test)]
fn check_analysis<F: Fn(PackageAnalysis)>(rel_path: &str, callback: F) {
    for use_cargo_metadata in UseCargoMetadata::all() {
        let analysis = find_unused(
            &PathBuf::from(TOP_LEVEL).join(rel_path),
            *use_cargo_metadata,
        )
        .expect("find_unused must return an Ok result")
        .expect("no error during processing");
        callback(analysis);
    }
}

#[test]
fn test_just_unused() {
    // a crate that simply does not use a dependency it refers to
    check_analysis("./integration-tests/just-unused/Cargo.toml", |analysis| {
        assert_eq!(analysis.unused, &["log".to_string()]);
    });
}

#[test]
fn test_unused_transitive() {
    // lib1 has zero dependencies
    check_analysis(
        "./integration-tests/unused-transitive/lib1/Cargo.toml",
        |analysis| {
            assert!(analysis.unused.is_empty());
        },
    );

    // lib2 effectively uses lib1
    check_analysis(
        "./integration-tests/unused-transitive/lib2/Cargo.toml",
        |analysis| {
            assert!(analysis.unused.is_empty());
        },
    );

    // but top level references both lib1 and lib2, and only uses lib2
    check_analysis(
        "./integration-tests/unused-transitive/Cargo.toml",
        |analysis| {
            assert_eq!(analysis.unused, &["lib1".to_string()]);
        },
    );
}

#[test]
fn test_false_positive_macro_use() {
    // when a lib uses a dependency via a macro, there's no way we can find it by scanning the
    // source code.
    check_analysis(
        "./integration-tests/false-positive-log/Cargo.toml",
        |analysis| {
            assert_eq!(analysis.unused, &["log".to_string()]);
        },
    );
}

#[test]
fn test_with_bench() {
    // when a package has a bench file designated by binary name, it seems that `cargo_toml`
    // doesn't fill in a default path to the source code.
    check_analysis(
        "./integration-tests/with-bench/bench/Cargo.toml",
        |analysis| {
            assert!(analysis.unused.is_empty());
        },
    );
}

#[test]
fn test_crate_renaming_works() -> anyhow::Result<()> {
    // when a lib like xml-rs is exposed with a different name, cargo-machete doesn't return false
    // positives.
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/renaming-works/Cargo.toml"),
        UseCargoMetadata::Yes,
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    // But when not using cargo-metadata, there's a false positive!
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/renaming-works/Cargo.toml"),
        UseCargoMetadata::No,
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["xml-rs".to_string()]);

    Ok(())
}

#[test]
fn test_ignore_deps_works() -> anyhow::Result<()> {
    // ensure that ignored deps listed in Cargo.toml package.metadata.cargo-machete.ignore are
    // correctly ignored
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/ignored-dep/Cargo.toml"),
        UseCargoMetadata::Yes,
    )?
    .expect("no error during processing");
    assert!(analysis.unused.is_empty());

    Ok(())
}
