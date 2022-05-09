# 0.3.0 (released on 2022-05-09)

- Use exit code to signal if there are unused dependencies:
    - 0: when no unused dependencies are found
    - 1: when at least one unused (non-ignored) dependency is found
    - 2: on error
- Preserve Cargo.toml format when automatically removing dependencies
- Warn if any dependency marked as ignored is actually used

# 0.2.0 (released on 2022-04-26)

Initial public version.
