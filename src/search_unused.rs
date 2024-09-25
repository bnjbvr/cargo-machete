use cargo_metadata::CargoOpt;
use grep::{
    matcher::LineTerminator,
    regex::{RegexMatcher, RegexMatcherBuilder},
    searcher::{self, BinaryDetection, Searcher, SearcherBuilder, Sink},
};
use log::{debug, trace};
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, HashSet},
    error::{self, Error},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

use crate::UseCargoMetadata;
#[cfg(test)]
use crate::TOP_LEVEL;

use self::meta::PackageMetadata;

mod meta {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    pub struct PackageMetadata {
        #[serde(rename = "cargo-machete")]
        pub cargo_machete: Option<MetadataFields>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct MetadataFields {
        /// Crates triggering false positives in `cargo-machete`, which should not be reported as
        /// unused.
        pub ignored: Vec<String>,
    }
}

pub(crate) struct PackageAnalysis {
    metadata: Option<cargo_metadata::Metadata>,
    pub manifest: cargo_toml::Manifest<meta::PackageMetadata>,
    pub package_name: String,
    pub unused: Vec<String>,
    pub ignored_used: Vec<String>,
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
            unused: Vec::default(),
            ignored_used: Vec::default(),
        })
    }
}

fn make_line_regexp(name: &str) -> String {
    // Syntax documentation: https://docs.rs/regex/latest/regex/#syntax
    //
    // Breaking down this regular expression: given a line,
    // - `use (::)?(?i){name}(?-i)(::|;| as)`: matches `use foo;`, `use foo::bar`, `use foo as bar;`, with
    // an optional "::" in front of the crate's name.
    // - `(?:[^:]|^|\W::)\b(?i){name}(?-i)::`: matches `foo::X`, but not `barfoo::X`. To ensure there's no polluting
    //   prefix we add `(?:[^:]|^|\W::)\b`, meaning that the crate name must be prefixed by either:
    //    * Not a `:` (therefore not a sub module)
    //    * The start of a line
    //    * Not a word character followed by `::` (to allow ::my_crate)
    // - `extern crate (?i){name}(?-i)( |;)`: matches `extern crate foo`, or `extern crate foo as bar`.
    // - `(?i){name}(?-i)` makes the match against the crate's name case insensitive
    format!(
        r#"use (::)?(?i){name}(?-i)(::|;| as)|(?:[^:]|^|\W::)\b(?i){name}(?-i)::|extern crate (?i){name}(?-i)( |;)"#
    )
}

