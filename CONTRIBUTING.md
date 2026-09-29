# Contributing to etude

Thanks for your interest in contributing! etude is a workspace of small, focused Rust
crates. This guide covers the local checks and conventions the project expects.

## Prerequisites

- A stable Rust toolchain. The workspace uses **edition 2024** with a minimum supported
  Rust version (**MSRV**) of **1.88**.
- The crates are also built for WebAssembly, so install the wasm targets:

  ```sh
  rustup target add wasm32-unknown-unknown wasm32-wasip1
  rustup component add rustfmt clippy
  ```

## Before opening a pull request

Run the same checks the release gate enforces (a merge to `main` re-runs these before any
crates.io publish, so a green local run keeps the pipeline green):

```sh
# formatting
cargo fmt --all --check

# lints (warnings are denied)
cargo clippy --workspace --all-targets --all-features -- -D warnings

# every shipping crate must compile to wasm (the crates run as WebAssembly guests)
cargo check --workspace --lib --target wasm32-unknown-unknown
cargo check --workspace --lib --no-default-features --target wasm32-unknown-unknown
cargo check --workspace --lib --target wasm32-wasip1

# tests, with and without default features
cargo test --workspace --all-features
cargo test --workspace
```

## Commit messages

Commits follow the [Conventional Commits](https://www.conventionalcommits.org) format —
this is load-bearing, not cosmetic: [release-plz](https://release-plz.dev) derives each
crate's version bump and changelog from the commit history.

```
<type>(<scope>): <summary>
```

- Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `perf`, `chore`, `ci`.
- `<scope>` is usually the crate short name (e.g. `bigint`, `bytevec`, `json`) or `crates`
  for a workspace-wide change.
- A `feat:` triggers a minor bump; a `fix:` a patch bump; a `!` (or `BREAKING CHANGE:`
  footer) a breaking bump. Versions are **independent per crate** — a change to one crate
  releases only it (plus any semver-required dependents), not the whole workspace.

Examples:

```
fix(bigint): normalize the sign of a zero magnitude
docs(crates): add per-crate READMEs for crates.io
```

## Releases

Releases are fully automated. On a push to `main`, release-plz keeps a version-bump +
changelog "release PR" open; merging it publishes the newly-versioned crates to crates.io
(via GitHub OIDC Trusted Publishing), tags them, and creates the GitHub Releases. You do
not publish or tag by hand.

## Workspace layout

Each crate lives in `crates/<name>/`; shared package metadata (edition, MSRV, license,
repository) is inherited from `[workspace.package]` in the root `Cargo.toml`.

## License

By contributing, you agree that your contributions are licensed under the
[Apache License, Version 2.0](LICENSE).
