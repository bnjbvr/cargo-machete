use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, bail};

pub struct ResolveSkipPaths<'a> {
    /// Whether or not to skip the target directory.
    pub skip_target_dir: bool,

    /// Patterns to skip in directories.
    pub skip_patterns: &'a mut Vec<PathBuf>,

    /// Absolute or relative directories to skip.
    pub skip: &'a mut Vec<PathBuf>,
}

pub fn resolve_skip_paths(cwd: &Path, args: ResolveSkipPaths<'_>) -> anyhow::Result<()> {
    // Handle skip patterns.
    for path in &*args.skip_patterns {
        if path.is_empty() {
            // This would lead to skipping everything, so let's make it a hard error instead.
            bail!("empty skip pattern");
        }

        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        {
            bail!(
                "passing an absolute or relative directory is not allowed with `--skip-patterns`, use `--skip` instead"
            );
        }
    }

    // Add a skip pattern if we're skipping the target dir.
    if args.skip_target_dir && !args.skip_patterns.iter().any(|p| p.as_os_str() == "target") {
        args.skip_patterns.push(PathBuf::from("target"));
    }

    // Canonicalize exact skip paths.
    {
        for raw in args.skip {
            let abs_path = fs::canonicalize(cwd.join(&raw))
                .with_context(|| format!("invalid --skip path {}", raw.display()))?;
            *raw = abs_path;
        }
    }

    Ok(())
}

pub struct CollectPathOptions {
    /// Patterns to skip in directories.
    pub skip_patterns: Vec<PathBuf>,

    /// Canonicalized, absolute directories to skip.
    pub exact_skip_paths: Vec<PathBuf>,

    /// Should we ignore files as specified in .gitignore (in the target directory, or any parent),
    /// and `.ignore`?
    pub respect_ignore_files: bool,

    // As an override to the above `respect_ignore_files`, should we use `.gitignore` overall?
    //
    // This is used only in testing, to avoid reading this repository's `.gitignore` file for
    // testing the `collect_paths()` function.
    pub override_respect_git_ignore: Option<bool>,
}

