//! A printer that will report the results as JSON.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    printers::{AnalyzedPaths, Printer},
    search_unused::{PackageAnalysis, WorkspaceAnalysis},
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
        workspaces: &'a [WorkspaceAnalysis],
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

        /// JSON structure for a single workspace's unused shared dependencies.
        #[derive(Serialize)]
        struct WorkspaceUnusedDeps {
            /// Path to the workspace root's Cargo.toml file.
            manifest_path: String,
            /// List of `[workspace.dependencies]` entries no member inherits.
            unused: Vec<String>,
        }

        /// JSON output structure for unused dependencies.
        #[derive(Serialize)]
        struct JsonOutput {
            /// List of crates with unused dependencies.
            crates: Vec<CrateUnusedDeps>,
            /// List of workspaces with unused shared dependencies.
            workspaces: Vec<WorkspaceUnusedDeps>,
        }

        if results.is_empty() && workspaces.is_empty() {
            // Render an empty JSON object.
            println!("{{}}");
            return Ok(());
        }

        let mut json_output = JsonOutput {
            crates: Vec::with_capacity(results.len()),
            workspaces: Vec::with_capacity(workspaces.len()),
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

        for analysis in workspaces {
            json_output.workspaces.push(WorkspaceUnusedDeps {
                manifest_path: analysis.manifest_path.to_string_lossy().to_string(),
                unused: analysis.unused.clone(),
            });
        }

        println!("{}", serde_json::to_string(&json_output)?);

        Ok(())
    }

    fn print_tail(&self, _has_unused_dependencies: bool) {
        // Print nothing.
    }
}
