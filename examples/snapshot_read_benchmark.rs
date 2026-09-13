//! Reproducible fresh-process and warm-query comparison; see integrations/caos/README.md.
use git_vdb::*;
use serde_json::json;
use std::path::Path;
use std::time::Instant;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    match args.get(1).map(String::as_str) {
        Some("fixture") => {
            let directory = Path::new(args.get(2).ok_or("fixture needs output directory")?);
            std::fs::create_dir_all(directory)?;
            let points = (0..4096)
                .map(|i| {
                    let vector = (0..32)
                        .map(|j| {
                            if j == i % 32 {
                                1.0
                            } else {
                                ((i * 17 + j * 31) % 97) as f32 / 9700.0
                            }
                        })
                        .collect::<Vec<_>>();
                    Point::new(format!("point-{i:05}"), vector).with_metadata(json!({
                        "group": i % 3, "document": "deterministic source passage\n".repeat(40),
                    }))
                })
                .collect::<Result<Vec<_>>>()?;
            let root =
                SnapshotEngine::ephemeral()?.build(CollectionConfig::new(32), points.clone())?;
            root.materialize(directory.join("index"))?;
            std::fs::write(directory.join("root"), root.root().to_string())?;
            std::fs::write(directory.join("points.json"), serde_json::to_vec(&points)?)?;
            let vector = points[0].vector.clone();
            let mut selective = Query::approximate(vector.clone(), 10).with_payload();
            selective.params.probes = 1;
            selective.params.candidate_limit = 32;
            let mut broad = Query::approximate(vector.clone(), 10).with_payload();
            broad.params.probes = 64;
            broad.params.candidate_limit = 4096;
            let filtered = broad
                .clone()
                .with_filter(Filter::must([Condition::matches("group", 1)]));
            let exact = Query::exact(vector, 10).with_payload();
            std::fs::write(
                directory.join("queries.json"),
                serde_json::to_vec(&json!({
                    "selective":selective, "broad":broad, "filtered":filtered, "exact":exact,
                }))?,
            )?;
            println!("{}", root.root());
        }
        Some("export") => {
            let directory = Path::new(args.get(2).ok_or("export needs directory")?);
            let snapshot = Snapshot::open_directory(directory.join("index"))?;
            let points = snapshot
                .get(GetRequest {
                    with_payload: true,
                    with_vector: true,
                    ..GetRequest::default()
                })?
                .points
                .into_iter()
                .map(|record| Point {
                    id: record.id,
                    vector: record.vector.expect("requested vector"),
                    payload: record.payload.expect("requested payload"),
                })
                .collect::<Vec<_>>();
            std::fs::write(directory.join("root"), snapshot.root().to_string())?;
            std::fs::write(directory.join("points.json"), serde_json::to_vec(&points)?)?;
        }
        Some(mode @ ("eager" | "lazy" | "git")) => {
            let directory = Path::new(args.get(2).ok_or("query needs directory")?);
            let query: Query = serde_json::from_slice(&std::fs::read(
                args.get(3).ok_or("query needs JSON file")?,
            )?)?;
            let started = Instant::now();
            let mut results = Vec::new();
            let mut timings = Vec::new();
            let reads;
            let setup_ns;
            if mode == "eager" {
                let snapshot = Snapshot::open_directory(directory.join("index"))?;
                setup_ns = started.elapsed().as_nanos();
                for _ in 0..6 {
                    let start = Instant::now();
                    results.push(snapshot.query(query.clone())?);
                    timings.push(start.elapsed().as_nanos());
                }
                let (blobs, bytes, trees) = measure_directory(&directory.join("index"))?;
                reads = json!({"blob_reads":blobs, "blob_bytes":bytes, "directory_reads":trees});
            } else if mode == "git" {
                let root: ObjectId = std::fs::read_to_string(directory.join("root"))?.parse()?;
                let mut reader = SnapshotReader::open(
                    root.clone(),
                    GitSource::new(directory.join("objects.git"), &root)?,
                )?;
                setup_ns = started.elapsed().as_nanos();
                for _ in 0..6 {
                    let start = Instant::now();
                    results.push(reader.query(query.clone())?);
                    timings.push(start.elapsed().as_nanos());
                }
                reads = serde_json::to_value(reader.read_stats())?;
            } else {
                let root: ObjectId = std::fs::read_to_string(directory.join("root"))?.parse()?;
                let mut reader =
                    SnapshotReader::open(root, DirectorySource::new(directory.join("index"))?)?;
                setup_ns = started.elapsed().as_nanos();
                let start = Instant::now();
                results.push(reader.query(query.clone())?);
                timings.push(start.elapsed().as_nanos());
                reads = serde_json::to_value(reader.read_stats())?;
                for _ in 0..5 {
                    let start = Instant::now();
                    results.push(reader.query(query.clone())?);
                    timings.push(start.elapsed().as_nanos());
                }
            }
            for result in &results[1..] {
                assert_eq!(
                    serde_json::to_value(result)?,
                    serde_json::to_value(&results[0])?
                );
            }
            let peak = std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|text| {
                    text.lines()
                        .find(|line| line.starts_with("VmHWM:"))
                        .map(str::to_owned)
                });
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "mode":mode, "setup_ns":setup_ns, "query_ns":timings, "reads":reads,
                    "peak_memory":peak, "result":results[0],
                }))?
            );
        }
        _ => {
            return Err(
                "usage: snapshot_read_benchmark fixture|export DIR; eager|lazy DIR QUERY.json"
                    .into(),
            )
        }
    }
    Ok(())
}
fn measure_directory(path: &Path) -> std::io::Result<(usize, u64, usize)> {
    let mut totals = (0, 0, 1);
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let child = measure_directory(&entry.path())?;
            totals.0 += child.0;
            totals.1 += child.1;
            totals.2 += child.2;
        } else {
            totals.0 += 1;
            totals.1 += entry.metadata()?.len();
        }
    }
    Ok(totals)
}
