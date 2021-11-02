# the setup

- lib1 exports a function
- lib2 contains a dependency to lib1, and calls into lib1's method
- binary contains a dependency to lib2, and uses it
- binary contains an unused dependency to lib1 (it's not used directly, but
  it's referenced in the Cargo.toml file)

# output of cargo-udeps

```
➜ cargo +nightly udeps
    Checking lib1 v0.1.0 (/tmp/benjamin/lib1)
    Checking lib2 v0.1.0 (/tmp/benjamin/lib2)
    Checking udeps-test-case v0.1.0 (/tmp/benjamin)
    Finished dev [unoptimized + debuginfo] target(s) in 0.76s
info: Loading save analysis from "/tmp/benjamin/target/debug/deps/save-analysis/udeps_test_case-1e86a6794399c1a0.json"
All deps seem to have been used.
```
