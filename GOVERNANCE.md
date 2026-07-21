# Governance

`git-vdb` is currently maintained by [@nishu-builder](https://github.com/nishu-builder).
The maintainer reviews contributions, manages releases and security responses,
and is responsible for preserving the project's technical scope and format
compatibility.

## Decision making

Small fixes are decided through normal pull-request review. Significant public
API, CLI, index, or storage-format proposals begin as GitHub issues and should
record the problem, alternatives, compatibility effects, and evidence. The
maintainer seeks rough consensus while retaining final responsibility for a
coherent format and release.

Decisions favor:

1. deterministic and inspectable behavior;
2. compatibility with ordinary Git tooling;
3. correctness and visible work bounds over opaque performance claims;
4. a conventional embedded vector-database interface;
5. a small, offline, auditable dependency and feature surface.

Material decisions should be captured in issues, pull requests, format
documentation, or findings rather than existing only in private conversation.

## Maintainers

Additional maintainers may be invited after sustained, high-quality technical
and community contributions. Maintainers are expected to review impartially,
follow the Code of Conduct, disclose relevant conflicts, and recuse themselves
when necessary.

If the project grows beyond a single-maintainer model, this document will be
updated before changing decision or release authority.

