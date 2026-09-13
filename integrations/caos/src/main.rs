//! Caos worker: orchestration ends in continuations, so waiting holds no slot.
use git_vdb::{ObjectId, Query, SnapshotReader, SnapshotSource};
use git_vdb_caos::*;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::{Command, ExitCode};

fn caos(args: &[&str]) -> Result<String> {
    let output = Command::new(std::env::var("CAOS_BIN").unwrap_or_else(|_| "/bin/caos".into()))
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "caos {}: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
fn get(path: &str) -> Result<()> {
    caos(&["get", path])?;
    Ok(())
}
fn hash(path: &str) -> Result<String> {
    caos(&["hash", path])
}
fn arg(name: &str) -> String {
    format!("/cas/args/{name}")
}
fn optional(name: &str) -> Result<Option<String>> {
    let path = arg(name);
    if !Path::new(&path).exists() {
        return Ok(None);
    }
    get(&path)?;
    Ok(Some(fs::read_to_string(path)?))
}
fn required(name: &str) -> Result<String> {
    optional(name)?.ok_or_else(|| format!("missing {name}").into())
}
fn read_json<T: DeserializeOwned>(path: &str) -> Result<T> {
    get(path)?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn curry(stage: &str, args: &[String]) -> Result<String> {
    let mut tokens = vec![
        "curry".into(),
        "--base:@=/cas/args/base".into(),
        format!("--stage={stage}"),
    ];
    tokens.extend_from_slice(args);
    if let Some(salt) = optional("run-salt")? { tokens.push(format!("--run-salt={salt}")); }
    caos(&tokens.iter().map(String::as_str).collect::<Vec<_>>())
}
fn run_then(input: &str, run: &str, then: &str) -> Result<()> {
    caos(&[
        "run-then",
        input,
        &format!("--run:hash={run}"),
        &format!("--then:hash={then}"),
    ])?;
    Ok(())
}
fn put(path: &Path, output: &str) -> Result<()> {
    caos(&["put", path.to_str().ok_or("non-UTF8 output path")?, output])?;
    Ok(())
}
fn failure(message: &str) -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("report"), format!("FAILED: {message}\n"))?;
    fs::write(
        temp.path().join("results.json"),
        serde_json::to_vec(&json!({"error":message}))?,
    )?;
    put(temp.path(), "/cas/out")
}
fn start() -> Result<()> {
    let query = match optional("query")? {
        Some(query) if !query.trim().is_empty() && query.len() <= 16384 => query,
        _ => return failure("query must contain between 1 and 16384 UTF-8 bytes"),
    };
    let limit = match optional("limit")?
        .unwrap_or_else(|| "10".into())
        .parse::<usize>()
    {
        Ok(limit) if (1..=100).contains(&limit) => limit,
        _ => return failure("limit must be between 1 and 100"),
    };
    let chunk_chars = match optional("chunk-chars")?
        .unwrap_or_else(|| DEFAULT_CHUNK_CHARS.to_string())
        .parse::<usize>()
    {
        Ok(size) if (1..=16384).contains(&size) => size,
        _ => return failure("chunk-chars must be between 1 and 16384"),
    };
    let scope = optional("path")?.unwrap_or_else(|| ".".into());
    if scope != "."
        && (scope.is_empty()
            || Path::new(&scope)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_))))
    {
        return failure("path must be a relative source path without parent traversal");
    }
    if scope != "." && Path::new(&scope).components().any(|part| {
        matches!(part, std::path::Component::Normal(name) if name.to_str().is_some_and(excluded_directory))
    }) { return failure("selected path is an excluded generated or metadata directory"); }
    let original = hash(&arg("in"))?;
    let source_commit = if caos(&["kind", &arg("in")])? == "commit" {
        original.clone()
    } else {
        String::new()
    };
    caos(&["resolve", &original, ".", "/cas/source"])?;
    let source_tree = hash("/cas/source")?;
    if let Err(error) = caos(&["resolve", &source_tree, &scope, "/cas/selected"]) {
        let message = error.to_string();
        if message.contains(&format!("no such path: {scope}"))
            || message.contains("traverses a non-directory")
        {
            return failure("selected path is absent from the source snapshot");
        }
        return Err(error);
    }
    let index = curry("index", &[format!("--chunk-chars={chunk_chars}")])?;
    let query = curry(
        "query",
        &[
            format!("--query={query}"),
            format!("--limit={limit}"),
            format!("--source-tree={source_tree}"),
            format!("--source-commit={source_commit}"),
            format!("--scope={scope}"),
        ],
    )?;
    run_then("/cas/selected", &index, &query)
}
fn collect(
    source: &str,
    relative: &str,
    files: &mut Vec<FileLocation>,
    blobs: &mut BTreeMap<String, String>,
    skipped: &mut Vec<String>,
) -> Result<()> {
    if fs::symlink_metadata(source)?.file_type().is_symlink() {
        skipped.push(format!("{relative}: symlink"));
        return Ok(());
    }
    if caos(&["kind", source])? == "commit" {
        skipped.push(format!("{relative}: nested commit reference"));
        return Ok(());
    }
    get(source)?;
    let path = Path::new(source);
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        skipped.push(format!("{relative}: symlink"));
        return Ok(());
    }
    if path.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "non-UTF8 source path")?;
            let next = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if excluded_directory(&name) {
                skipped.push(format!("{next}: excluded"));
                continue;
            }
            let child = entry.path();
            if child.is_dir() || selected_file(&next) {
                collect(
                    child.to_str().ok_or("non-UTF8 CAS path")?,
                    &next,
                    files,
                    blobs,
                    skipped,
                )?;
            }
        }
    } else {
        let bytes = fs::read(path)?;
        if bytes.len() > MAX_FILE_BYTES
            || bytes.contains(&0)
            || std::str::from_utf8(&bytes).is_err()
        {
            skipped.push(format!(
                "{relative}: binary, invalid UTF-8, or over 256 KiB"
            ));
            return Ok(());
        }
        let blob = hash(source)?;
        files.push(FileLocation {
            path: relative.to_owned(),
            blob: blob.clone(),
        });
        blobs.entry(blob).or_insert_with(|| source.to_owned());
    }
    Ok(())
}
fn index() -> Result<()> {
    let chunk_chars: usize = required("chunk-chars")?.parse()?;
    let mut manifest = Manifest {
        source_tree: hash(&arg("in"))?,
        files: Vec::new(),
        skipped: Vec::new(),
        model: embedding_identity()?,
        dimension: 384,
        chunk_chars,
    };
    let mut blobs = BTreeMap::new();
    collect(
        &arg("in"),
        "",
        &mut manifest.files,
        &mut blobs,
        &mut manifest.skipped,
    )?;
    let temp = tempfile::tempdir()?;
    let inputs = temp.path().join("inputs");
    fs::create_dir(&inputs)?;
    for (blob, path) in blobs {
        symlink(path, inputs.join(blob))?;
    }
    let meta = temp.path().join("manifest.json");
    fs::write(&meta, serde_json::to_vec(&manifest)?)?;
    put(&meta, "/cas/manifest")?;
    put(&inputs, "/cas/files")?;
    let embed = curry("embed", &[format!("--chunk-chars={chunk_chars}")])?;
    let assemble = curry("assemble", &["--manifest:@=/cas/manifest".into()])?;
    caos(&[
        "map-then",
        "/cas/files",
        &format!("--map:hash={embed}"),
        &format!("--then:hash={assemble}"),
        "--max-parallel=2",
    ])?;
    Ok(())
}
fn embed_file() -> Result<()> {
    get(&arg("in"))?;
    let chunks = embedded_chunks(
        &fs::read_to_string(arg("in"))?,
        required("chunk-chars")?.parse()?,
    )?;
    let temp = tempfile::tempdir()?;
    let file = temp.path().join("embedding.json");
    fs::write(&file, serde_json::to_vec(&chunks)?)?;
    put(&file, "/cas/out")
}
fn assemble() -> Result<()> {
    let manifest: Manifest = read_json(&arg("manifest"))?;
    let mut embedded = BTreeMap::new();
    for file in &manifest.files {
        if !embedded.contains_key(&file.blob) {
            get(&arg("children"))?;
            let chunks = read_json(&format!("{}/{}", arg("children"), file.blob))?;
            embedded.insert(file.blob.clone(), chunks);
        }
    }
    let temp = tempfile::tempdir()?;
    build_directory(temp.path(), &manifest, &embedded)?;
    put(temp.path(), "/cas/out")
}
fn query() -> Result<()> {
    let query = required("query")?;
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("query"), query)?;
    put(&temp.path().join("query"), "/cas/query")?;
    let embed = curry("embed-query", &[])?;
    let search = curry(
        "search",
        &[
            "--index:@=/cas/args/result".into(),
            format!("--limit={}", required("limit")?),
            format!("--source-tree={}", required("source-tree")?),
            format!("--source-commit={}", required("source-commit")?),
            format!("--scope={}", required("scope")?),
        ],
    )?;
    run_then("/cas/query", &embed, &search)
}
fn embed_query() -> Result<()> {
    get(&arg("in"))?;
    let result = embed(&[fs::read_to_string(arg("in"))?])?;
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("vector"), serde_json::to_vec(&result[0])?)?;
    put(&temp.path().join("vector"), "/cas/out")
}
// Materialize each ancestor once, then only requested leaf objects. Caos keeps
// the recorded root/hash binding; no temporary Git import is involved.
struct CaosSource {
    root: String,
    loaded: RefCell<BTreeSet<String>>,
    trace: RefCell<Vec<Value>>,
}
impl CaosSource {
    fn load(&self, relative: &str) -> git_vdb::Result<String> {
        let mut path = self.root.clone();
        let mut paths = vec![path.clone()];
        for part in Path::new(relative).components() {
            let std::path::Component::Normal(name) = part else {
                return Err(git_vdb::Error::Invalid("invalid object path".into()));
            };
            path.push('/');
            path.push_str(
                name.to_str()
                    .ok_or_else(|| git_vdb::Error::Invalid("non-UTF8 path".into()))?,
            );
            paths.push(path.clone());
        }
        for path in paths {
            if !self.loaded.borrow().contains(&path) {
                get(&path).map_err(|error| git_vdb::Error::Invalid(error.to_string()))?;
                let object =
                    hash(&path).map_err(|error| git_vdb::Error::Invalid(error.to_string()))?;
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.file_type().is_symlink() {
                    return Err(git_vdb::Error::Corrupt("symlink in snapshot".into()));
                }
                self.trace.borrow_mut().push(
                    json!({"path":path.strip_prefix(&self.root).unwrap_or(&path),
                    "object":object, "kind":if metadata.is_dir() { "tree" } else { "blob" },
                    "blob_bytes":if metadata.is_file() { metadata.len() } else { 0 }}),
                );
                self.loaded.borrow_mut().insert(path);
            }
        }
        Ok(path)
    }
}
impl SnapshotSource for CaosSource {
    fn read_blob(&self, path: &str) -> git_vdb::Result<Vec<u8>> {
        Ok(fs::read(self.load(path)?)?)
    }
    fn list(&self, path: &str) -> git_vdb::Result<Vec<String>> {
        fs::read_dir(self.load(path)?)?
            .map(|entry| {
                entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| git_vdb::Error::Corrupt("non-UTF8 snapshot entry".into()))
            })
            .collect()
    }
}

