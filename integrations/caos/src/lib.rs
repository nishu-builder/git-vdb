//! Content-only ingestion and source provenance for the optional Caos adapter.

use git_vdb::{CollectionConfig, Point, Query, Snapshot, SnapshotEngine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const DEFAULT_CHUNK_CHARS: usize = 1200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub text: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddedChunk {
    #[serde(flatten)]
    pub chunk: Chunk,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileLocation {
    pub path: String,
    pub blob: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub source_tree: String,
    pub files: Vec<FileLocation>,
    pub skipped: Vec<String>,
    pub model: String,
    pub dimension: usize,
    pub chunk_chars: usize,
}

/// Keep exact UTF-8 source slices, with inclusive one-based line spans.
/// A long line may span several chunks; no whitespace is rewritten.
pub fn chunks(text: &str, maximum_chars: usize) -> Result<Vec<Chunk>> {
    if maximum_chars == 0 || maximum_chars > 16384 {
        return Err("chunk-chars must be between 1 and 16384".into());
    }
    let mut result = Vec::new();
    let mut start = 0;
    let mut start_line = 1;
    let mut line = 1;
    let mut count = 0;
    let mut end_line = 1;
    for (offset, character) in text.char_indices() {
        end_line = line;
        count += 1;
        if character == '\n' {
            line += 1;
        }
        let end = offset + character.len_utf8();
        if count >= maximum_chars || (character == '\n' && line - start_line >= 24) {
            let slice = &text[start..end];
            if !slice.trim().is_empty() {
                result.push(Chunk {
                    text: slice.into(),
                    start_line,
                    end_line,
                });
            }
            start = end;
            start_line = line;
            count = 0;
        }
    }
    if start < text.len() && !text[start..].trim().is_empty() {
        result.push(Chunk {
            text: text[start..].into(),
            start_line,
            end_line,
        });
    }
    Ok(result)
}

pub fn excluded_directory(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".caos"
            | ".git-vdb"
            | ".git-vdb-index"
            | "target"
            | "node_modules"
            | "DEEP-DEPS"
            | "caos-std"
            | "__pycache__"
            | ".venv"
            | "vendor"
            | "dist"
            | "build"
    )
}

pub fn selected_file(path: &str) -> bool {
    let path = Path::new(path);
    let name = path.file_name().and_then(|x| x.to_str()).unwrap_or("");
    matches!(name, "Dockerfile" | "Makefile" | "LICENSE" | "DEPS")
        || path
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension,
                    "rs" | "py"
                        | "go"
                        | "js"
                        | "jsx"
                        | "ts"
                        | "tsx"
                        | "c"
                        | "h"
                        | "cc"
                        | "cpp"
                        | "java"
                        | "rb"
                        | "sh"
                        | "nix"
                        | "md"
                        | "txt"
                        | "toml"
                        | "yaml"
                        | "yml"
                        | "json"
                        | "sql"
                        | "html"
                        | "css"
                )
            })
}

/// Inference is supplied by the image. Tests can supply a fixture executable.
pub fn embed(texts: &[String]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let executable = std::env::var("GIT_VDB_EMBED").unwrap_or_else(|_| "/embed".into());
    let mut process = Command::new(executable)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    // File and chunk bounds keep this request small. Drain stdout after closing stdin.
    process
        .stdin
        .take()
        .ok_or("no embedder stdin")?
        .write_all(&serde_json::to_vec(texts)?)?;
    let output = process.wait_with_output()?;
    if !output.status.success() {
        return Err(format!("embedding process failed: {}", output.status).into());
    }
    let vectors: Vec<Vec<f32>> = serde_json::from_slice(&output.stdout)?;
    if vectors.len() != texts.len()
        || vectors
            .iter()
            .any(|v| v.is_empty() || v.iter().any(|x| !x.is_finite()))
    {
        return Err("embedding process returned invalid vectors".into());
    }
    Ok(vectors)
}

pub fn embedding_identity() -> Result<String> {
    let executable = std::env::var("GIT_VDB_EMBED").unwrap_or_else(|_| "/embed".into());
    let output = Command::new(executable).arg("--identity").output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let identity = String::from_utf8(output.stdout)?.trim().to_owned();
    if identity.is_empty() {
        return Err("empty embedding identity".into());
    }
    Ok(identity)
}

pub fn embedded_chunks(text: &str, chunk_chars: usize) -> Result<Vec<EmbeddedChunk>> {
    let chunks = chunks(text, chunk_chars)?;
    let mut result = Vec::with_capacity(chunks.len());
    // Fixed batches make per-file identity independent of the containing corpus.
    for batch in chunks.chunks(32) {
        let input = batch.iter().map(|x| x.text.clone()).collect::<Vec<_>>();
        let vectors = embed(&input)?;
        result.extend(
            batch
                .iter()
                .cloned()
                .zip(vectors)
                .map(|(chunk, vector)| EmbeddedChunk { chunk, vector }),
        );
    }
    Ok(result)
}

