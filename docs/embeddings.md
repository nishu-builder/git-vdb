# Text and embedding models

The core database never downloads a model or calls a network service. Implement
the small `Embedder` trait with a local model or provider client, then bind its
stable model identity to a text collection:

```rust
use git_vdb::{open, Document, Embedder, Result};

#[derive(Clone, Debug)]
struct ExampleEmbedder;

impl Embedder for ExampleEmbedder {
    fn model_id(&self) -> &str { "example/compass@1" }

    fn embed(&self, input: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(input.iter().map(|text| {
            if text.to_lowercase().contains("north") {
                vec![0.0, 1.0]
            } else {
                vec![1.0, 0.0]
            }
        }).collect())
    }
}

# fn main() -> git_vdb::Result<()> {
# let temporary = tempfile::TempDir::new()?;
let db = open(temporary.path().join("documents.git"))?;
let docs = db.text_collection("docs", ExampleEmbedder)?;
docs.upsert_documents([
    Document::new("east", "A document about the east"),
    Document::new("north", "A document about the north"),
])?;
let hits = docs.search_text("north wind", 1)?;
assert_eq!(hits[0].id.to_string(), "north");
# Ok(())
# }
```

Document text is retained under the payload key `document`; application metadata
may use every other key. The model ID is persisted as the collection's vector
space and checked whenever the collection is reopened, so different embedding
models cannot be mixed silently.

## Why there is no bundled default model

The bounded provider spike tested `fastembed 5.17.3` with default features
disabled. Its required `ort 2.0.0-rc.12` and `ort-sys` packages require Rust
1.88, which fails this crate's Rust 1.87 compatibility gate. Bundling its default
features would also introduce model/network and native-runtime behavior into the
default build.

The provider-independent adapter therefore ships without new dependencies. A
local default can be added later when it satisfies the MSRV, platform, offline,
model-identity, and package gates without complicating ordinary vector use.
