# Text and embedding models

The core database stores vectors and never downloads a model or calls a network
service. This keeps the default crate small, offline, and reproducible.

Whichever embedding function an application uses, assign a stable model identity
to explicitly configured collections with `CollectionConfig::with_vector_space`.
Detailed queries can require the same identity with `Query::in_vector_space`,
preventing vectors from different models from being mixed silently.

An optional first-party text adapter is specified in
[`specs/0001-simple-embedded-api.md`](specs/0001-simple-embedded-api.md). Until
that rung is complete, applications should embed text before constructing a
`Point` and retain the original text in point metadata.