pub fn points(
    manifest: &Manifest,
    embedded: &BTreeMap<String, Vec<EmbeddedChunk>>,
) -> Result<Vec<Point>> {
    let mut points = Vec::new();
    for file in &manifest.files {
        for (index, chunk) in embedded
            .get(&file.blob)
            .ok_or("missing file embeddings")?
            .iter()
            .enumerate()
        {
            let id = hex::encode(Sha256::digest(format!("{}\0{index}", file.path).as_bytes()));
            points.push(
                Point::new(id, chunk.vector.clone()).with_metadata(serde_json::json!({
                    "document": chunk.chunk.text,
                    "path": file.path,
                    "blob": file.blob,
                    "start_line": chunk.chunk.start_line,
                    "end_line": chunk.chunk.end_line,
                }))?,
            );
        }
    }
    Ok(points)
}

pub fn build_directory(
    output: &Path,
    manifest: &Manifest,
    embedded: &BTreeMap<String, Vec<EmbeddedChunk>>,
) -> Result<String> {
    std::fs::create_dir_all(output)?;
    let engine = SnapshotEngine::ephemeral()?;
    let snapshot = engine.build(
        CollectionConfig::new(manifest.dimension).with_vector_space(&manifest.model),
        points(manifest, embedded)?,
    )?;
    snapshot.materialize(output.join("index"))?;
    std::fs::write(output.join("manifest.json"), serde_json::to_vec(manifest)?)?;
    std::fs::write(
        output.join("report"),
        format!(
            "Indexed {} files into snapshot {}.\n",
            manifest.files.len(),
            snapshot.root()
        ),
    )?;
    Ok(snapshot.root().to_string())
}

pub fn query_directory(
    directory: &Path,
    vector: Vec<f32>,
    limit: usize,
    model: &str,
) -> Result<serde_json::Value> {
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json"))?)?;
    if manifest.model != model {
        return Err("query model does not match indexed model".into());
    }
    let snapshot = Snapshot::open_directory(directory.join("index"))?;
    let result = snapshot.query(
        Query::new(vector, limit)
            .in_vector_space(model)
            .with_payload(),
    )?;
    Ok(serde_json::json!({
        "source_tree": manifest.source_tree,
        "index_root": result.root,
        "model": model,
        "hits": result.points,
        "stats": result.stats,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_preserve_unicode_and_line_spans() {
        let input = "alpha\nβeta\nlast";
        let result = chunks(input, 6).unwrap();
        assert_eq!(
            result.iter().map(|x| x.text.as_str()).collect::<String>(),
            input
        );
        assert_eq!((result[0].start_line, result[0].end_line), (1, 1));
        assert_eq!((result[1].start_line, result[1].end_line), (2, 3));
        assert_eq!((result[2].start_line, result[2].end_line), (3, 3));
        assert!(chunks("", 12).unwrap().is_empty());
        assert!(chunks(" \n\t", 12).unwrap().is_empty());
        assert!(chunks(input, 0).is_err());
    }

    #[test]
    fn duplicate_content_preserves_occurrences_and_old_snapshots() {
        let initial = Manifest {
            source_tree: "a".repeat(40),
            files: vec![
                FileLocation {
                    path: "a.rs".into(),
                    blob: "b".repeat(40),
                },
                FileLocation {
                    path: "copy.rs".into(),
                    blob: "b".repeat(40),
                },
            ],
            skipped: vec![],
            model: "fixture-v1".into(),
            dimension: 2,
            chunk_chars: 100,
        };
        let embedded = BTreeMap::from([(
            "b".repeat(40),
            vec![EmbeddedChunk {
                chunk: Chunk {
                    text: "retry requests".into(),
                    start_line: 1,
                    end_line: 1,
                },
                vector: vec![1.0, 0.0],
            }],
        )]);
        let temp = tempfile::tempdir().unwrap();
        let first = build_directory(&temp.path().join("first"), &initial, &embedded).unwrap();
        let result =
            query_directory(&temp.path().join("first"), vec![1.0, 0.0], 10, "fixture-v1").unwrap();
        assert_eq!(result["hits"].as_array().unwrap().len(), 2);
        let mut next = initial.clone();
        next.files.remove(0);
        next.files[0].path = "renamed.rs".into();
        next.source_tree = "c".repeat(40);
        let second = build_directory(&temp.path().join("next"), &next, &embedded).unwrap();
        assert_ne!(first, second);
        assert!(
            query_directory(&temp.path().join("next"), vec![1.0, 0.0], 10, "different").is_err()
        );
        assert_eq!(
            query_directory(&temp.path().join("first"), vec![1.0, 0.0], 10, "fixture-v1").unwrap()
                ["hits"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
