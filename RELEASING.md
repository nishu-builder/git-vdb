# Releasing git-vdb

Crates.io releases are permanent. A version cannot be overwritten or deleted;
it can only be yanked. Publish only from a reviewed, clean `main` commit whose
package contents and generated documentation have been inspected.

## Prepare

1. Update `Cargo.toml` and `CHANGELOG.md` to the same Semantic Version.
2. Keep persisted format-version changes separate and update `docs/format.md`
   only when the compatibility and migration policy has been approved.
3. Run the complete release gates:

   ```sh
   nix flake check --print-build-logs
   cargo +1.87.0 check --lib --locked
   cargo +1.87.0 test --doc --locked
   cargo package --list
   cargo publish --dry-run --locked
   ```

4. Inspect `target/package/git-vdb-VERSION.crate` and verify that it contains
   only the intended library, CLI, public examples/tests, specifications, and
   project metadata.
5. Push the release commit and wait for CI on `main` to pass.

## Publish

Authenticate with a scoped crates.io token using `cargo login`, then publish:

```sh
cargo publish --locked
```

Confirm that the crate page and docs.rs build are healthy before tagging. Then
create and push an annotated tag matching the crate version:

```sh
git tag -a vVERSION -m "git-vdb VERSION"
git push origin vVERSION
```

The tag workflow creates a GitHub release and uploads native Linux, macOS, and
Windows CLI archives with SHA-256 checksum files. Verify the release artifacts,
installation with `cargo install git-vdb --version VERSION`, and the versioned
docs.rs URL.
