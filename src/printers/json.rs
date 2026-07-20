//! A printer that will report the results as JSON.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    printers::{AnalyzedPaths, Printer},
    search_unused::PackageAnalysis,
};

pub struct JsonPrinter;

impl Printer for JsonPrinter {
    fn print_version(&self, version: &str) -> anyhow::Result<()> {
        /// JSON output structure for unused dependencies.
        #[derive(Serialize)]
        struct VersionOutput<'a> {
            /// List of crates with unused dependencies.
            version: &'a str,
        }

        let json_output = VersionOutput { version };

        println!("{}", serde_json::to_string(&json_output)?);
        Ok(())
    }

    fn print_paths<'a>(&self, _paths: AnalyzedPaths<'a>) {
        // Print nothing, Jon Snow.
    }

    fn print_results<'a>(
        &self,
        _path: &Path,
        results: &'a [(PackageAnalysis, &'a PathBuf)],
    ) -> anyhow::Result<()> {
        /// JSON structure for a single crate's unused dependencies.
        #[derive(Serialize)]
        struct CrateUnusedDeps {
            /// The name of the package.
            package_name: String,
            /// Path to the Cargo.toml file.
            manifest_path: String,
            /// List of unused dependency names.
            unused: Vec<String>,
            /// List of dependencies marked as ignored but actually used.
            ignored_used: Vec<String>,
        }

        /// JSON output structure for unused dependencies.
        #[derive(Serialize)]
        struct JsonOutput {
            /// List of crates with unused dependencies.
            crates: Vec<CrateUnusedDeps>,
        }

        if results.is_empty() {
            // Render an empty JSON object.
            println!("{{}}");
            return Ok(());
        }

        let mut json_output = JsonOutput {
            crates: Vec::with_capacity(results.len()),
        };

        // Collect results for JSON output.
        for (analysis, path) in results {
            json_output.crates.push(CrateUnusedDeps {
                package_name: analysis.package_name.clone(),
                manifest_path: path.to_string_lossy().to_string(),
                unused: analysis.unused.clone(),
                ignored_used: analysis.ignored_used.clone(),
            });
        }

        println!("{}", serde_json::to_string(&json_output)?);

        Ok(())
    }

    fn print_tail(&self, _has_unused_dependencies: bool) {
        // Print nothing.
    }
}
