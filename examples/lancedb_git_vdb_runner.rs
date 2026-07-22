use git_vdb::{
    CollectionConfig, Condition, Database, Filter, Point, PointId, Query, QueryParams, Range,
    SnapshotEngine, SnapshotMutation,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

#[derive(Deserialize)]
struct RunSpec {
    schema_version: u32,
    case_name: String,
    dimension: usize,
    point_count: usize,
    query_count: usize,
    points_path: PathBuf,
    queries_path: PathBuf,
    k: Vec<usize>,
    mutation_fractions: Vec<f64>,
    filter_selectivities: Vec<f64>,
    concurrency: Vec<usize>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let input = args.next().ok_or("usage: runner INPUT.json OUTPUT.json")?;
    let output = args.next().ok_or("usage: runner INPUT.json OUTPUT.json")?;
    if args.next().is_some() {
        return Err("usage: runner INPUT.json OUTPUT.json".into());
    }
    let spec: RunSpec = serde_json::from_slice(&fs::read(input)?)?;
    if spec.schema_version != 1 {
        return Err(format!("unsupported harness schema version {}", spec.schema_version).into());
    }
    let vectors = read_vectors(&spec.points_path, spec.point_count, spec.dimension)?;
    let queries = read_vectors(&spec.queries_path, spec.query_count, spec.dimension)?;
    let points = make_points(&vectors);
    let maximum_k = *spec.k.iter().max().ok_or("k must not be empty")?;
    let config = CollectionConfig {
        dimension: spec.dimension,
        ..CollectionConfig::default()
    };

    let setup_started = Instant::now();
    let core_dir = tempfile::TempDir::new()?;
    let engine = SnapshotEngine::init(core_dir.path())?;
    let setup_us = micros(setup_started);
    let build_started = Instant::now();
    let snapshot = engine.build(config.clone(), points.clone())?;
    let build_us = micros(build_started);
    let baseline_on_disk_bytes = directory_bytes(core_dir.path())?;

    let (exact_query_us, exact_results, exact_vectors_scored) =
        query_all(&snapshot, &queries, maximum_k, true, None)?;
    let (approximate_query_us, approximate_results, approximate_vectors_scored) =
        query_all(&snapshot, &queries, maximum_k, false, None)?;
    let snapshot_throughput = json!({
        "exact": query_snapshot_throughput(
            &snapshot,
            &queries,
            maximum_k,
            true,
            &spec.concurrency,
        )?,
        "approximate": query_snapshot_throughput(
            &snapshot,
            &queries,
            maximum_k,
            false,
            &spec.concurrency,
        )?,
    });

    let mut filtered = Map::new();
    for selectivity in &spec.filter_selectivities {
        let filter = selectivity_filter(*selectivity);
        let (exact_us, exact, exact_scored) =
            query_all(&snapshot, &queries, maximum_k, true, Some(filter.clone()))?;
        let (approximate_us, approximate, approximate_scored) =
            query_all(&snapshot, &queries, maximum_k, false, Some(filter))?;
        filtered.insert(
            selectivity.to_string(),
            json!({
                "exact_query_us": exact_us,
                "approximate_query_us": approximate_us,
                "exact_results": exact,
                "approximate_results": approximate,
                "exact_vectors_scored": exact_scored,
                "approximate_vectors_scored": approximate_scored,
            }),
        );
    }

    let mut mutations = Map::new();
    for fraction in &spec.mutation_fractions {
        let count = fraction_count(spec.point_count, *fraction);
        let changed = changed_points(&points[..count]);
        let upsert_started = Instant::now();
        let upserted = engine.apply(
            snapshot.root(),
            changed.into_iter().map(SnapshotMutation::upsert).collect(),
        )?;
        let upsert_us = micros(upsert_started);
        let delete_started = Instant::now();
        let deleted = engine.apply(
            snapshot.root(),
            vec![SnapshotMutation::delete_ids(
                (0..count).map(|id| PointId::from(id as u64)),
            )],
        )?;
        let delete_us = micros(delete_started);
        mutations.insert(
            fraction.to_string(),
            json!({
                "points": count,
                "upsert_us": upsert_us,
                "delete_us": delete_us,
                "upsert_root": upserted.root(),
                "delete_root": deleted.root(),
            }),
        );
    }

    let adapter_setup_started = Instant::now();
    let adapter_dir = tempfile::TempDir::new()?;
    let database = Database::init_bare(adapter_dir.path())?;
    let collection = database.create_collection("benchmark", config)?;
    let adapter_setup_us = micros(adapter_setup_started);
    let adapter_build_started = Instant::now();
    let adapter_root = collection.upsert(points)?.root;
    let adapter_build_us = micros(adapter_build_started);
    if adapter_root != snapshot.root() {
        return Err("snapshot-core and named-adapter roots differ".into());
    }
    query_collection_all(&collection, &queries[..1], maximum_k, true, None)?;
    query_collection_all(&collection, &queries[..1], maximum_k, false, None)?;
    let (adapter_exact_query_us, adapter_exact_results, adapter_exact_vectors_scored) =
        query_collection_all(&collection, &queries, maximum_k, true, None)?;
    let (
        adapter_approximate_query_us,
        adapter_approximate_results,
        adapter_approximate_vectors_scored,
    ) = query_collection_all(&collection, &queries, maximum_k, false, None)?;
    let adapter_throughput = json!({
        "exact": query_collection_throughput(
            &collection,
            &queries,
            maximum_k,
            true,
            &spec.concurrency,
        )?,
        "approximate": query_collection_throughput(
            &collection,
            &queries,
            maximum_k,
            false,
            &spec.concurrency,
        )?,
    });
    let historical_started = Instant::now();
    let historical = collection.at(&adapter_root)?;
    let historical_count = historical.count(None)?.count;
    let historical_read_us = micros(historical_started);

    let report = json!({
        "schema_version": 1,
        "engine": "git-vdb",
        "case_name": spec.case_name,
        "point_count": spec.point_count,
        "dimension": spec.dimension,
        "query_count": spec.query_count,
        "k": spec.k,
        "root": snapshot.root(),
        "setup_us": setup_us,
        "snapshot_core": {
            "build_us": build_us,
            "exact_query_us": exact_query_us,
            "approximate_query_us": approximate_query_us,
            "exact_results": exact_results,
            "approximate_results": approximate_results,
            "exact_vectors_scored": exact_vectors_scored,
            "approximate_vectors_scored": approximate_vectors_scored,
            "throughput": snapshot_throughput,
            "filtered": filtered,
            "mutations": mutations,
            "on_disk_bytes": baseline_on_disk_bytes,
        },
        "named_adapter": {
            "setup_us": adapter_setup_us,
            "build_us": adapter_build_us,
            "exact_query_us": adapter_exact_query_us,
            "approximate_query_us": adapter_approximate_query_us,
            "exact_results": adapter_exact_results,
            "approximate_results": adapter_approximate_results,
            "exact_vectors_scored": adapter_exact_vectors_scored,
            "approximate_vectors_scored": adapter_approximate_vectors_scored,
            "throughput": adapter_throughput,
            "historical_read_us": historical_read_us,
            "historical_count": historical_count,
            "on_disk_bytes": directory_bytes(adapter_dir.path())?,
        }
    });
    fs::write(output, serde_json::to_vec_pretty(&report)?)?;
    Ok(())
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

type QueryBatch = (Vec<u64>, Vec<Value>, Vec<usize>);

fn query_all(
    snapshot: &git_vdb::Snapshot,
    queries: &[Vec<f32>],
    limit: usize,
    exact: bool,
    filter: Option<Filter>,
) -> git_vdb::Result<QueryBatch> {
    let mut durations = Vec::with_capacity(queries.len());
    let mut results = Vec::with_capacity(queries.len());
    let mut vectors_scored = Vec::with_capacity(queries.len());
    for vector in queries {
        let started = Instant::now();
        let result = snapshot.query(Query {
            vector: vector.clone(),
            limit,
            filter: filter.clone(),
            params: QueryParams {
                exact: Some(exact),
                ..QueryParams::default()
            },
            ..Query::default()
        })?;
        durations.push(micros(started));
        vectors_scored.push(result.stats.vectors_scored);
        results.push(Value::Array(
            result
                .points
                .into_iter()
                .map(|point| json!({"id": point.id, "score": point.score}))
                .collect(),
        ));
    }
    Ok((durations, results, vectors_scored))
}

fn query_collection_all(
    collection: &git_vdb::Collection,
    queries: &[Vec<f32>],
    limit: usize,
    exact: bool,
    filter: Option<Filter>,
) -> git_vdb::Result<QueryBatch> {
    let mut durations = Vec::with_capacity(queries.len());
    let mut results = Vec::with_capacity(queries.len());
    let mut vectors_scored = Vec::with_capacity(queries.len());
    for vector in queries {
        let started = Instant::now();
        let result = collection.query(Query {
            vector: vector.clone(),
            limit,
            filter: filter.clone(),
            params: QueryParams {
                exact: Some(exact),
                ..QueryParams::default()
            },
            ..Query::default()
        })?;
        durations.push(micros(started));
        vectors_scored.push(result.stats.vectors_scored);
        results.push(Value::Array(
            result
                .points
                .into_iter()
                .map(|point| json!({"id": point.id, "score": point.score}))
                .collect(),
        ));
    }
    Ok((durations, results, vectors_scored))
}

fn query_snapshot_throughput(
    snapshot: &git_vdb::Snapshot,
    queries: &[Vec<f32>],
    limit: usize,
    exact: bool,
    concurrencies: &[usize],
) -> git_vdb::Result<Map<String, Value>> {
    let mut measurements = Map::new();
    for &workers in concurrencies {
        if workers == 0 {
            return Err(git_vdb::Error::Invalid(
                "benchmark concurrency must be positive".into(),
            ));
        }
        let started = Instant::now();
        let outcome: git_vdb::Result<()> = thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for worker in 0..workers {
                let snapshot = snapshot.clone();
                handles.push(scope.spawn(move || -> git_vdb::Result<()> {
                    for vector in queries.iter().skip(worker).step_by(workers) {
                        snapshot.query(Query {
                            vector: vector.clone(),
                            limit,
                            params: QueryParams {
                                exact: Some(exact),
                                ..QueryParams::default()
                            },
                            ..Query::default()
                        })?;
                    }
                    Ok(())
                }));
            }
            join_query_workers(handles)
        });
        outcome?;
        measurements.insert(
            workers.to_string(),
            throughput_measurement(queries.len(), micros(started)),
        );
    }
    Ok(measurements)
}