fn make_multiline_regexp(name: &str) -> String {
    // Syntax documentation: https://docs.rs/regex/latest/regex/#syntax
    //
    // Breaking down this Terrible regular expression: tries to match uses of the crate's name in
    // compound `use` statement across multiple lines.
    //
    // It's split into 3 parts:
    //   1. Matches modules before the usage of the crate's name: `\s*(?:(::)?\w+{sub_modules_match}\s*,\s*)*`
    //   2. Matches the crate's name with optional sub-modules: `(::)?{name}{sub_modules_match}\s*`
    //   3. Matches modules after the usage of the crate's name: `(?:\s*,\s*(::)?\w+{sub_modules_match})*\s*,?\s*`
    //
    // In order to avoid false usage detection of `not_my_dep::my_dep` the regexp ensures that the
    // crate's name is at the top level of the use statement. However, it's not possible with
    // regexp to allow any number of matching `{` and `}` before the crate's usage (rust regexp
    // engine doesn't support recursion). Therefore, sub modules are authorized up to 4 levels
    // deep.

    let sub_modules_match = r#"(?:::\w+)*(?:::\*|\s+as\s+\w+|::\{(?:[^{}]*(?:\{(?:[^{}]*(?:\{(?:[^{}]*(?:\{[^{}]*\})?[^{}]*)*\})?[^{}]*)*\})?[^{}]*)*\})?"#;

    format!(
        r#"use \{{\s*(?:(::)?\w+{sub_modules_match}\s*,\s*)*(::)?{name}{sub_modules_match}\s*(?:\s*,\s*(::)?\w+{sub_modules_match})*\s*,?\s*\}};"#
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
                    eprintln!("{err}");
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

    trace!("found transitive paths: {paths:?}");

    paths
}

/// Performs search of the given crate name with the following strategy: first try to use the line
/// matcher, then the multiline matcher if the line matcher failed.
///
/// Splitting the single line matcher from the multiline matcher makes maintenance of the regular
/// expressions simpler (oh well), and likely faster too since most use statements will be caught
/// by the single line matcher.
struct Search {
    line_matcher: RegexMatcher,
    line_searcher: Searcher,
    multiline_matcher: RegexMatcher,
    multiline_searcher: Searcher,
    sink: StopAfterFirstMatch,
}

impl Search {
    fn new(crate_name: &str) -> anyhow::Result<Self> {
        assert!(!crate_name.contains('-'));

        let line_matcher = RegexMatcher::new_line_matcher(&make_line_regexp(crate_name))?;
        let line_searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_terminator(LineTerminator::byte(b'\n'))
            .line_number(false)
            .build();

        let multiline_matcher = RegexMatcherBuilder::new()
            .multi_line(true)
            .build(&make_multiline_regexp(crate_name))?;
        let multiline_searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .multi_line(true)
            .line_number(false)
            .build();

        // Sanity-check: the matcher must allow multi-line searching.
        debug_assert!(multiline_searcher.multi_line_with_matcher(&multiline_matcher));

        let sink = StopAfterFirstMatch::new();

        Ok(Self {
            line_matcher,
            line_searcher,
            multiline_matcher,
            multiline_searcher,
            sink,
        })
    }

    fn try_singleline_then_multiline<
        F: FnMut(&mut Searcher, &RegexMatcher, &mut StopAfterFirstMatch) -> Result<(), Box<dyn Error>>,
    >(
        &mut self,
        mut func: F,
    ) -> anyhow::Result<bool> {
        match func(&mut self.line_searcher, &self.line_matcher, &mut self.sink) {
            Ok(()) => {
                if self.sink.found {
                    return Ok(true);
                }
                // Single line matcher didn't work, try the multiline matcher now.
                func(
                    &mut self.multiline_searcher,
                    &self.multiline_matcher,
                    &mut self.sink,
                )
                .map_err(|err| anyhow::anyhow!("when searching with complex pattern: {err}"))
                .map(|()| self.sink.found)
            }
            Err(err) => anyhow::bail!("when searching with line pattern: {err}"),
        }
    }

    fn search_path(&mut self, path: &Path) -> anyhow::Result<bool> {
        self.try_singleline_then_multiline(|searcher, matcher, sink| {
            searcher.search_path(matcher, path, sink)
        })
    }

    #[cfg(test)]
    fn search_string(&mut self, s: &str) -> anyhow::Result<bool> {
        self.try_singleline_then_multiline(|searcher, matcher, sink| {
            searcher.search_reader(matcher, s.as_bytes(), sink)
        })
    }
}

/// Read a manifest and try to find a workspace manifest to complete the data available in the
/// manifest.
///
/// This will look up the file tree to find the Cargo.toml workspace manifest, assuming it's on a
/// parent directory.
fn get_full_manifest(
    dir_path: &Path,
    manifest_path: &Path,
) -> anyhow::Result<(cargo_toml::Manifest<PackageMetadata>, Vec<String>)> {
    // HACK: we can't plain use `from_path_with_metadata` here, because it calls
    // `complete_from_path` just a bit too early (before we've had a chance to call
    // `inherit_workspace`). See https://gitlab.com/crates.rs/cargo_toml/-/issues/20 for details,
    // and a possible future fix.
    let cargo_toml_content = std::fs::read(manifest_path)?;
    let mut manifest =
        cargo_toml::Manifest::<PackageMetadata>::from_slice_with_metadata(&cargo_toml_content)?;

    let mut ws_manifest_and_path = None;
    let mut workspace_ignored = vec![];

    let mut dir_path = dir_path.to_path_buf();
    while dir_path.pop() {
        let workspace_cargo_path = dir_path.join("Cargo.toml");
        if let Ok(workspace_manifest) =
            cargo_toml::Manifest::<PackageMetadata>::from_path_with_metadata(&workspace_cargo_path)
        {
            if let Some(workspace) = &workspace_manifest.workspace {
                // Look for `workspace.metadata.cargo-machete.ignored` in the workspace Cargo.toml.
                if let Some(ignored) = workspace
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.cargo_machete.as_ref())
                    .map(|machete| &machete.ignored)
                {
                    workspace_ignored.clone_from(ignored);
                }

                ws_manifest_and_path = Some((workspace_manifest, workspace_cargo_path));
                break;
            }
        }
    }

    manifest.complete_from_path_and_workspace(
        manifest_path,
        ws_manifest_and_path.as_ref().map(|(m, p)| (m, p.as_path())),
    )?;

    Ok((manifest, workspace_ignored))
}

