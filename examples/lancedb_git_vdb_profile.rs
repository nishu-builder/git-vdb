use git_vdb::{
    CollectionConfig, Point, PointId, Query, QueryParams, SnapshotEngine, SnapshotMutation,
};
use serde::Deserialize;
use serde_json::{json, Map};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use std::time::Instant;

#[derive(Deserialize)]
struct RunSpec {
    schema_version: u32,
    dimension: usize,
    point_count: usize,
    query_count: usize,
    points_path: PathBuf,
    queries_path: PathBuf,
    k: Vec<usize>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, input, repository, output] if command == "build" => {
            build(Path::new(input), Path::new(repository), Path::new(output))
        }
        [command, input, repository, build_report, mode, output] if command == "query" => query(
            Path::new(input),
            Path::new(repository),
            Path::new(build_report),
            mode.to_str().ok_or("query mode is not UTF-8")?,
            Path::new(output),
        ),
        [command, input, repository, build_report, fraction, output] if command == "mutate" => {
            mutate(
                Path::new(input),
                Path::new(repository),
                Path::new(build_report),
                fraction
                    .to_str()
                    .ok_or("mutation fraction is not UTF-8")?
                    .parse()?,
                Path::new(output),
            )
        }
        [command, repository, build_report, output] if command == "validate" => validate(
            Path::new(repository),
            Path::new(build_report),
            Path::new(output),
        ),
        _ => Err(
            "usage: lancedb_git_vdb_profile build INPUT.json REPOSITORY OUTPUT.json\n       lancedb_git_vdb_profile query INPUT.json REPOSITORY BUILD.json exact|approximate OUTPUT.json\n       lancedb_git_vdb_profile mutate INPUT.json REPOSITORY BUILD.json FRACTION OUTPUT.json\n       lancedb_git_vdb_profile validate REPOSITORY BUILD.json OUTPUT.json"
                .into(),
        ),
    }
}

fn mutate(
    input: &Path,
    repository: &Path,
    build_report: &Path,
    fraction: f64,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = read_spec(input)?;
    if !(0.0..=1.0).contains(&fraction) || fraction == 0.0 {
        return Err("mutation fraction must be greater than zero and at most one".into());
    }
    let vectors = read_vectors(&spec.points_path, spec.point_count, spec.dimension)?;
    let points = make_points(&vectors);
    let count = ((spec.point_count as f64 * fraction).round() as usize).clamp(1, spec.point_count);
    let mut changed = points[..count].to_vec();
    for point in &mut changed {
        point.vector[0] += 0.001;
    }
    let build: serde_json::Value = serde_json::from_slice(&fs::read(build_report)?)?;
    let root = build
        .get("root")
        .and_then(serde_json::Value::as_str)
        .ok_or("build report root is missing")?;
    let engine = SnapshotEngine::open(repository)?;

    let upsert_started = Instant::now();
    let upserted = engine.apply(
        root,
        changed.into_iter().map(SnapshotMutation::upsert).collect(),
    )?;
    let upsert_us = micros(upsert_started);
    let delete_started = Instant::now();
    let deleted = engine.apply(
        root,
        vec![SnapshotMutation::delete_ids(
            (0..count).map(|id| PointId::from(id as u64)),
        )],
    )?;
    let delete_us = micros(delete_started);
    fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "root": root,
            "fraction": fraction,
            "points": count,
            "upsert_us": upsert_us,
            "delete_us": delete_us,
            "upsert_root": upserted.root(),
            "delete_root": deleted.root(),
            "on_disk_bytes_after": directory_bytes(repository)?,
        }))?,
    )?;
    Ok(())
}

fn validate(
    repository: &Path,
    build_report: &Path,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let build: serde_json::Value = serde_json::from_slice(&fs::read(build_report)?)?;
    let root = build
        .get("root")
        .and_then(serde_json::Value::as_str)
        .ok_or("build report root is missing")?;
    let engine = SnapshotEngine::open(repository)?;
    let started = Instant::now();
    let report = engine.validate(root, true)?;
    fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "root": root,
            "validation_us": micros(started),
            "report": report,
        }))?,
    )?;
    Ok(())
}

