## Goals of this process

- To not forget any step in the release process.
- To make sure the Github actions are properly tagged, and the Github action on the main branch
  keeps on using the main branch's code.

## Release process

- Bump the version in the `Cargo.toml` file.
- Compile, to make sure the `Cargo.lock` file is updated.
- Hardcode the new fixed version of the binary in the Github action.
- Commit the changes and open a PR.
- Once the PR passes CI, merge it.

### Create the Github release

- Tag the commit with the version number and push.
- Github will prepare a Github release, based on this, with the built binary artifacts.
- A maintainer must then publish the artifacts once they're built, adding the release notes if
  needs be, etc.

### Create the crates.io release

- `cargo publish`, with or without `--dry-run`.