pub(crate) fn find_unused(
    manifest_path: &Path,
    with_cargo_metadata: UseCargoMetadata,
) -> anyhow::Result<Option<PackageAnalysis>> {
    let mut dir_path = manifest_path.to_path_buf();
    dir_path.pop();

    trace!("trying to open {}...", manifest_path.display());

    let (manifest, workspace_ignored) = get_full_manifest(&dir_path, manifest_path)?;

    let package_name = match manifest.package {
        Some(ref package) => package.name.clone(),
        None => return Ok(None),
    };

    debug!("handling {} ({})", package_name, dir_path.display());

    let mut analysis = PackageAnalysis::new(
        package_name,
        manifest_path,
        manifest,
        matches!(with_cargo_metadata, UseCargoMetadata::Yes),
    )?;

    let paths = collect_paths(&dir_path, &analysis);

    // TODO extend to dev dependencies + build dependencies, and be smarter in the grouping of
    // searched paths
    // Maps dependency name (the name of the key in the Cargo.toml dependency
    // table, can have dashes, not necessarily the name in the crate registry)
    // to crate name (extern crate, snake case)
    let dependencies: BTreeMap<String, String> = if let Some((metadata, resolve)) = analysis
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.resolve.as_ref().map(|resolve| (metadata, resolve)))
    {
        if let Some(ref root) = resolve.root {
            // This gives us resolved dependencies, in crate form
            let root_node = resolve
                .nodes
                .iter()
                .find(|node| node.id == *root)
                .expect("root should be resolved by cargo-metadata");
            // This gives us the original dependency table
            // May have more than resolved if some were never enabled
            let root_package = metadata
                .packages
                .iter()
                .find(|pkg| pkg.id == *root)
                .expect("root should appear under cargo-metadata packages");
            // For every resolved dependency:
            // look it up in the package list to find the name (the one in registries)
            // look up that name in dependencies of the root_package;
            // find if it uses a different key through the rename field
            root_node
                .deps
                .iter()
                .map(|dep| {
                    let crate_name = dep.name.clone();
                    let dep_pkg = metadata
                        .packages
                        .iter()
                        .find(|pkg| pkg.id == dep.pkg)
                        .expect(
                            "resolved dependencies should appear under cargo-metadata packages",
                        );

                    let mut dep_spec_it = root_package
                        .dependencies
                        .iter()
                        .filter(|dep_spec| dep_spec.name == dep_pkg.name);

                    // The dependency can appear more than once, for example if it is both
                    // a dependency and a dev-dependency (often with more features enabled).
                    // We'll assume cargo enforces consistency.
                    let dep_spec = dep_spec_it
                        .next()
                        .expect("resolved dependency should have a matching dependency spec");

                    // If the dependency was renamed, through key = { package = … },
                    // the original key is in dep_spec.rename.
                    let dep_key = dep_spec
                        .rename
                        .clone()
                        .unwrap_or_else(|| dep_spec.name.clone());
                    (dep_key, crate_name)
                })
                .collect()
        } else {
            // No root -> virtual workspace, empty map
            Default::default()
        }
    } else {
        analysis
            .manifest
            .dependencies
            .keys()
            .map(|k| (k.clone(), k.replace('-', "_")))
            .collect()
    };

    // Keep a side-list of ignored dependencies (likely false positives).
    let ignored = analysis
        .manifest
        .package
        .as_ref()
        .and_then(|package| package.metadata.as_ref())
        .and_then(|meta| meta.cargo_machete.as_ref())
        .map(|meta| meta.ignored.iter().collect::<HashSet<_>>())
        .unwrap_or_default();

    let workspace_ignored: HashSet<_> = workspace_ignored.into_iter().collect();

    enum SingleDepResult {
        /// Dependency is unused and not marked as ignored.
        Unused(String),
        /// Dependency is marked as ignored but used.
        IgnoredButUsed(String),
    }

    let results: Vec<SingleDepResult> = dependencies
        .into_par_iter()
        .filter_map(|(dep_name, crate_name)| {
            let mut search = Search::new(&crate_name).expect("constructing grep context");

            let mut found_once = false;
            for path in &paths {
                trace!("looking for {} in {}", crate_name, path.to_string_lossy(),);
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

            if !found_once {
                if ignored.contains(&dep_name) || workspace_ignored.contains(&dep_name) {
                    return None;
                }

                Some(SingleDepResult::Unused(dep_name))
            } else {
                if ignored.contains(&dep_name) {
                    return Some(SingleDepResult::IgnoredButUsed(dep_name));
                }

                None
            }
        })
        .collect();

    for result in results {
        match result {
            SingleDepResult::Unused(dep) => analysis.unused.push(dep),
            SingleDepResult::IgnoredButUsed(dep) => analysis.ignored_used.push(dep),
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
        matsh: &searcher::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        let mat = String::from_utf8(matsh.bytes().to_vec())?;
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
    assert!(!test_one("log", "use ::flog;")?);
    assert!(!test_one("log", "use :log;")?);

    assert!(test_one("log", "use log;")?);
    assert!(test_one("log", "use ::log;")?);
    assert!(test_one("log", "use log::{self};")?);
    assert!(test_one("log", "use log::*;")?);
    assert!(test_one("log", "use log::info;")?);
    assert!(test_one("log", "use log as logging;")?);
    assert!(test_one("log", "extern crate log;")?);
    assert!(test_one("log", "extern crate log as logging")?);
    assert!(test_one("log", r#"log::info!("fyi")"#)?);

    assert!(test_one("Log", "use log;")?);
    assert!(test_one("Log", "use ::log;")?);
    assert!(test_one("Log", "use log::{self};")?);
    assert!(test_one("Log", "use log::*;")?);
    assert!(test_one("Log", "use log::info;")?);
    assert!(test_one("Log", "use log as logging;")?);
    assert!(test_one("Log", "extern crate log;")?);
    assert!(test_one("Log", "extern crate log as logging")?);
    assert!(test_one("Log", r#"log::info!("fyi")"#)?);

    assert!(test_one("log", "use Log;")?);
    assert!(test_one("log", "use ::Log;")?);
    assert!(test_one("log", "use Log::{self};")?);
    assert!(test_one("log", "use Log::*;")?);
    assert!(test_one("log", "use Log::info;")?);
    assert!(test_one("log", "use Log as logging;")?);
    assert!(test_one("log", "extern crate Log;")?);
    assert!(test_one("log", "extern crate Log as logging")?);
    assert!(test_one("log", r#"Log::info!("fyi")"#)?);

    assert!(test_one(
        "bitflags",
        r#"
use std::fmt;
bitflags::macro! {
"#
    )?);

    assert!(test_one(
        "Bitflags",
        r#"
use std::fmt;
bitflags::macro! {
"#
    )?);

    assert!(test_one(
        "bitflags",
        r#"
use std::fmt;
Bitflags::macro! {
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

    // Regression test.
    // Comments and spaces are meaningful here.
    assert!(test_one(
        "static_assertions",
        r#"
    // lol
    static_assertions::assert_not_impl_all!(A: B);
    "#
    )?);

    // Regression test.
    // Comments and spaces are meaningful here.
    assert!(test_one(
        "futures",
        r#"
// the [`futures::executor::block_on`] function
pub use futures::future;

    "#
    )?);

    // multi-dep single use statements
    assert!(test_one(
        "futures",
        r#"pub use {async_trait, futures, reqwest};"#
    )?);

    // multi-dep single use statements with ::
    assert!(test_one(
        "futures",
        r#"pub use {async_trait, ::futures, reqwest};"#
    )?);

    // No false usage detection of `not_my_dep::my_dep` on compound imports
    assert!(!test_one(
        "futures",
        r#"pub use {async_trait, not_futures::futures, reqwest};"#
    )?);

    // No false usage detection of `not_my_dep::my_dep` on multiple lines
    assert!(!test_one(
        "futures",
        r#"
pub use {
    async_trait,
    not_futures::futures,
    reqwest,
};"#
    )?);

    // No false usage detection on single line `not_my_dep::my_dep`
    assert!(!test_one(
        "futures",
        r#"use not_futures::futures::stuff_in_futures;"#
    )?);

    // multi-dep single use statements with nesting
    assert!(test_one(
        "futures",
        r#"pub use {
            async_trait::{mod1, dep2},
            futures::{futures_mod1, futures_mod2::{futures_mod21, futures_mod22}},
            reqwest,
        };"#
    )?);

    // multi-dep single use statements with star import and renaming
    assert!(test_one(
        "futures",
        r#"pub use {
            async_trait::sub_mod::*,
            futures as futures_renamed,
            reqwest,
        };"#
    )?);

    // multi-dep single use statements with complex imports and renaming
    assert!(test_one(
        "futures",
        r#"pub use {
            other_dep::{
                star_mod::*,
                unnamed_import::{UnnamedTrait as _, other_mod},
                renamed_import as new_name,
                sub_import::{mod1, mod2},
            },
            futures as futures_renamed,
            reqwest,
        };"#
    )?);

    // No false usage detection of `not_my_dep::my_dep` with nesting
    assert!(!test_one(
        "futures",
        r#"pub use {
            async_trait::{mod1, dep2},
            not_futures::futures::{futures_mod1, futures_mod2::{futures_mod21, futures_mod22}},
            reqwest,
        };"#
    )?);

    // Detects top level usage
    assert!(test_one("futures", r#" ::futures::mod1"#)?);

    Ok(())
}

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
fn test_just_unused_with_manifest() {
    // a crate that does not use a dependency it refers to, and uses workspace properties
    check_analysis(
        "./integration-tests/workspace-package/program/Cargo.toml",
        |analysis| {
            assert_eq!(analysis.unused, &["log".to_string()]);
        },
    );
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
fn test_unused_renamed_in_registry() -> anyhow::Result<()> {
    // when a lib like xml-rs is exposed with a different name,
    // cargo-machete reports the unused spec properly.
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/unused-renamed-in-registry/Cargo.toml"),
        UseCargoMetadata::Yes,
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["xml-rs".to_string()]);

    Ok(())
}

#[test]
fn test_unused_renamed_in_spec() -> anyhow::Result<()> {
    // when a lib is renamed through key = { package = … },
    // cargo-machete reports the unused spec properly.
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/unused-renamed-in-spec/Cargo.toml"),
        UseCargoMetadata::Yes,
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["tracing".to_string()]);

    Ok(())
}

#[test]
fn test_unused_kebab_spec() -> anyhow::Result<()> {
    // when a lib uses kebab naming, cargo-machete reports the unused spec properly.
    let analysis = find_unused(
        &PathBuf::from(TOP_LEVEL).join("./integration-tests/unused-kebab-spec/Cargo.toml"),
        UseCargoMetadata::Yes,
    )?
    .expect("no error during processing");
    assert_eq!(analysis.unused, &["log-once".to_string()]);

    Ok(())
}

#[test]
fn test_ignore_deps_works() {
    // ensure that ignored deps listed in Cargo.toml package.metadata.cargo-machete.ignored are
    // correctly ignored.
    check_analysis("./integration-tests/ignored-dep/Cargo.toml", |analysis| {
        assert_eq!(analysis.unused, &["rand".to_string()]);
        assert_eq!(analysis.ignored_used, &["rand_core".to_string()]);
    });
}

#[test]
fn test_ignore_deps_workspace_works() {
    // ensure that ignored deps listed in Cargo.toml workspace.metadata.cargo-machete.ignored are
    // correctly ignored.
    check_analysis(
        "./integration-tests/ignored-dep-workspace/inner/Cargo.toml",
        |analysis| {
            assert_eq!(analysis.unused, &["rand".to_string()]);
            assert_eq!(analysis.ignored_used, &["rand_core".to_string()]);
        },
    );
}
