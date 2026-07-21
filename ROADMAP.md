# Roadmap

The roadmap is directional rather than a release promise. Correctness,
determinism, and format compatibility take priority over dates.

## Near term

- Run and publish the complete 1,000/10,000/100,000-point benchmark matrix at
  production-like dimension.
- Instrument Git object reads and report packed storage, recall, candidate work,
  and structural reuse for vector, payload-only, and delete mutations.
- Make mutation CPU and object traversal proportional to changed paths while
  retaining clean-rebuild root equivalence.
- Expand corruption, concurrency, property, transfer, and crash-safety tests.
- Establish an explicit minimum supported Rust version from CI evidence.
- Package reproducible tagged releases and publish API documentation.

## Format evolution

Random-hyperplane LSH remains format version 1's deterministic approximate
index. If the 100,000-point evidence cannot meet the recall/work target, a
history-independent IVF-flat design is the leading format version 2 candidate.
Any new format must be independently versioned, canonical, inspectable, and
readable alongside old immutable roots.

## Out of scope

The project does not plan to add a server, hosted service, authentication,
model inference, workflow adapters, application-specific schemas, or opaque
history-dependent indexes. Those can be built around the embedded library
without entering its canonical storage format.
