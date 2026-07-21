use git_vdb::{CollectionConfig, Database, IndexConfig, Point, Query, QueryParams};
use serde::Serialize;
use std::collections::BTreeSet;
use std::env;
use std::process::Command;
use std::time::Instant;

const SEED: u64 = 0x6769_7476_6462_626d;

#[derive(Serialize)]
struct Report {
    seed: u64,
    points: usize,
    dimension: usize,
    queries: usize,
    clusters: usize,
    index: IndexConfig,
    root: String,
    build_ms: u128,
    exact_query_ms: u128,
    approximate_query_ms: u128,
    recall_at_1: f64,
    recall_at_5: f64,
    recall_at_10: f64,
    median_scored_fraction: f64,
    loose_objects: usize,
    loose_kib: usize,
    revision: String,
    target: String,
}

fn main() -> git_vdb::Result<()> {
    let args: Vec<_> = env::args().collect();
    let count = argument(&args, 1, 1_000);
    let dimension = argument(&args, 2, 768);
    let query_count = argument(&args, 3, 100);
    let clusters = 32.min(count.max(1));
    let mut rng = SplitMix64(SEED);
    let centers: Vec<Vec<f32>> = (0..clusters)
        .map(|_| (0..dimension).map(|_| rng.signed()).collect())
        .collect();
    let mut points = Vec::with_capacity(count);
    for id in 0..count {
        let center = &centers[id % clusters];
        let vector = center
            .iter()
            .map(|component| component + rng.signed() * 0.08)
            .collect();
        points.push(Point {
            id: (id as u64).into(),
            vector,
            payload: Default::default(),
        });
    }
    let queries: Vec<Vec<f32>> = (0..query_count)
        .map(|index| {
            centers[index % clusters]
                .iter()
                .map(|component| component + rng.signed() * 0.04)
                .collect()
        })
        .collect();

    let temp = tempfile::TempDir::new().expect("temporary benchmark repository");
    let db = Database::init_bare(temp.path())?;
    let config = CollectionConfig {
        dimension,
        ..CollectionConfig::default()
    };
    let collection = db.create_collection("benchmark", config.clone())?;
    let started = Instant::now();
    let root = collection.upsert(points)?.root;
    let build_ms = started.elapsed().as_millis();

    let exact_started = Instant::now();
    let mut exact = Vec::new();
    for vector in &queries {
        exact.push(collection.query(Query {
            vector: vector.clone(),
            limit: 10,
            params: QueryParams {
                exact: Some(true),
                ..QueryParams::default()
            },
            ..Query::default()
        })?);
    }
    let exact_query_ms = exact_started.elapsed().as_millis();

    let approximate_started = Instant::now();
    let mut approximate = Vec::new();
    for vector in &queries {
        approximate.push(collection.query(Query {
            vector: vector.clone(),
            limit: 10,
            params: QueryParams {
                exact: Some(false),
                ..QueryParams::default()
            },
            ..Query::default()
        })?);
    }
    let approximate_query_ms = approximate_started.elapsed().as_millis();

    let recall = |k: usize| -> f64 {
        exact
            .iter()
            .zip(&approximate)
            .map(|(oracle, result)| {
                let wanted: BTreeSet<_> = oracle.points.iter().take(k).map(|p| &p.id).collect();
                result
                    .points
                    .iter()
                    .take(k)
                    .filter(|p| wanted.contains(&p.id))
                    .count() as f64
                    / k as f64
            })
            .sum::<f64>()
            / query_count.max(1) as f64
    };
    let mut fractions: Vec<_> = approximate
        .iter()
        .map(|result| result.stats.vectors_scored as f64 / count.max(1) as f64)
        .collect();
    fractions.sort_by(f64::total_cmp);
    let median_scored_fraction = fractions.get(fractions.len() / 2).copied().unwrap_or(0.0);
    let count_objects = Command::new("git")
        .arg("--git-dir")
        .arg(temp.path())
        .args(["count-objects", "-v"])
        .output()
        .expect("git count-objects");
    let count_objects = String::from_utf8(count_objects.stdout).expect("UTF-8 Git output");
    let metric = |name: &str| {
        count_objects
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    let revision = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .unwrap_or_else(|| "uncommitted".into());
    println!(
        "{}",
        serde_json::to_string_pretty(&Report {
            seed: SEED,
            points: count,
            dimension,
            queries: query_count,
            clusters,
            index: config.index,
            root: root.0,
            build_ms,
            exact_query_ms,
            approximate_query_ms,
            recall_at_1: recall(1),
            recall_at_5: recall(5),
            recall_at_10: recall(10),
            median_scored_fraction,
            loose_objects: metric("count"),
            loose_kib: metric("size"),
            revision,
            target: format!("{}-{}", env::consts::OS, env::consts::ARCH),
        })?
    );
    Ok(())
}

fn argument(args: &[String], index: usize, default: usize) -> usize {
    args.get(index)
        .map(|value| value.parse().expect("benchmark arguments must be integers"))
        .unwrap_or(default)
}

struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn signed(&mut self) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1_u64 << 24) as f32;
        unit * 2.0 - 1.0
    }
}
