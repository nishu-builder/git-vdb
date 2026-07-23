//! Canonical format-version-2 sharded point storage and IVF-flat search.

use crate::codec::{canonical_json, id_digest, validate_vector_components};
use crate::filter::matches_filter;
use crate::root::{IvfConfig, PointChange, RootMeta, SearchView};
use crate::*;
use git2::{ObjectType, Oid, Repository, Tree};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const SHARD_BITS: usize = 6;
const SHARD_COUNT: usize = 1 << SHARD_BITS;
const SAMPLE_LIMIT: usize = 8_192;
const TRAINING_ITERATIONS: usize = 4;
const MAX_CENTROIDS: usize = 4_096;
const TREE_MODE: i32 = 0o040000;
const BLOB_MODE: i32 = 0o100644;
const IDS_MAGIC: &[u8; 8] = b"GTV2IDS\0";
const PAYLOADS_MAGIC: &[u8; 8] = b"GTV2PAY\0";
const VECTORS_MAGIC: &[u8; 8] = b"GTV2VEC\0";
const CODEBOOK_MAGIC: &[u8; 8] = b"GTV2IVF\0";
const SAMPLE_MAGIC: &[u8; 8] = b"GTV2SMP\0";
const POSTING_MAGIC: &[u8; 8] = b"GTV2PST\0";

#[derive(Clone, Debug)]
pub(crate) struct V2SearchIndex {
    centroids: Vec<Vec<f32>>,
    postings: Vec<Vec<(u16, u32)>>,
}

struct CanonicalIndex<'a> {
    search: V2SearchIndex,
    sample: Vec<&'a Point>,
}