fn build(input: &Path, repository: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if repository.exists() {
        return Err(format!("repository already exists: {}", repository.display()).into());
    }
    let spec = read_spec(input)?;
    let vectors = read_vectors(&spec.points_path, spec.point_count, spec.dimension)?;
    let points = make_points(&vectors);
    let config = CollectionConfig {
        dimension: spec.dimension,
        ..CollectionConfig::default()
    };
    let engine = SnapshotEngine::init(repository)?;
    let started = Instant::now();
    let snapshot = engine.build(config, points)?;
    let build_us = micros(started);
    fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "root": snapshot.root(),
            "build_us": build_us,
            "on_disk_bytes": directory_bytes(repository)?,
        }))?,
    )?;
    Ok(())
}

fn query(
    input: &Path,
    repository: &Path,
    build_report: &Path,
    mode: &str,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let exact = match mode {
        "exact" => true,
        "approximate" => false,
        _ => return Err(format!("unsupported query mode: {mode}").into()),
    };
    let spec = read_spec(input)?;
    let queries = read_vectors(&spec.queries_path, spec.query_count, spec.dimension)?;
    let maximum_k = *spec.k.iter().max().ok_or("k must not be empty")?;
    let build: serde_json::Value = serde_json::from_slice(&fs::read(build_report)?)?;
    let root = build
        .get("root")
        .and_then(serde_json::Value::as_str)
        .ok_or("build report root is missing")?;
    let engine = SnapshotEngine::open(repository)?;
    let snapshot = engine.open_snapshot(root)?;

    // Fill the immutable snapshot cache or warm the approximate ODB path without
    // including that one-time work in the samples.
    snapshot.query(make_query(&queries[0], maximum_k, exact))?;
    wait_for_profiler()?;

    let mut query_us = Vec::with_capacity(queries.len());
    let mut results = Vec::with_capacity(queries.len());
    let mut vectors_scored = Vec::with_capacity(queries.len());
    let batch_started = Instant::now();
    for vector in &queries {
        let started = Instant::now();
        let result = snapshot.query(make_query(vector, maximum_k, exact))?;
        query_us.push(micros(started));
        vectors_scored.push(result.stats.vectors_scored);
        results.push(result.points);
    }
    let batch_us = micros(batch_started);
    fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "root": root,
            "mode": mode,
            "query_us": query_us,
            "batch_us": batch_us,
            "vectors_scored": vectors_scored,
            "results": results,
        }))?,
    )?;
    Ok(())
}

fn wait_for_profiler() -> Result<(), Box<dyn std::error::Error>> {
    let Ok(ready_path) = env::var("GIT_VDB_PROFILE_READY") else {
        return Ok(());
    };
    let go_path = env::var("GIT_VDB_PROFILE_GO")
        .map_err(|_| "GIT_VDB_PROFILE_GO is required when profiler waiting is enabled")?;
    fs::write(ready_path, std::process::id().to_string())?;
    while !Path::new(&go_path).exists() {
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn read_spec(path: &Path) -> Result<RunSpec, Box<dyn std::error::Error>> {
    let spec: RunSpec = serde_json::from_slice(&fs::read(path)?)?;
    if spec.schema_version != 1 {
        return Err(format!("unsupported harness schema version {}", spec.schema_version).into());
    }
    Ok(spec)
}

fn make_query(vector: &[f32], limit: usize, exact: bool) -> Query {
    Query {
        vector: vector.to_vec(),
        limit,
        params: QueryParams {
            exact: Some(exact),
            ..QueryParams::default()
        },
        ..Query::default()
    }
}

fn read_vectors(
    path: &Path,
    count: usize,
    dimension: usize,
) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let expected = count
        .checked_mul(dimension)
        .and_then(|components| components.checked_mul(4))
        .ok_or("dataset size overflow")?;
    if bytes.len() != expected {
        return Err(format!(
            "{} has {} bytes, expected {expected}",
            path.display(),
            bytes.len()
        )
        .into());
    }
    Ok(bytes
        .chunks_exact(dimension * 4)
        .map(|row| {
            row.chunks_exact(4)
                .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
                .collect()
        })
        .collect())
}

fn make_points(vectors: &[Vec<f32>]) -> Vec<Point> {
    vectors
        .iter()
        .enumerate()
        .map(|(id, vector)| {
            let mut payload = Map::new();
            payload.insert("selectivity_bucket".into(), json!(id % 1000));
            Point {
                id: (id as u64).into(),
                vector: vector.clone(),
                payload,
            }
        })
        .collect()
}

fn directory_bytes(path: &Path) -> Result<u64, std::io::Error> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            directory_bytes(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}

fn micros(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}
