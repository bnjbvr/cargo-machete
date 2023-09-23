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
