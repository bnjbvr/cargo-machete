//! A printer that will print everything to stdout.
//!
//! Errors will be reported to stderr.

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use crate::{
    printers::{AnalyzedPaths, Printer},
    search_unused::PackageAnalysis,
};

pub struct StdoutPrinter {
    pub quiet: bool,
    pub with_metadata: bool,
}

impl Printer for StdoutPrinter {
    fn print_version(&self, version: &str) -> anyhow::Result<()> {
        // Print even in quiet mode.
        println!("{}", version);
        Ok(())
    }

    fn print_paths<'a>(&self, paths: AnalyzedPaths<'a>) {
        if self.quiet {
            // Skip printing the paths in quiet mode.
            return;
        }

        match paths {
            AnalyzedPaths::CurrentDir => {
                println!("Analyzing dependencies of crates in this directory...");
            }
            AnalyzedPaths::Paths(path_bufs) => {
                println!(
                    "Analyzing dependencies of crates in {}...",
                    path_bufs
                        .iter()
                        .map(|path| path.as_os_str().to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }

    fn print_results<'a>(
        &self,
        path: &Path,
        results: &'a [(PackageAnalysis, &'a PathBuf)],
    ) -> anyhow::Result<()> {
        let location = match path.to_string_lossy() {
            Cow::Borrowed(".") => Cow::from("this directory"),
            pathstr => pathstr,
        };

        if results.is_empty() {
            if !self.quiet {
                println!(
                    "cargo-machete didn't find any unused dependencies in {location}. Good job!"
                );
            }
            return Ok(());
        }

        println!("cargo-machete found the following unused dependencies in {location}:");
        for (analysis, path) in results {
            println!("{} -- {}:", analysis.package_name, path.to_string_lossy());
            for dep in &analysis.unused {
                println!("\t{dep}");
            }

            for dep in &analysis.ignored_used {
                println!("\t⚠️  {dep} was marked as ignored, but is actually used!");
            }
        }

        println!();

        Ok(())
    }

    fn print_tail(&self, has_unused_dependencies: bool) {
        if has_unused_dependencies {
            println!(
                r#"If you believe cargo-machete has detected an unused dependency incorrectly, you can add the dependency to the list of dependencies to ignore in the `[package.metadata.cargo-machete]` section of the appropriate Cargo.toml.

For example:

[package.metadata.cargo-machete]
ignored = ["prost"]
"#
            );

            if !self.with_metadata {
                println!(
                    "You can also try running it with the `--with-metadata` flag for better accuracy, though this may modify your Cargo.lock files."
                );
            }
        }

        if !self.quiet {
            println!("Done!");
        }
    }
}
