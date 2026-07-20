pub mod json;
pub mod stdout;

use std::path::{Path, PathBuf};

use crate::search_unused::PackageAnalysis;

/// Which paths are going to be analyzed by machete?
pub enum AnalyzedPaths<'a> {
    /// Start from the current directory, and explore all sub-directories.
    CurrentDir,

    /// A list of paths as specified on the CLI.
    ///
    /// Each path will be explored recursively.
    Paths(&'a [PathBuf]),
}

/// General trait to implement for a printer.
pub trait Printer {
    /// Print the current version of the binary.
    fn print_version(&self, version: &str) -> anyhow::Result<()>;

    /// Print the paths to be analyzed.
    fn print_paths<'a>(&self, paths: AnalyzedPaths<'a>);

    /// Print the results, after they've been analyzed.
    ///
    /// Will be called for any base paths specified in [`AnalyzedPaths`], even for those which don't
    /// have unused dependencies.
    fn print_results<'a>(
        &self,
        path: &Path,
        results: &'a [(PackageAnalysis, &'a PathBuf)],
    ) -> anyhow::Result<()>;

    /// Print the tail of the analysis, usually a "done" message, and the false positive explainer.
    fn print_tail(&self, has_unused_dependencies: bool);
}
