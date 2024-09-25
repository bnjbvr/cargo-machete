# 0.7.0 (released on 2024-09-25)

- Breaking change: Don't search in ignored files (those specified in .ignore/.gitignore) by default. It's possible to use `--no-ignore` to search in these directories by default (#137).
- Improved: fix false positives for multi dependencies single use statements (#120). This improves precision at the cost of a small performance hit.
- Improved: make usage of `--with-metadata` more accurate (#122, #132).
- Improved: instead of displaying `.` for the current directory, `cargo-machete` will now display `this directory` (#109).
- Added: There's now an automated docker image build that publishes to the [github repository](https://github.com/bnjbvr/cargo-machete/pkgs/container/cargo-machete) (#121).
- Added: `--ignore` flag which make cargo-machete respect .ignore and .gitignore files when searching for files (#95).

# 0.6.2 (released on 2024-03-24)

- Added: shorter display when scanning the current directory (#109).
- Fix: adapt to latest pkgid specification, so as not to crash with `--with-metadata` (#106).

# 0.6.1 (released on 2024-02-21)

- Chore: bump major dependencies, to fix parsing issues of Cargo.toml files (#101, #105).

# 0.6.0 (released on 2023-09-23)

- *Breaking*/improved: match against crate name case-insensitive (#69).
- Added: Github action (#85). See README for documentation.
- Added: support for ignored workspace dependencies (#57, #86). See README for documentation.
- Added: `--version` switch to print the version (#66).
- Fix: avoid searching for workspace Cargo.toml longer than needed (#84).
- Chore: better documentation and reporting (#63, #72, #80).

# 0.5.0 (released on 2022-11-15)

- *Breaking*: Use `argh` for parsing. Now, paths of directories to scan must be passed in the last
  position, when running from the command line (#51).
- Fix rare false positive and speed up most common case (#53).
- Fix loading properties from workspace (#54).

# 0.4.0 (released on 2022-10-16)

- Added `--skip-target-dir` to not analyze `target/` directories.
- Added a message indicating of any unused dependencies were found or not.
- Support for workspace properties

# 0.3.1 (released on 2022-06-12)

- Support empty global prefix, e.g. `use ::log;`.

# 0.3.0 (released on 2022-05-09)

- Use exit code to signal if there are unused dependencies:
    - 0: when no unused dependencies are found
    - 1: when at least one unused (non-ignored) dependency is found
    - 2: on error
- Preserve Cargo.toml format when automatically removing dependencies
- Warn if any dependency marked as ignored is actually used

# 0.2.0 (released on 2022-04-26)

Initial public version.
