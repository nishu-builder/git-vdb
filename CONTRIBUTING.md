# Contributing to git-vdb

Thank you for helping improve `git-vdb`. Contributions of code, tests,
documentation, benchmarks, bug reports, and design feedback are welcome.

By participating, you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).
Security vulnerabilities must be reported privately as described in
[SECURITY.md](SECURITY.md), not through a public issue.

## Before opening a change

For a bug fix or documentation correction, a pull request can be the first
discussion. For new public API, CLI, storage-format, or index behavior, open a
feature issue first. Early agreement matters because collection roots are a
versioned persistence format, not an internal implementation detail.

Keep changes within the project scope: an embedded Git-native vector database.
Servers, model inference, workflow adapters, authentication, and
application-specific concepts belong elsewhere.

## Development setup

Install stable Rust and Git. Building `git2` also requires the native build
tools expected by `libgit2-sys` on your platform.

```sh
git clone https://github.com/nishu-builder/git-vdb.git
cd git-vdb
cargo build
cargo test
```

Before submitting a pull request, run:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --all-features --no-deps
cargo deny check
```

CI runs the Rust checks on Linux, macOS, and Windows. `cargo deny` is optional
locally if it is not installed; its GitHub check is required before merge.

## Tests and determinism

Every behavior change should have focused tests. Storage and indexing changes
must also demonstrate the relevant invariants:

- roots do not depend on input order, repository path, history, or bare versus
  non-bare layout;
- snapshot operations do not create commits or refs, resolve symbolic names, or
  require repository history;
- materializing and reopening a snapshot preserves its exact root object ID;
- incremental writes equal clean rebuilds of the resulting point set;
- exact search agrees with a simple cosine oracle and uses canonical tie order;
- stale writers cannot replace a collection head;
- historical roots remain readable;
- approximate work stays within explicit probe and candidate limits;
- reads do not modify objects or refs;
- malformed input cannot advance a collection ref.

Use fixed seeds for randomized or property tests and include the seed in failure
output. Do not commit generated Git databases or benchmark output.

## Format and performance changes

The bytes documented in `docs/format.md` define format version 1. A change that
would alter an existing root's meaning or object ID requires explicit design
discussion, a new format version, compatibility tests, and migration/read
semantics. Never silently reinterpret an old root.

Performance claims need a reproducible dataset seed, complete index/query
parameters, hardware, revision, exact-search baseline, recall, work fraction,
and storage measurements. Record meaningful findings in `docs/findings.md`,
including negative results.

## Pull requests

Keep each pull request focused. In its description, explain:

- the problem and user-visible behavior;
- the approach and important tradeoffs;
- any public API, CLI, or format implications;
- the tests and benchmarks run;
- remaining limitations or follow-up work.

Update `CHANGELOG.md` under `Unreleased` for user-visible changes. Avoid
drive-by dependency or formatting churn unrelated to the change.

Contributions are accepted under Apache License 2.0 section 5. No separate
contributor license agreement is required.