pub fn collect_paths(path: &Path, options: CollectPathOptions) -> anyhow::Result<Vec<PathBuf>> {
    // Find directory entries.
    let mut builder = ignore::WalkBuilder::new(path);

    builder.standard_filters(options.respect_ignore_files);

    if let Some(val) = options.override_respect_git_ignore {
        builder.git_ignore(val);
    }

    let exact_skip_paths = options.exact_skip_paths;
    let skip_patterns = options.skip_patterns;

    // Exact skip paths are matched exactly, so entries must be compared through the canonical form
    // of the walk root. Entries are always `path.join(relative)`, so we only canonicalize the root
    // once instead of every entry.
    let canonical_root = if !exact_skip_paths.is_empty() {
        Some(
            fs::canonicalize(path).map_err(|err| ignore::Error::WithPath {
                path: path.to_path_buf(),
                err: Box::new(ignore::Error::Io(err)),
            })?,
        )
    } else {
        None
    };

    // If the walk root is a skipped directory, or lies anywhere under one, everything it contains
    // is skipped. `Path::starts_with` compares whole components, so skipping `foo` doesn't affect
    // a root at `foobar`.
    if let Some(canonical_root) = &canonical_root
        && exact_skip_paths
            .iter()
            .any(|skip| canonical_root.starts_with(skip))
    {
        bail!("found no Cargo.toml in {}", path.display());
    }

    if !skip_patterns.is_empty() || !exact_skip_paths.is_empty() {
        // Entries are built from the walk root as given (possibly non-canonical, e.g. `src/..`),
        // so the relative part must be stripped with it, not with the canonical root.
        let walk_root = path.to_path_buf();
        builder.filter_entry(move |entry| {
            let entry_path = entry.path();

            for last_component in &skip_patterns {
                if entry_path.ends_with(last_component) {
                    return false;
                }
            }

            if let Some(canonical_root) = &canonical_root {
                let rel = entry_path.strip_prefix(&walk_root).unwrap_or(entry_path);
                let canonical_entry = canonical_root.join(rel);
                // The root itself isn't under any skipped path (checked above), so prune the exact
                // matching child directories.
                if exact_skip_paths.contains(&canonical_entry) {
                    return false;
                }
            }

            true
        });
    }

    let walker = builder.build();

    // Keep only errors and `Cargo.toml` files (filter), then map correct paths into owned
    // `PathBuf`.
    let entries = walker
        .into_iter()
        .filter(|entry| {
            entry
                .as_ref()
                .map_or(true, |entry| entry.file_name() == "Cargo.toml")
        })
        .map(|res_entry| res_entry.map(|e| e.into_path()))
        .collect::<Result<Vec<_>, _>>()?;

    if entries.is_empty() {
        bail!("found no Cargo.toml in {}", path.display());
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use crate::TOP_LEVEL;

    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_skip_exact_paths() {
        let fixture = PathBuf::from(TOP_LEVEL).join("integration-tests/skip-sub-dir");

        // Collects manifests under `root`, with `--skip` value.
        let collect = |root: &Path, cwd: &Path, skip_paths_str: &[&str]| {
            let mut skip = skip_paths_str.into_iter().map(PathBuf::from).collect();

            resolve_skip_paths(
                cwd,
                ResolveSkipPaths {
                    skip_target_dir: false,
                    skip_patterns: &mut Vec::new(),
                    skip: &mut skip,
                },
            )
            .unwrap();

            collect_paths(
                root,
                CollectPathOptions {
                    skip_patterns: Vec::new(),
                    exact_skip_paths: skip,
                    respect_ignore_files: false,
                    override_respect_git_ignore: Some(false),
                },
            )
        };
        let has_skipped =
            |entries: &[PathBuf]| entries.iter().any(|p| p.ends_with("skipped/Cargo.toml"));

        // Without any skip patterns, both the top-level and the nested `skipped` manifest are collected.
        {
            let entries = collect(&fixture, &fixture, &[]).unwrap();
            assert_eq!(entries.len(), 2);
            assert!(has_skipped(&entries));
        }

        // Exact paths are correctly matched.
        {
            let entries = collect(&fixture, &fixture, &["./skipped"]).unwrap();
            assert_eq!(entries.len(), 1);
            assert!(!has_skipped(&entries));
        }

        // `--skip .` skips the current directory, including children, so this leads to no collected
        // Cargo.toml, hence an error.
        {
            let error = collect(&fixture, &fixture, &["."]).unwrap_err();
            assert!(error.to_string().contains("found no Cargo.toml"));
        }

        // `--skip .` matches all directories when run from a source sub-dir; `.` is resolved to the
        // current working directory.
        {
            let cwd = fixture.join("src");
            let entries = collect(&fixture, &cwd, &["."]).unwrap();
            assert_eq!(entries.len(), 2);
            assert!(has_skipped(&entries));
        }

        // `--skip .` matches some directories when run from an actual Rust package; `.` is resolved to
        // the current working directory.
        {
            let cwd = fixture.join("skipped");
            let entries = collect(&fixture, &cwd, &["."]).unwrap();
            assert_eq!(entries.len(), 1);
            assert!(!has_skipped(&entries));
        }

        // Skipped paths are resolved against the current working directory, not the root.
        // This is running from `skip-sub-dir/src`, `cargo machete --skip ../skipped ..`.
        {
            let cwd = fixture.join("src");
            let entries = collect(&cwd.join(".."), &cwd, &["../skipped"]).unwrap();
            assert_eq!(entries.len(), 1);
            assert!(!has_skipped(&entries));
        }

        // Walking a directory that lies under a skipped one skips it entirely.
        // This is running from `skip-sub-dir/skipped`, `cargo machete --skip .. .`.
        {
            let cwd = fixture.join("skipped");
            let error = collect(&cwd, &cwd, &[".."]).unwrap_err();
            assert!(error.to_string().contains("found no Cargo.toml"));
        }

        // Same, with a walk root nested deeper than the skipped directory, and given in a
        // non-canonical form: running from `skip-sub-dir`, `cargo machete --skip . skipped/src/..`.
        {
            let root = fixture.join("skipped/src/..");
            let error = collect(&root, &fixture, &["."]).unwrap_err();
            assert!(error.to_string().contains("found no Cargo.toml"));
        }
    }

    #[test]
    fn test_skip_patterns() {
        let fixture = PathBuf::from(TOP_LEVEL).join("integration-tests/skip-sub-dir");

        // Collects manifests under `root`, with `--skip-patterns` value.
        let collect = |root: &Path, skip_patterns_str: &[&str]| -> Vec<PathBuf> {
            let mut skip_patterns = skip_patterns_str.into_iter().map(PathBuf::from).collect();

            resolve_skip_paths(
                root,
                ResolveSkipPaths {
                    skip_target_dir: false,
                    skip_patterns: &mut skip_patterns,
                    skip: &mut Vec::new(),
                },
            )
            .unwrap();

            collect_paths(
                root,
                CollectPathOptions {
                    skip_patterns,
                    exact_skip_paths: Vec::new(),
                    respect_ignore_files: false,
                    override_respect_git_ignore: Some(false),
                },
            )
            .unwrap()
        };
        let has_skipped =
            |entries: &[PathBuf]| entries.iter().any(|p| p.ends_with("skipped/Cargo.toml"));

        // Without any skip patterns, both the top-level and the nested `skipped` manifest are collected.
        {
            let entries = collect(&fixture, &[]);
            assert_eq!(entries.len(), 2);
            assert!(has_skipped(&entries));
        }

        // Plain relative paths match on trailing components.
        for skip in ["skipped", "skipped/", "skip-sub-dir/skipped"] {
            let entries = collect(&fixture, &[skip]);
            assert_eq!(entries.len(), 1, "--skip-patterns {skip}");
            assert!(!has_skipped(&entries), "--skip-patterns {skip}");
        }
    }

    #[test]
    fn test_invalid_skip_patterns() {
        let run = |skip_patterns: &[&str]| {
            let mut skip_patterns = skip_patterns.into_iter().map(PathBuf::from).collect();
            let cwd = PathBuf::from("unused");
            resolve_skip_paths(
                &cwd,
                ResolveSkipPaths {
                    skip_target_dir: false,
                    skip_patterns: &mut skip_patterns,
                    skip: &mut Vec::new(),
                },
            )
        };

        let err = run(&[""]).unwrap_err();
        assert!(err.to_string().contains("empty"));

        for path in &["./current", "../parent", "/tmp"] {
            let err = run(&[path]).unwrap_err();
            assert!(err.to_string().contains("absolute or relative directory"));
        }
    }

    #[test]
    fn test_no_cargo_toml_error() {
        let entries = collect_paths(
            &PathBuf::from(TOP_LEVEL).join("./integration-tests/empty-dir/"),
            CollectPathOptions {
                skip_patterns: Vec::new(),
                exact_skip_paths: Vec::new(),
                respect_ignore_files: false,
                override_respect_git_ignore: Some(false),
            },
        );
        let err = entries.unwrap_err();
        assert!(err.to_string().contains("found no Cargo.toml"));
    }

    #[test]
    fn test_ignore_target() {
        let mut skip_patterns = Vec::new();
        let mut exact_skip_paths = Vec::new();
        resolve_skip_paths(
            &PathBuf::from("."),
            ResolveSkipPaths {
                skip_target_dir: true,
                skip_patterns: &mut skip_patterns,
                skip: &mut exact_skip_paths,
            },
        )
        .unwrap();

        let entries = collect_paths(
            &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
            CollectPathOptions {
                skip_patterns,
                exact_skip_paths,
                respect_ignore_files: false,
                override_respect_git_ignore: Some(false),
            },
        );
        assert!(entries.is_err());

        let entries = collect_paths(
            &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
            CollectPathOptions {
                skip_patterns: Vec::new(),
                exact_skip_paths: Vec::new(),
                respect_ignore_files: true,
                override_respect_git_ignore: Some(false),
            },
        );
        assert!(entries.is_err());

        let entries = collect_paths(
            &PathBuf::from(TOP_LEVEL).join("./integration-tests/with-target/"),
            CollectPathOptions {
                skip_patterns: Vec::new(),
                exact_skip_paths: Vec::new(),
                respect_ignore_files: false,
                override_respect_git_ignore: Some(false),
            },
        );
        assert!(!entries.unwrap().is_empty());
    }
}