fn search() -> Result<()> {
    get(&arg("index"))?;
    let manifest: Manifest = read_json(&format!("{}/manifest.json", arg("index")))?;
    let model = embedding_identity()?;
    if manifest.model != model {
        return Err("query model does not match indexed model".into());
    }
    let vector: Vec<f32> = read_json(&arg("result"))?;
    let root = format!("{}/index", arg("index"));
    let source = CaosSource {
        root: root.clone(),
        loaded: RefCell::new(BTreeSet::new()),
        trace: RefCell::new(vec![]),
    };
    let mut reader = SnapshotReader::open(ObjectId(hash(&root)?), source)?;
    let found = reader.query(
        Query::new(vector, required("limit")?.parse()?)
            .in_vector_space(&model)
            .with_payload(),
    )?;
    let mut result = json!({"source_tree":manifest.source_tree, "index_root":found.root,
        "model":model, "hits":found.points, "stats":found.stats,
        "reads":reader.read_stats(), "objects":reader.source().trace.borrow().clone()});
    result["source_tree"] = Value::String(required("source-tree")?);
    let source_commit = required("source-commit")?;
    result["source_commit"] = if source_commit.is_empty() {
        Value::Null
    } else {
        Value::String(source_commit)
    };
    let scope = required("scope")?;
    let mut report = String::new();
    for (rank, hit) in result["hits"]
        .as_array_mut()
        .ok_or("query hits missing")?
        .iter_mut()
        .enumerate()
    {
        let relative = hit["payload"]["path"].as_str().ok_or("hit path missing")?;
        let path = if scope == "." {
            relative.to_owned()
        } else if relative.is_empty() {
            scope.clone()
        } else {
            format!("{scope}/{relative}")
        };
        hit["payload"]["path"] = Value::String(path.clone());
        report.push_str(&format!(
            "{}. {}:{}-{} (score {})\n{}\n\n",
            rank + 1,
            path,
            hit["payload"]["start_line"],
            hit["payload"]["end_line"],
            hit["score"],
            hit["payload"]["document"].as_str().unwrap_or("")
        ));
    }
    report.push_str(&format!(
        "{} matches in source {} (index {}).\n",
        result["hits"].as_array().ok_or("query hits missing")?.len(),
        result["source_tree"].as_str().unwrap_or(""),
        result["index_root"].as_str().unwrap_or("")
    ));
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("results.json"),
        serde_json::to_vec(&result)?,
    )?;
    // Pinned Caos treats the reserved uppercase marker anywhere in a report
    // as a failure. Exact source text remains untouched in results.json.
    fs::write(
        temp.path().join("report"),
        report.replace("FAILED", "Failed"),
    )?;
    // A real subtree keeps the snapshot reachable through the returned object.
    symlink(arg("index"), temp.path().join("snapshot"))?;
    put(temp.path(), "/cas/out")
}
fn run() -> Result<()> {
    match optional("stage")?.as_deref() {
        None | Some("start") => start(),
        Some("index") => index(),
        Some("embed") => embed_file(),
        Some("assemble") => assemble(),
        Some("query") => query(),
        Some("embed-query") => embed_query(),
        Some("search") => search(),
        Some(stage) => Err(format!("unknown stage {stage}").into()),
    }
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("git-vdb-caos: {error}");
            ExitCode::FAILURE
        }
    }
}
