use git_vdb_caos::{chunks, Manifest};
use serde_json::json;
use std::path::Path;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(
        args.get(1).ok_or("manifest path required")?,
    )?)?;
    let source = Path::new(args.get(2).ok_or("source checkout required")?);
    let mut runs = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let mut files = Vec::new();
        for entry in &manifest.files {
            let bytes = std::fs::read(source.join(&entry.path))?;
            let blob = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)?.to_string();
            if blob != entry.blob {
                return Err(format!("source blob mismatch: {}", entry.path).into());
            }
            files.push(String::from_utf8(bytes)?);
        }
        let read_ns = started.elapsed().as_nanos();
        let started = Instant::now();
        let mut count = 0;
        for text in &files {
            let result = chunks(text, manifest.chunk_chars)?;
            count += result.len();
            std::hint::black_box(result);
        }
        runs.push(json!({"read_and_verify_ns":read_ns, "chunk_ns":started.elapsed().as_nanos(),
            "chunks":count, "files":files.len(), "bytes":files.iter().map(String::len).sum::<usize>()}));
    }
    println!(
        "{}",
        serde_json::to_string(&json!({"source_tree":manifest.source_tree,
        "chunk_chars":manifest.chunk_chars,"runs":runs,
        "note":"Local cached filesystem reads with blob verification; chunk CPU time measured separately from inference."}))?
    );
    Ok(())
}