pub(crate) fn build_root(
    repo: &Repository,
    config: &CollectionConfig,
    points: &BTreeMap<PointId, Point>,
) -> Result<Oid> {
    let shards = point_shards(points)?;
    let index = canonical_index(points)?;

    let ids_tree = write_blob_tree(
        repo,
        shards
            .iter()
            .map(|(shard, points)| Ok((format!("{shard:03x}.bin"), encode_ids(points)?)))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let payloads_tree = write_blob_tree(
        repo,
        shards
            .iter()
            .map(|(shard, points)| Ok((format!("{shard:03x}.bin"), encode_payloads(points)?)))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let vectors_tree = write_blob_tree(
        repo,
        shards
            .iter()
            .map(|(shard, points)| {
                Ok((
                    format!("{shard:03x}.f32le"),
                    encode_vectors(points, config.dimension)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?,
    )?;
    let mut points_builder = repo.treebuilder(None)?;
    points_builder.insert("ids", ids_tree, TREE_MODE)?;
    points_builder.insert("payloads", payloads_tree, TREE_MODE)?;
    points_builder.insert("vectors", vectors_tree, TREE_MODE)?;
    let points_tree = points_builder.write()?;

    let postings_tree = write_blob_tree(
        repo,
        index
            .search
            .postings
            .iter()
            .enumerate()
            .map(|(centroid, posting)| {
                Ok((format!("{centroid:04x}.bin"), encode_posting(posting)?))
            })
            .collect::<Result<Vec<_>>>()?,
    )?;
    let codebook = repo.blob(&encode_codebook(&index.search.centroids, config.dimension)?)?;
    let sample = repo.blob(&encode_sample(&index.sample)?)?;
    let mut ivf_builder = repo.treebuilder(None)?;
    ivf_builder.insert("codebook.bin", codebook, BLOB_MODE)?;
    ivf_builder.insert("postings", postings_tree, TREE_MODE)?;
    ivf_builder.insert("sample.bin", sample, BLOB_MODE)?;
    let ivf_tree = ivf_builder.write()?;
    let mut index_builder = repo.treebuilder(None)?;
    index_builder.insert("ivf-flat-v2", ivf_tree, TREE_MODE)?;
    let index_tree = index_builder.write()?;

    let meta = RootMeta::new_v2(
        config,
        points.len(),
        IvfConfig {
            shard_bits: SHARD_BITS,
            centroid_count: index.search.centroids.len(),
            training_sample_limit: SAMPLE_LIMIT,
            training_iterations: TRAINING_ITERATIONS,
        },
    );
    let meta = repo.blob(&canonical_json(&meta)?)?;
    let mut root = repo.treebuilder(None)?;
    root.insert("index", index_tree, TREE_MODE)?;
    root.insert("meta.json", meta, BLOB_MODE)?;
    root.insert("points", points_tree, TREE_MODE)?;
    Ok(root.write()?)
}

pub(crate) fn update_root(
    repo: &Repository,
    previous_root: Oid,
    config: &CollectionConfig,
    changes: &BTreeMap<PointId, PointChange>,
) -> Result<Oid> {
    let mut points = read_all_points(repo, previous_root)?;
    let mut changed_shards = Vec::with_capacity(changes.len());
    for (id, change) in changes {
        changed_shards.push(shard_for_id(id));
        match &change.new {
            Some(point) => {
                points.insert(id.clone(), point.clone());
            }
            None => {
                points.remove(id);
            }
        }
    }
    changed_shards.sort_unstable();
    changed_shards.dedup();
    let shards = point_shards(&points)?;

    let old_points = root_tree(repo, previous_root, "points")?;
    let old_ids = child_tree(repo, old_points, "ids")?;
    let old_payloads = child_tree(repo, old_points, "payloads")?;
    let old_vectors = child_tree(repo, old_points, "vectors")?;
    let mut ids_builder = repo.treebuilder(Some(&repo.find_tree(old_ids)?))?;
    let mut payloads_builder = repo.treebuilder(Some(&repo.find_tree(old_payloads)?))?;
    let mut vectors_builder = repo.treebuilder(Some(&repo.find_tree(old_vectors)?))?;
    for shard in changed_shards {
        let ids_name = format!("{shard:03x}.bin");
        let vectors_name = format!("{shard:03x}.f32le");
        if let Some(shard_points) = shards.get(&shard) {
            ids_builder.insert(&ids_name, repo.blob(&encode_ids(shard_points)?)?, BLOB_MODE)?;
            payloads_builder.insert(
                &ids_name,
                repo.blob(&encode_payloads(shard_points)?)?,
                BLOB_MODE,
            )?;
            vectors_builder.insert(
                &vectors_name,
                repo.blob(&encode_vectors(shard_points, config.dimension)?)?,
                BLOB_MODE,
            )?;
        } else {
            ids_builder.remove(&ids_name)?;
            payloads_builder.remove(&ids_name)?;
            vectors_builder.remove(&vectors_name)?;
        }
    }
    let mut points_builder = repo.treebuilder(Some(&repo.find_tree(old_points)?))?;
    points_builder.insert("ids", ids_builder.write()?, TREE_MODE)?;
    points_builder.insert("payloads", payloads_builder.write()?, TREE_MODE)?;
    points_builder.insert("vectors", vectors_builder.write()?, TREE_MODE)?;
    let points_tree = points_builder.write()?;

    let old_meta = crate::root::read_meta(repo, previous_root)?;
    let old_index = read_index(repo, previous_root, &old_meta)?;
    let new_index = canonical_index(&points)?;
    let old_index_tree = root_tree(repo, previous_root, "index")?;
    let old_ivf_tree = child_tree(repo, old_index_tree, "ivf-flat-v2")?;
    let old_ivf = repo.find_tree(old_ivf_tree)?;
    let codebook = if equal_vectors(&old_index.centroids, &new_index.search.centroids) {
        blob_oid(&old_ivf, "codebook.bin")?
    } else {
        repo.blob(&encode_codebook(
            &new_index.search.centroids,
            config.dimension,
        )?)?
    };
    let sample_bytes = encode_sample(&new_index.sample)?;
    let sample = if read_blob(repo, old_ivf_tree, "sample.bin")? == sample_bytes {
        blob_oid(&old_ivf, "sample.bin")?
    } else {
        repo.blob(&sample_bytes)?
    };
    let old_postings = child_tree(repo, old_ivf_tree, "postings")?;
    let postings_tree = if old_index.postings.len() == new_index.search.postings.len() {
        let mut postings = repo.treebuilder(Some(&repo.find_tree(old_postings)?))?;
        for (centroid, (old, new)) in old_index
            .postings
            .iter()
            .zip(&new_index.search.postings)
            .enumerate()
        {
            if old != new {
                postings.insert(
                    format!("{centroid:04x}.bin"),
                    repo.blob(&encode_posting(new)?)?,
                    BLOB_MODE,
                )?;
            }
        }
        postings.write()?
    } else {
        write_blob_tree(
            repo,
            new_index
                .search
                .postings
                .iter()
                .enumerate()
                .map(|(centroid, posting)| {
                    Ok((format!("{centroid:04x}.bin"), encode_posting(posting)?))
                })
                .collect::<Result<Vec<_>>>()?,
        )?
    };
    let mut ivf_builder = repo.treebuilder(Some(&old_ivf))?;
    ivf_builder.insert("codebook.bin", codebook, BLOB_MODE)?;
    ivf_builder.insert("postings", postings_tree, TREE_MODE)?;
    ivf_builder.insert("sample.bin", sample, BLOB_MODE)?;
    let mut index_builder = repo.treebuilder(Some(&repo.find_tree(old_index_tree)?))?;
    index_builder.insert("ivf-flat-v2", ivf_builder.write()?, TREE_MODE)?;

    let meta = RootMeta::new_v2(
        config,
        points.len(),
        IvfConfig {
            shard_bits: SHARD_BITS,
            centroid_count: new_index.search.centroids.len(),
            training_sample_limit: SAMPLE_LIMIT,
            training_iterations: TRAINING_ITERATIONS,
        },
    );
    let mut root = repo.treebuilder(Some(&repo.find_tree(previous_root)?))?;
    root.insert("index", index_builder.write()?, TREE_MODE)?;
    root.insert("meta.json", repo.blob(&canonical_json(&meta)?)?, BLOB_MODE)?;
    root.insert("points", points_tree, TREE_MODE)?;
    Ok(root.write()?)
}

pub(crate) fn read_all_points(repo: &Repository, root: Oid) -> Result<BTreeMap<PointId, Point>> {
    let meta = crate::root::read_meta(repo, root)?;
    let points_tree = root_tree(repo, root, "points")?;
    let ids_tree = child_tree(repo, points_tree, "ids")?;
    let payloads_tree = child_tree(repo, points_tree, "payloads")?;
    let vectors_tree = child_tree(repo, points_tree, "vectors")?;
    let ids_names = blob_names(repo, ids_tree)?;
    let payload_names = blob_names(repo, payloads_tree)?;
    let vector_names = blob_names(repo, vectors_tree)?;
    let expected_vectors = ids_names
        .iter()
        .map(|name| name.replace(".bin", ".f32le"))
        .collect::<Vec<_>>();
    if ids_names != payload_names || expected_vectors != vector_names {
        return Err(Error::Corrupt(
            "format-2 point shard sets do not match".into(),
        ));
    }
    let mut output = BTreeMap::new();
    for ids_name in ids_names {
        let shard = parse_shard_name(&ids_name, ".bin")?;
        let points = read_shard(
            repo,
            ids_tree,
            payloads_tree,
            vectors_tree,
            shard,
            meta.dimension,
        )?;
        for point in points {
            let id = point.id.clone();
            if output.insert(id, point).is_some() {
                return Err(Error::Corrupt("duplicate format-2 point ID".into()));
            }
        }
    }
    Ok(output)
}

pub(crate) fn read_point_by_id(
    repo: &Repository,
    root: Oid,
    id: &PointId,
) -> Result<Option<Point>> {
    let meta = crate::root::read_meta(repo, root)?;
    let points_tree = root_tree(repo, root, "points")?;
    let ids_tree = child_tree(repo, points_tree, "ids")?;
    let payloads_tree = child_tree(repo, points_tree, "payloads")?;
    let vectors_tree = child_tree(repo, points_tree, "vectors")?;
    let shard = shard_for_id(id);
    let name = format!("{shard:03x}.bin");
    let ids = repo.find_tree(ids_tree)?;
    if ids.get_name(&name).is_none() {
        return Ok(None);
    }
    let points = read_shard(
        repo,
        ids_tree,
        payloads_tree,
        vectors_tree,
        shard,
        meta.dimension,
    )?;
    Ok(points.into_iter().find(|point| &point.id == id))
}

pub(crate) fn approximate_query(
    repo: &Repository,
    root: Oid,
    meta: &RootMeta,
    query: Query,
    cached: Option<&OnceLock<SearchView>>,
) -> Result<QueryResult> {
    let local;
    let view = if let Some(cache) = cached {
        if cache.get().is_none() {
            let points = read_all_points(repo, root)?.into_values().collect();
            let _ = cache.set(SearchView::new(points));
        }
        cache.get().expect("format-2 query cache was initialized")
    } else {
        local = SearchView::new(read_all_points(repo, root)?.into_values().collect());
        &local
    };
    if view.v2_index.get().is_none() {
        let parsed = read_index(repo, root, meta)?;
        let _ = view.v2_index.set(parsed);
    }
    let index = view
        .v2_index
        .get()
        .expect("format-2 search index was initialized");
    let requested_probes = if query.params.probes == 0 {
        if query.filter.is_some() {
            index.centroids.len()
        } else {
            meta.index.default_probes
        }
    } else {
        query.params.probes
    };
    let probes = requested_probes.min(index.centroids.len());
    let candidate_limit = if query.params.candidate_limit == 0 {
        meta.index.default_candidate_limit
    } else {
        query.params.candidate_limit
    };
    let mut centroids = index
        .centroids
        .iter()
        .enumerate()
        .map(|(centroid, vector)| (centroid, cosine_f64(&query.vector, vector)))
        .collect::<Vec<_>>();
    centroids.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    centroids.truncate(probes);

    let mut ranked = Vec::new();
    let mut discovered = 0;
    let mut scored = 0;
    let mut buckets_probed = 0;
    let mut candidate_limit_exhausted = false;
    'centroids: for (centroid, _) in centroids {
        buckets_probed += 1;
        for &(shard, row) in &index.postings[centroid] {
            discovered += 1;
            let point = view.point_for_v2_row(shard, row)?;
            if query
                .filter
                .as_ref()
                .is_some_and(|filter| !matches_filter(filter, &point.id, &point.payload))
            {
                continue;
            }
            if scored >= candidate_limit {
                candidate_limit_exhausted = true;
                break 'centroids;
            }
            scored += 1;
            ranked.push(RankedPoint {
                ranking_score: cosine_f64(&query.vector, &point.vector),
                point: ScoredPoint {
                    id: point.id.clone(),
                    score: 0.0,
                    payload: query.with_payload.then(|| point.payload.clone()),
                    vector: query.with_vector.then(|| point.vector.clone()),
                },
            });
        }
    }
    for winner in &mut ranked {
        winner.point.score = winner.ranking_score as f32;
    }
    ranked.sort_by(compare_ranked);
    ranked.truncate(query.limit);
    Ok(QueryResult {
        root: root.into(),
        points: ranked.into_iter().map(|winner| winner.point).collect(),
        stats: QueryStats {
            mode: QueryMode::Approximate,
            collection_points: meta.point_count,
            buckets_probed,
            candidates_discovered: discovered,
            vectors_scored: scored,
            probe_limit_exhausted: buckets_probed == probes && probes < index.centroids.len(),
            candidate_limit_exhausted,
        },
    })
}

pub(crate) fn validate_root(
    repo: &Repository,
    root: Oid,
    meta: &RootMeta,
    full: bool,
) -> Result<ValidationReport> {
    crate::root::validate_config(&meta.config())
        .map_err(|error| Error::Corrupt(error.to_string()))?;
    validate_ivf_meta(meta)?;
    validate_tree_layout(repo, root)?;
    let points = read_all_points(repo, root)?;
    if points.len() != meta.point_count {
        return Err(Error::Corrupt(format!(
            "metadata point count {} does not match shard count {}",
            meta.point_count,
            points.len()
        )));
    }
    for point in points.values() {
        crate::root::validate_point(point, &meta.config())
            .map_err(|error| Error::Corrupt(error.to_string()))?;
    }
    let actual = read_index(repo, root, meta)?;
    if full {
        let expected = canonical_index(&points)?;
        if !equal_vectors(&actual.centroids, &expected.search.centroids)
            || actual.postings != expected.search.postings
        {
            return Err(Error::Corrupt(
                "format-2 IVF index does not match authoritative points".into(),
            ));
        }
        let ivf = child_tree(repo, root_tree(repo, root, "index")?, "ivf-flat-v2")?;
        if read_blob(repo, ivf, "sample.bin")? != encode_sample(&expected.sample)? {
            return Err(Error::Corrupt(
                "format-2 training sample does not match authoritative points".into(),
            ));
        }
    }
    Ok(ValidationReport {
        root: root.into(),
        full,
        point_count: points.len(),
        checked_buckets: if full { actual.postings.len() } else { 0 },
        valid: true,
    })
}

fn validate_ivf_meta(meta: &RootMeta) -> Result<()> {
    let ivf = meta
        .ivf
        .as_ref()
        .ok_or_else(|| Error::Corrupt("format-2 metadata is missing IVF configuration".into()))?;
    let expected_centroids = centroid_count(meta.point_count);
    if ivf.shard_bits != SHARD_BITS
        || ivf.centroid_count != expected_centroids
        || ivf.training_sample_limit != SAMPLE_LIMIT
        || ivf.training_iterations != TRAINING_ITERATIONS
    {
        return Err(Error::Corrupt(
            "unsupported format-2 IVF configuration".into(),
        ));
    }
    Ok(())
}

fn canonical_index(points: &BTreeMap<PointId, Point>) -> Result<CanonicalIndex<'_>> {
    let mut sample = points.values().collect::<Vec<_>>();
    sample.sort_by(|left, right| {
        id_digest(&left.id)
            .cmp(&id_digest(&right.id))
            .then_with(|| left.id.cmp(&right.id))
    });
    sample.truncate(SAMPLE_LIMIT.min(sample.len()));
    let count = centroid_count(points.len());
    let centroids = train(&sample, count);
    let shards = point_shards(points)?;
    let mut postings = vec![Vec::new(); count];
    for (shard, shard_points) in shards {
        for (row, point) in shard_points.into_iter().enumerate() {
            let centroid = closest_centroid(&point.vector, &centroids);
            postings[centroid].push((
                shard,
                u32::try_from(row)
                    .map_err(|_| Error::Invalid("format-2 shard exceeds u32 rows".into()))?,
            ));
        }
    }
    Ok(CanonicalIndex {
        search: V2SearchIndex {
            centroids,
            postings,
        },
        sample,
    })
}

fn train(sample: &[&Point], count: usize) -> Vec<Vec<f32>> {
    if count == 0 {
        return Vec::new();
    }
    let mut centroids = (0..count)
        .map(|centroid| {
            let position = if count == 1 {
                0
            } else {
                centroid * (sample.len() - 1) / (count - 1)
            };
            sample[position].vector.clone()
        })
        .collect::<Vec<_>>();
    for _ in 0..TRAINING_ITERATIONS {
        let mut sums = vec![vec![0.0_f64; sample[0].vector.len()]; count];
        let mut counts = vec![0_usize; count];
        for point in sample {
            let centroid = closest_centroid(&point.vector, &centroids);
            counts[centroid] += 1;
            for (sum, component) in sums[centroid].iter_mut().zip(&point.vector) {
                *sum += f64::from(*component);
            }
        }
        for centroid in 0..count {
            if counts[centroid] == 0 {
                continue;
            }
            for (component, sum) in centroids[centroid].iter_mut().zip(&sums[centroid]) {
                *component = (*sum / counts[centroid] as f64) as f32;
            }
        }
    }
    centroids
}

fn closest_centroid(vector: &[f32], centroids: &[Vec<f32>]) -> usize {
    let mut best = 0;
    let mut best_score = f64::NEG_INFINITY;
    for (index, centroid) in centroids.iter().enumerate() {
        let score = cosine_f64(vector, centroid);
        if score > best_score {
            best = index;
            best_score = score;
        }
    }
    best
}

fn centroid_count(points: usize) -> usize {
    if points == 0 {
        return 0;
    }
    let floor = points.isqrt();
    let lower_distance = points - floor * floor;
    let next_square_step = floor * 2 + 1;
    let rounded = if lower_distance < next_square_step - lower_distance {
        floor
    } else {
        floor + 1
    };
    let rounded = rounded.clamp(1, MAX_CENTROIDS);
    let lower = 1_usize << (usize::BITS - 1 - rounded.leading_zeros());
    let upper = lower.saturating_mul(2).min(MAX_CENTROIDS);
    if rounded - lower < upper - rounded {
        lower
    } else {
        upper
    }
}

fn point_shards(points: &BTreeMap<PointId, Point>) -> Result<BTreeMap<u16, Vec<&Point>>> {
    let mut shards = BTreeMap::<u16, Vec<&Point>>::new();
    for point in points.values() {
        let shard = shard_for_id(&point.id);
        let shard_points = shards.entry(shard).or_default();
        if shard_points.len() == u32::MAX as usize {
            return Err(Error::Invalid("format-2 shard exceeds u32 rows".into()));
        }
        shard_points.push(point);
    }
    Ok(shards)
}

pub(crate) fn shard_for_id(id: &PointId) -> u16 {
    let digest = id_digest(id);
    u16::from(digest[0] >> (8 - SHARD_BITS))
}

fn encode_ids(points: &[&Point]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut offsets = Vec::with_capacity(points.len() + 1);
    offsets.push(0_u32);
    for point in points {
        match &point.id {
            PointId::String(value) => {
                body.push(0);
                let length = u32::try_from(value.len())
                    .map_err(|_| Error::Invalid("string point ID exceeds u32 bytes".into()))?;
                body.extend_from_slice(&length.to_le_bytes());
                body.extend_from_slice(value.as_bytes());
            }
            PointId::UInt(value) => {
                body.push(1);
                body.extend_from_slice(&value.to_le_bytes());
            }
        }
        offsets.push(
            u32::try_from(body.len())
                .map_err(|_| Error::Invalid("format-2 ID shard exceeds u32 bytes".into()))?,
        );
    }
    encode_offsets(IDS_MAGIC, points.len(), &offsets, &body)
}

fn encode_payloads(points: &[&Point]) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut offsets = Vec::with_capacity(points.len() + 1);
    offsets.push(0_u32);
    for point in points {
        body.extend_from_slice(&canonical_json(&point.payload)?);
        offsets.push(
            u32::try_from(body.len())
                .map_err(|_| Error::Invalid("format-2 payload shard exceeds u32 bytes".into()))?,
        );
    }
    encode_offsets(PAYLOADS_MAGIC, points.len(), &offsets, &body)
}

fn encode_offsets(magic: &[u8; 8], count: usize, offsets: &[u32], body: &[u8]) -> Result<Vec<u8>> {
    let count = u32::try_from(count)
        .map_err(|_| Error::Invalid("format-2 shard exceeds u32 rows".into()))?;
    let mut bytes = Vec::with_capacity(12 + offsets.len() * 4 + body.len());
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&count.to_le_bytes());
    for offset in offsets {
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    bytes.extend_from_slice(body);
    Ok(bytes)
}

fn encode_vectors(points: &[&Point], dimension: usize) -> Result<Vec<u8>> {
    let dimension = u32::try_from(dimension)
        .map_err(|_| Error::Invalid("vector dimension exceeds u32".into()))?;
    let count = u32::try_from(points.len())
        .map_err(|_| Error::Invalid("format-2 shard exceeds u32 rows".into()))?;
    let mut bytes = Vec::with_capacity(16 + points.len() * dimension as usize * 4);
    bytes.extend_from_slice(VECTORS_MAGIC);
    bytes.extend_from_slice(&dimension.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    for point in points {
        validate_vector_components(&point.vector)?;
        for component in &point.vector {
            bytes.extend_from_slice(&component.to_bits().to_le_bytes());
        }
    }
    Ok(bytes)
}

fn encode_codebook(centroids: &[Vec<f32>], dimension: usize) -> Result<Vec<u8>> {
    let dimension = u32::try_from(dimension)
        .map_err(|_| Error::Invalid("vector dimension exceeds u32".into()))?;
    let count = u32::try_from(centroids.len())
        .map_err(|_| Error::Invalid("centroid count exceeds u32".into()))?;
    let mut bytes = Vec::with_capacity(16 + centroids.len() * dimension as usize * 4);
    bytes.extend_from_slice(CODEBOOK_MAGIC);
    bytes.extend_from_slice(&dimension.to_le_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    for centroid in centroids {
        for component in centroid {
            bytes.extend_from_slice(&component.to_bits().to_le_bytes());
        }
    }
    Ok(bytes)
}

fn encode_sample(sample: &[&Point]) -> Result<Vec<u8>> {
    let count = u32::try_from(sample.len())
        .map_err(|_| Error::Invalid("training sample exceeds u32 rows".into()))?;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(SAMPLE_MAGIC);
    bytes.extend_from_slice(&count.to_le_bytes());
    for point in sample {
        let id = point.id.canonical_bytes();
        let length = u32::try_from(id.len())
            .map_err(|_| Error::Invalid("sample point ID exceeds u32 bytes".into()))?;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(&id);
        let mut hasher = Sha256::new();
        for component in &point.vector {
            hasher.update(component.to_bits().to_le_bytes());
        }
        bytes.extend_from_slice(&hasher.finalize());
    }
    Ok(bytes)
}

fn encode_posting(posting: &[(u16, u32)]) -> Result<Vec<u8>> {
    let count = u32::try_from(posting.len())
        .map_err(|_| Error::Invalid("posting exceeds u32 rows".into()))?;
    let mut bytes = Vec::with_capacity(12 + posting.len() * 6);
    bytes.extend_from_slice(POSTING_MAGIC);
    bytes.extend_from_slice(&count.to_le_bytes());
    for (shard, row) in posting {
        bytes.extend_from_slice(&shard.to_le_bytes());
        bytes.extend_from_slice(&row.to_le_bytes());
    }
    Ok(bytes)
}

fn read_shard(
    repo: &Repository,
    ids_tree: Oid,
    payloads_tree: Oid,
    vectors_tree: Oid,
    shard: u16,
    expected_dimension: usize,
) -> Result<Vec<Point>> {
    let ids = decode_ids(
        &read_blob(repo, ids_tree, &format!("{shard:03x}.bin"))?,
        shard,
    )?;
    let payloads = decode_payloads(&read_blob(
        repo,
        payloads_tree,
        &format!("{shard:03x}.bin"),
    )?)?;
    let vectors = decode_vectors(&read_blob(
        repo,
        vectors_tree,
        &format!("{shard:03x}.f32le"),
    )?)?;
    if ids.is_empty() {
        return Err(Error::Corrupt(
            "empty format-2 point shard must not exist".into(),
        ));
    }
    if vectors
        .first()
        .is_some_and(|vector| vector.len() != expected_dimension)
    {
        return Err(Error::Corrupt(
            "format-2 vector dimension does not match metadata".into(),
        ));
    }
    if ids.len() != payloads.len() || ids.len() != vectors.len() {
        return Err(Error::Corrupt(
            "format-2 shard row counts do not match".into(),
        ));
    }
    Ok(ids
        .into_iter()
        .zip(payloads)
        .zip(vectors)
        .map(|((id, payload), vector)| Point {
            id,
            vector,
            payload,
        })
        .collect())
}

fn decode_ids(bytes: &[u8], shard: u16) -> Result<Vec<PointId>> {
    let sections = decode_offsets(bytes, IDS_MAGIC)?;
    let mut ids = Vec::with_capacity(sections.len());
    for section in sections {
        let (&kind, value) = section
            .split_first()
            .ok_or_else(|| Error::Corrupt("empty format-2 ID row".into()))?;
        let id = match kind {
            0 => {
                if value.len() < 4 {
                    return Err(Error::Corrupt("truncated format-2 string ID".into()));
                }
                let length = read_u32(&value[..4])? as usize;
                if value.len() != 4 + length {
                    return Err(Error::Corrupt("invalid format-2 string ID length".into()));
                }
                PointId::String(
                    std::str::from_utf8(&value[4..])
                        .map_err(|_| Error::Corrupt("format-2 string ID is not UTF-8".into()))?
                        .to_owned(),
                )
            }
            1 if value.len() == 8 => PointId::UInt(u64::from_le_bytes(
                value.try_into().expect("checked uint ID length"),
            )),
            _ => return Err(Error::Corrupt("invalid format-2 ID encoding".into())),
        };
        if shard_for_id(&id) != shard {
            return Err(Error::Corrupt(
                "format-2 ID is stored in the wrong shard".into(),
            ));
        }
        if ids.last().is_some_and(|previous| previous >= &id) {
            return Err(Error::Corrupt(
                "format-2 IDs are not in canonical order".into(),
            ));
        }
        ids.push(id);
    }
    Ok(ids)
}

fn decode_payloads(bytes: &[u8]) -> Result<Vec<JsonObject>> {
    decode_offsets(bytes, PAYLOADS_MAGIC)?
        .into_iter()
        .map(|section| {
            let payload: JsonObject = serde_json::from_slice(section)?;
            if canonical_json(&payload)? != section {
                return Err(Error::Corrupt(
                    "format-2 payload is not canonical JSON".into(),
                ));
            }
            Ok(payload)
        })
        .collect()
}

fn decode_offsets<'a>(bytes: &'a [u8], magic: &[u8; 8]) -> Result<Vec<&'a [u8]>> {
    if bytes.len() < 16 || &bytes[..8] != magic {
        return Err(Error::Corrupt("invalid format-2 offset blob header".into()));
    }
    let count = read_u32(&bytes[8..12])? as usize;
    let offset_bytes = count
        .checked_add(1)
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| Error::Corrupt("format-2 offset table overflows".into()))?;
    let body_start = 12_usize
        .checked_add(offset_bytes)
        .ok_or_else(|| Error::Corrupt("format-2 offset table overflows".into()))?;
    if bytes.len() < body_start {
        return Err(Error::Corrupt("truncated format-2 offset table".into()));
    }
    let mut offsets = Vec::with_capacity(count + 1);
    for chunk in bytes[12..body_start].chunks_exact(4) {
        offsets.push(read_u32(chunk)? as usize);
    }
    let body = &bytes[body_start..];
    if offsets.first() != Some(&0)
        || offsets.last() != Some(&body.len())
        || offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(Error::Corrupt("non-canonical format-2 offsets".into()));
    }
    Ok(offsets
        .windows(2)
        .map(|pair| &body[pair[0]..pair[1]])
        .collect())
}

fn decode_vectors(bytes: &[u8]) -> Result<Vec<Vec<f32>>> {
    if bytes.len() < 16 || &bytes[..8] != VECTORS_MAGIC {
        return Err(Error::Corrupt("invalid format-2 vector header".into()));
    }
    let dimension = read_u32(&bytes[8..12])? as usize;
    let count = read_u32(&bytes[12..16])? as usize;
    if dimension == 0 {
        return Err(Error::Corrupt(
            "format-2 vector dimension must be positive".into(),
        ));
    }
    let expected = count
        .checked_mul(dimension)
        .and_then(|value| value.checked_mul(4))
        .and_then(|value| value.checked_add(16))
        .ok_or_else(|| Error::Corrupt("format-2 vector length overflows".into()))?;
    if bytes.len() != expected {
        return Err(Error::Corrupt(
            "format-2 vector byte length does not match header".into(),
        ));
    }
    let mut vectors = Vec::with_capacity(count);
    for row in bytes[16..].chunks_exact(dimension * 4) {
        let vector = row
            .chunks_exact(4)
            .map(|chunk| f32::from_bits(read_u32(chunk).expect("four-byte vector component")))
            .collect::<Vec<_>>();
        validate_vector_components(&vector).map_err(|error| Error::Corrupt(error.to_string()))?;
        vectors.push(vector);
    }
    Ok(vectors)
}

fn read_index(repo: &Repository, root: Oid, meta: &RootMeta) -> Result<V2SearchIndex> {
    validate_ivf_meta(meta)?;
    let ivf = child_tree(repo, root_tree(repo, root, "index")?, "ivf-flat-v2")?;
    validate_sample(
        &read_blob(repo, ivf, "sample.bin")?,
        meta.point_count.min(SAMPLE_LIMIT),
    )?;
    let centroids = decode_codebook(&read_blob(repo, ivf, "codebook.bin")?, meta.dimension)?;
    let expected = meta
        .ivf
        .as_ref()
        .expect("validated IVF metadata")
        .centroid_count;
    if centroids.len() != expected {
        return Err(Error::Corrupt(
            "format-2 codebook centroid count does not match metadata".into(),
        ));
    }
    let postings_tree = child_tree(repo, ivf, "postings")?;
    let names = blob_names(repo, postings_tree)?;
    if names.len() != expected {
        return Err(Error::Corrupt(
            "format-2 posting count does not match metadata".into(),
        ));
    }
    let mut postings = Vec::with_capacity(expected);
    for (centroid, actual_name) in names.iter().enumerate() {
        let name = format!("{centroid:04x}.bin");
        if actual_name != &name {
            return Err(Error::Corrupt("non-canonical format-2 posting name".into()));
        }
        postings.push(decode_posting(&read_blob(repo, postings_tree, &name)?)?);
    }
    Ok(V2SearchIndex {
        centroids,
        postings,
    })
}

fn decode_codebook(bytes: &[u8], expected_dimension: usize) -> Result<Vec<Vec<f32>>> {
    if bytes.len() < 16 || &bytes[..8] != CODEBOOK_MAGIC {
        return Err(Error::Corrupt("invalid format-2 codebook header".into()));
    }
    let dimension = read_u32(&bytes[8..12])? as usize;
    let count = read_u32(&bytes[12..16])? as usize;
    if dimension == 0 || dimension != expected_dimension {
        return Err(Error::Corrupt(
            "format-2 codebook dimension does not match metadata".into(),
        ));
    }
    let expected = count
        .checked_mul(dimension)
        .and_then(|value| value.checked_mul(4))
        .and_then(|value| value.checked_add(16))
        .ok_or_else(|| Error::Corrupt("format-2 codebook length overflows".into()))?;
    if bytes.len() != expected {
        return Err(Error::Corrupt(
            "format-2 codebook byte length does not match header".into(),
        ));
    }
    let mut centroids = Vec::with_capacity(count);
    for row in bytes[16..].chunks_exact(dimension * 4) {
        let centroid = row
            .chunks_exact(4)
            .map(|chunk| f32::from_bits(read_u32(chunk).expect("four-byte centroid component")))
            .collect::<Vec<_>>();
        validate_vector_components(&centroid).map_err(|error| Error::Corrupt(error.to_string()))?;
        centroids.push(centroid);
    }
    Ok(centroids)
}

fn validate_sample(bytes: &[u8], expected_count: usize) -> Result<()> {
    if bytes.len() < 12 || &bytes[..8] != SAMPLE_MAGIC {
        return Err(Error::Corrupt("invalid format-2 sample header".into()));
    }
    let count = read_u32(&bytes[8..12])? as usize;
    if count != expected_count {
        return Err(Error::Corrupt(
            "format-2 sample count does not match metadata".into(),
        ));
    }
    let mut cursor = 12_usize;
    for _ in 0..count {
        let length_end = cursor
            .checked_add(4)
            .ok_or_else(|| Error::Corrupt("format-2 sample length overflows".into()))?;
        if length_end > bytes.len() {
            return Err(Error::Corrupt("truncated format-2 sample ID length".into()));
        }
        let length = read_u32(&bytes[cursor..length_end])? as usize;
        cursor = length_end;
        let id_end = cursor
            .checked_add(length)
            .ok_or_else(|| Error::Corrupt("format-2 sample ID length overflows".into()))?;
        let row_end = id_end
            .checked_add(32)
            .ok_or_else(|| Error::Corrupt("format-2 sample row length overflows".into()))?;
        if row_end > bytes.len() || length < 2 {
            return Err(Error::Corrupt("truncated format-2 sample row".into()));
        }
        let id = &bytes[cursor..id_end];
        match &id[..2] {
            b"s\0" => {
                std::str::from_utf8(&id[2..])
                    .map_err(|_| Error::Corrupt("format-2 sample string ID is not UTF-8".into()))?;
            }
            b"u\0" if id.len() == 10 => {}
            _ => return Err(Error::Corrupt("invalid format-2 sample ID".into())),
        }
        cursor = row_end;
    }
    if cursor != bytes.len() {
        return Err(Error::Corrupt(
            "format-2 sample contains trailing bytes".into(),
        ));
    }
    Ok(())
}

fn decode_posting(bytes: &[u8]) -> Result<Vec<(u16, u32)>> {
    if bytes.len() < 12 || &bytes[..8] != POSTING_MAGIC {
        return Err(Error::Corrupt("invalid format-2 posting header".into()));
    }
    let count = read_u32(&bytes[8..12])? as usize;
    let expected = count
        .checked_mul(6)
        .and_then(|value| value.checked_add(12))
        .ok_or_else(|| Error::Corrupt("format-2 posting length overflows".into()))?;
    if bytes.len() != expected {
        return Err(Error::Corrupt(
            "format-2 posting byte length does not match header".into(),
        ));
    }
    let mut posting = Vec::with_capacity(count);
    for entry in bytes[12..].chunks_exact(6) {
        let shard = u16::from_le_bytes(entry[..2].try_into().expect("two-byte shard"));
        let row = read_u32(&entry[2..])?;
        if usize::from(shard) >= SHARD_COUNT
            || posting
                .last()
                .is_some_and(|previous| previous >= &(shard, row))
        {
            return Err(Error::Corrupt(
                "non-canonical format-2 posting order".into(),
            ));
        }
        posting.push((shard, row));
    }
    Ok(posting)
}

fn write_blob_tree(repo: &Repository, entries: Vec<(String, Vec<u8>)>) -> Result<Oid> {
    let mut tree = repo.treebuilder(None)?;
    for (name, bytes) in entries {
        tree.insert(name, repo.blob(&bytes)?, BLOB_MODE)?;
    }
    Ok(tree.write()?)
}

fn validate_tree_layout(repo: &Repository, root: Oid) -> Result<()> {
    validate_entries(
        repo,
        root,
        &[
            ("index", ObjectType::Tree),
            ("meta.json", ObjectType::Blob),
            ("points", ObjectType::Tree),
        ],
    )?;
    let points = root_tree(repo, root, "points")?;
    validate_entries(
        repo,
        points,
        &[
            ("ids", ObjectType::Tree),
            ("payloads", ObjectType::Tree),
            ("vectors", ObjectType::Tree),
        ],
    )?;
    let index = root_tree(repo, root, "index")?;
    validate_entries(repo, index, &[("ivf-flat-v2", ObjectType::Tree)])?;
    let ivf = child_tree(repo, index, "ivf-flat-v2")?;
    validate_entries(
        repo,
        ivf,
        &[
            ("codebook.bin", ObjectType::Blob),
            ("postings", ObjectType::Tree),
            ("sample.bin", ObjectType::Blob),
        ],
    )
}

fn validate_entries(repo: &Repository, tree: Oid, expected: &[(&str, ObjectType)]) -> Result<()> {
    let tree = repo.find_tree(tree)?;
    if tree.len() != expected.len() {
        return Err(Error::Corrupt(
            "format-2 tree contains missing or extra entries".into(),
        ));
    }
    for (name, kind) in expected {
        let entry = tree
            .get_name(name)
            .ok_or_else(|| Error::Corrupt(format!("missing format-2 tree entry {name}")))?;
        if entry.kind() != Some(*kind) {
            return Err(Error::Corrupt(format!(
                "format-2 tree entry {name} has the wrong kind"
            )));
        }
    }
    Ok(())
}

fn root_tree(repo: &Repository, root: Oid, name: &str) -> Result<Oid> {
    child_tree(repo, root, name)
}

fn child_tree(repo: &Repository, parent: Oid, name: &str) -> Result<Oid> {
    let tree = repo.find_tree(parent)?;
    let entry = tree
        .get_name(name)
        .ok_or_else(|| Error::Corrupt(format!("missing tree path component {name}")))?;
    if entry.kind() != Some(ObjectType::Tree) {
        return Err(Error::Corrupt(format!(
            "tree path component {name} is not a tree"
        )));
    }
    Ok(entry.id())
}

fn read_blob(repo: &Repository, tree: Oid, name: &str) -> Result<Vec<u8>> {
    let tree = repo.find_tree(tree)?;
    read_named_blob(repo, &tree, name)
}

fn read_named_blob(repo: &Repository, tree: &Tree<'_>, name: &str) -> Result<Vec<u8>> {
    let entry = tree
        .get_name(name)
        .ok_or_else(|| Error::Corrupt(format!("missing {name}")))?;
    if entry.kind() != Some(ObjectType::Blob) {
        return Err(Error::Corrupt(format!("{name} is not a blob")));
    }
    Ok(repo.find_blob(entry.id())?.content().to_vec())
}

fn blob_oid(tree: &Tree<'_>, name: &str) -> Result<Oid> {
    let entry = tree
        .get_name(name)
        .ok_or_else(|| Error::Corrupt(format!("missing {name}")))?;
    if entry.kind() != Some(ObjectType::Blob) {
        return Err(Error::Corrupt(format!("{name} is not a blob")));
    }
    Ok(entry.id())
}

fn blob_names(repo: &Repository, tree: Oid) -> Result<Vec<String>> {
    let tree = repo.find_tree(tree)?;
    let mut names = Vec::with_capacity(tree.len());
    for entry in &tree {
        if entry.kind() != Some(ObjectType::Blob) {
            return Err(Error::Corrupt(
                "format-2 blob tree contains a non-blob entry".into(),
            ));
        }
        names.push(
            entry
                .name()
                .map_err(|_| Error::Corrupt("Git tree name is not UTF-8".into()))?
                .to_owned(),
        );
    }
    Ok(names)
}

fn parse_shard_name(name: &str, suffix: &str) -> Result<u16> {
    let hex = name
        .strip_suffix(suffix)
        .filter(|hex| hex.len() == 3)
        .ok_or_else(|| Error::Corrupt("invalid format-2 shard name".into()))?;
    let shard = u16::from_str_radix(hex, 16)
        .map_err(|_| Error::Corrupt("invalid format-2 shard name".into()))?;
    if usize::from(shard) >= SHARD_COUNT || format!("{shard:03x}{suffix}") != name {
        return Err(Error::Corrupt("non-canonical format-2 shard name".into()));
    }
    Ok(shard)
}

fn read_u32(bytes: &[u8]) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| {
        Error::Corrupt("truncated format-2 u32".into())
    })?))
}

