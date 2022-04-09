<div align="center">
  <h1><code>cargo-machete</code></h1>

  <p>
    <strong>Remove unused Rust dependencies with this one weird trick!</strong>
  </p>

  <p>
    <a href="https://github.com/bnjbvr/cargo-machete/actions?query=workflow%3ARust"><img src="https://github.com/bnjbvr/cargo-machete/workflows/Rust/badge.svg" alt="build status" /></a>
    <a href="https://matrix.to/#/#cargo-machete:delire.party"><img src="https://img.shields.io/badge/matrix-join_chat-brightgreen.svg" alt="matrix chat" /></a>
    <img src="https://img.shields.io/badge/rustc-stable+-green.svg" alt="supported rustc stable" />
  </p>
</div>

## Writeup coming soon 🔜

## Installation

Install `cargo-machete` with cargo:

`cargo install cargo-machete`

## Example

Run cargo-machete in a directory that contains one or more Rust projects (using Cargo for
dependency management):

```bash
cd my-directory && cargo machete

# alternatively

cargo machete /absolute/path/to/my/directory
```

If there are too many false positives, consider using the `--with-metadata` CLI
flag, which will call `cargo metadata --all-features` to find final dependency
names, more accurate dependencies per build type, etc. ⚠ This may modify the
`Cargo.lock` files in your projects.

## Contributing

[Contributor Covenant](https://img.shields.io/badge/contributor%20covenant-v1.4-ff69b4.svg)

We welcome community contributions to this project.

## License

[MIT license](LICENSE.md).