fn query_collection_throughput(
    collection: &git_vdb::Collection,
    queries: &[Vec<f32>],
    limit: usize,
    exact: bool,
    concurrencies: &[usize],
) -> git_vdb::Result<Map<String, Value>> {
    let mut measurements = Map::new();
    for &workers in concurrencies {
        if workers == 0 {
            return Err(git_vdb::Error::Invalid(
                "benchmark concurrency must be positive".into(),
            ));
        }
        let started = Instant::now();
        let outcome: git_vdb::Result<()> = thread::scope(|scope| {
            let mut handles = Vec::with_capacity(workers);
            for worker in 0..workers {
                let collection = collection.clone();
                handles.push(scope.spawn(move || -> git_vdb::Result<()> {
                    for vector in queries.iter().skip(worker).step_by(workers) {
                        collection.query(Query {
                            vector: vector.clone(),
                            limit,
                            params: QueryParams {
                                exact: Some(exact),
                                ..QueryParams::default()
                            },
                            ..Query::default()
                        })?;
                    }
                    Ok(())
                }));
            }
            join_query_workers(handles)
        });
        outcome?;
        measurements.insert(
            workers.to_string(),
            throughput_measurement(queries.len(), micros(started)),
        );
    }
    Ok(measurements)
}

fn join_query_workers(
    handles: Vec<thread::ScopedJoinHandle<'_, git_vdb::Result<()>>>,
) -> git_vdb::Result<()> {
    for handle in handles {
        match handle.join() {
            Ok(result) => result?,
            Err(_) => {
                return Err(git_vdb::Error::Invalid(
                    "benchmark query worker panicked".into(),
                ));
            }
        }
    }
    Ok(())
}

fn throughput_measurement(queries: usize, wall_us: u64) -> Value {
    json!({
        "queries": queries,
        "wall_us": wall_us,
        "queries_per_second": queries as f64 * 1_000_000.0 / wall_us as f64,
    })
}

fn selectivity_filter(selectivity: f64) -> Filter {
    Filter::must([Condition::range(
        "selectivity_bucket",
        Range {
            lt: Some((selectivity * 1000.0).round()),
            ..Range::default()
        },
    )])
}

fn fraction_count(total: usize, fraction: f64) -> usize {
    ((total as f64 * fraction).round() as usize).clamp(1, total)
}

fn changed_points(points: &[Point]) -> Vec<Point> {
    points
        .iter()
        .cloned()
        .map(|mut point| {
            point.vector[0] += 0.001;
            point
        })
        .collect()
}

fn micros(started: Instant) -> u64 {
    started.elapsed().as_micros().try_into().unwrap_or(u64::MAX)
}

fn directory_bytes(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += directory_bytes(&entry.path())?;
        } else {
            total += metadata.len();
        }
    }
    Ok(total)
}