fn cosine_f64(left: &[f32], right: &[f32]) -> f64 {
    let mut dot = 0.0_f64;
    let mut left_norm = 0.0_f64;
    let mut right_norm = 0.0_f64;
    for (&left, &right) in left.iter().zip(right) {
        let left = f64::from(left);
        let right = f64::from(right);
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

fn equal_vectors(left: &[Vec<f32>], right: &[Vec<f32>]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| left.to_bits() == right.to_bits())
        })
}

struct RankedPoint {
    ranking_score: f64,
    point: ScoredPoint,
}

fn compare_ranked(left: &RankedPoint, right: &RankedPoint) -> Ordering {
    right
        .ranking_score
        .total_cmp(&left.ranking_score)
        .then_with(|| left.point.id.cmp(&right.point.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn point(id: impl Into<PointId>, vector: [f32; 2]) -> Point {
        Point {
            id: id.into(),
            vector: vector.to_vec(),
            payload: json!({"kind": "test"}).as_object().unwrap().clone(),
        }
    }

    #[test]
    fn codecs_round_trip_typed_ids_and_reject_corruption() {
        let points = [point("a", [1.0, 0.0]), point(7_u64, [0.0, 1.0])];
        for group in points
            .iter()
            .collect::<Vec<_>>()
            .chunks(1)
            .map(|chunk| chunk.to_vec())
        {
            let shard = shard_for_id(&group[0].id);
            assert_eq!(
                decode_ids(&encode_ids(&group).unwrap(), shard).unwrap()[0],
                group[0].id
            );
            assert_eq!(
                decode_payloads(&encode_payloads(&group).unwrap()).unwrap()[0],
                group[0].payload
            );
            assert_eq!(
                decode_vectors(&encode_vectors(&group, 2).unwrap()).unwrap()[0],
                group[0].vector
            );
        }
        assert!(decode_vectors(b"broken").is_err());
        assert!(decode_posting(b"broken").is_err());
        assert_eq!(centroid_count(usize::MAX), MAX_CENTROIDS);
    }

    #[test]
    fn clean_builds_are_canonical_and_fully_valid() {
        let left = TempDir::new().unwrap();
        let right = TempDir::new().unwrap();
        let left = Repository::init_bare(left.path()).unwrap();
        let right = Repository::init_bare(right.path()).unwrap();
        let config = CollectionConfig::new(2);
        let points = (0..16)
            .map(|index| {
                point(
                    format!("point-{index:02}"),
                    [index as f32 - 7.0, ((index * 7) % 11) as f32 - 5.0],
                )
            })
            .collect::<Vec<_>>();
        let first = points
            .iter()
            .cloned()
            .map(|point| (point.id.clone(), point))
            .collect::<BTreeMap<_, _>>();
        let second = points
            .into_iter()
            .rev()
            .map(|point| (point.id.clone(), point))
            .collect::<BTreeMap<_, _>>();
        let left_root = build_root(&left, &config, &first).unwrap();
        let right_root = build_root(&right, &config, &second).unwrap();
        assert_eq!(left_root, right_root);
        assert_eq!(
            left_root.to_string(),
            "584f3d35bab91a82a2900e0c7bdcd9d059d69810"
        );
        let meta = crate::root::read_meta(&left, left_root).unwrap();
        assert_eq!(meta.format_version, 2);
        assert!(validate_root(&left, left_root, &meta, true).unwrap().valid);
    }
}
