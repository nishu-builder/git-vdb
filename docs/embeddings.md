# Text and embedding models

For the shortest fully local path, enable the optional FastEmbed integration:

```sh
cargo add git-vdb --features fastembed
```

```rust,ignore
use git_vdb::{open, Document, FastEmbedder, TextQuery};

fn main() -> git_vdb::Result<()> {
    let db = open("./documents.git")?;
    let docs = db.text_collection("docs", FastEmbedder::try_new()?)?;
    docs.upsert_documents([Document::new("guide", "Git-native vector search")])?;
    let hits = docs.query(TextQuery::new("versioned search").limit(5))?;
    println!("{}", hits[0].document);
    Ok(())
}
```

The model downloads on first initialization, is cached in the platform user
cache for offline use, and is serialized behind the collection handle so
concurrent calls remain safe. Set `FASTEMBED_CACHE_DIR` to override the cache
location. The
feature is disabled by default, so vector-only builds never include FastEmbed,
model downloads, or an ONNX runtime. The same path is compiled as
`examples/text_fastembed.rs` whenever the feature is enabled.

## Custom providers

The core database never downloads a model or calls a network service. Implement
the small `Embedder` trait with a local model or provider client, then bind its
stable model identity to a text collection:

```rust,no_run
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

fn main() -> git_vdb::Result<()> {
    let db = open("./documents.git")?;
    let docs = db.text_collection("docs", ExampleEmbedder)?;
    docs.upsert_documents([
        Document::new("east", "A document about the east"),
        Document::new("north", "A document about the north"),
    ])?;
    let hits = docs.search_text("north wind", 1)?;
    assert_eq!(hits[0].id.to_string(), "north");
    Ok(())
}
```

Document text is retained under the payload key `document`; application metadata
may use every other key. The model ID is persisted as the collection's vector
space and checked whenever the collection is reopened, so different embedding
models cannot be mixed silently.

## Provider decision

The original provider spike stopped because FastEmbed's ONNX dependencies
required newer Rust than the crate's former Rust 1.87 floor. With the toolchain
now pinned to Rust 1.97.1, the integration passes that gate. It remains an
explicit feature so the default vector database stays small and network-free.

The persisted model space includes the FastEmbed model variant and adapter
version. To use a different supported model, pass a `FastEmbedModel` to
`FastEmbedder::try_with_model`.
