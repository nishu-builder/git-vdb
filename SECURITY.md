# Security policy

## Supported versions

Until the first stable release, security fixes are made on `main` and included
in the next `0.1.x` release. After stable releases begin, this table will list
the supported release lines explicitly.

| Version | Supported |
|---|---|
| `main` / latest `0.1.x` | Yes |
| Older revisions | No |

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's
[private vulnerability reporting form](https://github.com/nishu-builder/git-vdb/security/advisories/new).

Include, when possible:

- affected revision or version;
- impact and realistic threat model;
- reproduction steps or a minimal proof of concept;
- whether crafted Git objects, refs, point data, or CLI input are involved;
- any suggested mitigation.

You should receive an acknowledgement within seven days. The maintainers will
investigate, coordinate a fix and disclosure when warranted, and credit the
reporter unless anonymity is requested. Please allow time for a release before
public disclosure.

The most relevant security boundaries are untrusted Git object graphs,
malformed canonical data, resource exhaustion, ref-update races, unsafe path
handling, and dependencies that process repository data. `git-vdb` is an
embedded local database and does not provide authentication or an authorization
boundary.

