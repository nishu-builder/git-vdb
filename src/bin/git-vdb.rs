use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;
use git_vdb::*;
use serde::Serialize;
use serde_json::{json, Value};
#[cfg(feature = "fastembed")]
use std::fs;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

#[derive(Parser)]
#[command(name = "git-vdb", version, about)]
struct Cli {
    /// Database directory. Overrides GIT_VDB_REPO.
    #[arg(long = "db", visible_alias = "repo", global = true)]
    db: Option<PathBuf>,
    /// Machine-readable output encoding.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json, global = true)]
    format: OutputFormat,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a Git repository for git-vdb.
    Init {
        repo: PathBuf,
        #[arg(long)]
        bare: bool,
    },
    /// Manage collections.
    Collection {
        #[command(subcommand)]
        command: CollectionCommand,
    },
    /// Insert or replace points from JSON Lines.
    Upsert {
        collection: String,
        /// JSON Lines file, or - for stdin.
        #[arg(value_name = "FILE", conflicts_with_all = ["legacy_input", "id", "vector"])]
        input: Option<PathBuf>,
        /// Compatibility spelling for the input file.
        #[arg(long = "input", hide = true, conflicts_with_all = ["input", "id", "vector"])]
        legacy_input: Option<PathBuf>,
        /// Typed JSON ID or plain string for one inline point.
        #[arg(long, requires = "vector")]
        id: Option<String>,
        /// Inline comma-separated or JSON vector for one point.
        #[arg(long, requires = "id")]
        vector: Option<String>,
        /// Inline JSON object payload for one point.
        #[arg(long, requires = "id")]
        payload: Option<String>,
        #[arg(long)]
        expect_root: Option<String>,
        /// Maximum JSON Lines points held in memory per write.
        #[arg(long, default_value_t = 1_000)]
        batch_size: usize,
        /// Report completed batches to stderr.
        #[arg(long)]
        progress: bool,
    },
    /// Retrieve points without similarity ranking.
    Get {
        collection: String,
        #[arg(long, num_args = 1..)]
        ids: Vec<String>,
        #[arg(long)]
        /// Filter JSON, a JSON file, or - for stdin.
        filter: Option<String>,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long)]
        limit: Option<usize>,
        #[arg(long)]
        with_payload: bool,
        #[arg(long)]
        with_vector: bool,
        #[arg(long)]
        at: Option<String>,
    },
    /// Delete points by IDs or filter.
    Delete {
        collection: String,
        #[arg(long, num_args = 1.., conflicts_with = "filter")]
        ids: Vec<String>,
        #[arg(long, conflicts_with = "ids")]
        /// Filter JSON, a JSON file, or - for stdin.
        filter: Option<String>,
        #[arg(long)]
        expect_root: Option<String>,
    },
    /// Count points, optionally under a filter.
    Count {
        collection: String,
        #[arg(long)]
        /// Filter JSON, a JSON file, or - for stdin.
        filter: Option<String>,
        #[arg(long)]
        at: Option<String>,
    },
    /// Search by cosine similarity.
    #[command(name = "search", alias = "query")]
    Query {
        collection: String,
        /// Query vector, inline or from a JSON file.
        #[arg(long, conflicts_with = "text", required_unless_present = "text")]
        vector: Option<String>,
        /// Text query embedded with the bundled local model.
        #[arg(long, conflicts_with = "vector", required_unless_present = "vector")]
        text: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        /// Filter JSON, a JSON file, or - for stdin.
        filter: Option<String>,
        #[arg(long)]
        exact: bool,
        #[arg(long)]
        approximate: bool,
        #[arg(long, default_value_t = 0)]
        probes: usize,
        #[arg(long, default_value_t = 0)]
        candidate_limit: usize,
        #[arg(long)]
        with_payload: bool,
        #[arg(long)]
        with_vector: bool,
        #[arg(long)]
        expected_vector_space: Option<String>,
        #[arg(long)]
        at: Option<String>,
    },
    /// Index UTF-8 files for local semantic text search.
    Index {
        collection: String,
        #[arg(required = true, num_args = 1..)]
        paths: Vec<PathBuf>,
        /// Approximate maximum characters per stable file chunk.
        #[arg(long, default_value_t = 2_000)]
        chunk_chars: usize,
    },
    /// Show collection commit history.
    History {
        collection: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Restore a historical collection revision as a new commit.
    Restore {
        collection: String,
        revision: String,
    },
    /// Push one collection ref to a configured Git remote.
    Push {
        collection: String,
        #[arg(default_value = "origin")]
        remote: String,
    },
    /// Fast-forward one collection ref from a configured Git remote.
    Pull {
        collection: String,
        #[arg(default_value = "origin")]
        remote: String,
    },
    /// Compare two immutable collection roots or commits.
    Diff {
        collection: String,
        left_root: String,
        right_root: String,
    },
    /// Validate the current or historical collection root.
    Validate {
        collection: String,
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        full: bool,
    },
    /// Inspect the database and verify every current collection root.
    Doctor,
    /// Generate shell completion definitions on stdout.
    Completions { shell: Shell },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Json,
    Pretty,
    Jsonl,
}

#[derive(Subcommand)]
enum CollectionCommand {
    Create(CreateCollection),
    List,
    Info {
        name: String,
        #[arg(long)]
        at: Option<String>,
    },
    Delete {
        name: String,
    },
}

#[derive(Args)]
struct CreateCollection {
    name: String,
    #[arg(long)]
    dimension: usize,
    #[arg(long, value_enum, default_value_t = DistanceArg::Cosine)]
    distance: DistanceArg,
    #[arg(long)]
    vector_space: Option<String>,
    #[arg(long, default_value_t = 1_000)]
    full_scan_threshold: usize,
    #[arg(long, default_value_t = 96)]
    default_probes: usize,
    #[arg(long, default_value_t = 10_000)]
    default_candidate_limit: usize,
}

#[derive(Clone, Copy, ValueEnum)]
enum DistanceArg {
    Cosine,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("git-vdb: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let output_format = cli.format;
    if let Command::Completions { shell } = &cli.command {
        clap_complete::generate(*shell, &mut Cli::command(), "git-vdb", &mut io::stdout());
        return Ok(());
    }
    if let Command::Init { repo, bare } = cli.command {
        let database = Database::init_with_options(&repo, bare)?;
        print_value(
            &json!({
                "repository": repo,
                "bare": bare,
                "collections": database.list_collections()?,
            }),
            output_format,
        )?;
        return Ok(());
    }

    let repo_path = cli
        .db
        .or_else(|| std::env::var_os("GIT_VDB_REPO").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".git-vdb"));
    let mutating = matches!(
        &cli.command,
        Command::Collection {
            command: CollectionCommand::Create(_) | CollectionCommand::Delete { .. }
        } | Command::Upsert { .. }
            | Command::Index { .. }
            | Command::Delete { .. }
            | Command::Restore { .. }
            | Command::Pull { .. }
    );
    let store = mutating.then(|| Store::open(&repo_path)).transpose()?;
    let database = match &store {
        Some(store) => store.advanced().clone(),
        None => Database::open(&repo_path)?,
    };
    match cli.command {
        Command::Init { .. } => unreachable!(),
        Command::Collection { command } => match command {
            CollectionCommand::Create(args) => {
                let collection = database.create_collection(
                    &args.name,
                    CollectionConfig {
                        dimension: args.dimension,
                        distance: Distance::Cosine,
                        vector_space: args.vector_space,
                        index: IndexConfig {
                            full_scan_threshold: args.full_scan_threshold,
                            default_probes: args.default_probes,
                            default_candidate_limit: args.default_candidate_limit,
                            ..IndexConfig::default()
                        },
                    },
                )?;
                print_value(&collection.info()?, output_format)?;
            }
            CollectionCommand::List => {
                let collections = database
                    .list_collections()?
                    .into_iter()
                    .map(|name| database.collection(&name)?.info())
                    .collect::<Result<Vec<_>>>()?;
                print_value(&json!({"collections": collections}), output_format)?;
            }
            CollectionCommand::Info { name, at } => {
                let collection = at_collection(database.collection(name)?, at.as_deref())?;
                print_value(&collection.info()?, output_format)?;
            }
            CollectionCommand::Delete { name } => {
                let root = database.delete_collection(&name)?;
                print_value(
                    &json!({"name": name, "root": root, "deleted": true}),
                    output_format,
                )?;
            }
        },
        Command::Upsert {
            collection,
            input,
            legacy_input,
            id,
            vector,
            payload,
            expect_root,
            batch_size,
            progress,
        } => {
            let expected = parse_object_id(expect_root)?;
            let store = store.as_ref().expect("upsert opens a store");
            let result = match (input.or(legacy_input), id, vector) {
                (Some(path), None, None) => stream_upsert(
                    store,
                    &database,
                    &collection,
                    &path,
                    expected,
                    batch_size,
                    progress,
                )?,
                (None, Some(id), Some(vector)) => {
                    let points = inline_point(id, vector, payload)?;
                    match expected {
                        Some(expected) => database
                            .collection(collection)?
                            .upsert_expect(points, Some(expected))?,
                        None => store.collection(collection).upsert(points)?,
                    }
                }
                (None, None, None) => {
                    return Err(Error::Invalid(
                        "upsert requires a FILE, or both --id and --vector".into(),
                    ))
                }
                _ => {
                    return Err(Error::Invalid(
                        "upsert accepts either a FILE or one --id/--vector point".into(),
                    ))
                }
            };
            print_value(&result, output_format)?;
        }
        Command::Get {
            collection,
            ids,
            filter,
            offset,
            limit,
            with_payload,
            with_vector,
            at,
        } => {
            let collection = at_collection(database.collection(collection)?, at.as_deref())?;
            print_value(
                &collection.get(GetRequest {
                    ids: parse_ids(ids)?,
                    filter: read_optional_json_arg(filter.as_deref())?,
                    offset,
                    limit,
                    with_payload,
                    with_vector,
                })?,
                output_format,
            )?;
        }
        Command::Delete {
            collection,
            ids,
            filter,
            expect_root,
        } => {
            let selector = DeleteSelector {
                ids: parse_ids(ids)?,
                filter: read_optional_json_arg(filter.as_deref())?,
            };
            let expected = parse_object_id(expect_root)?;
            print_value(
                &database
                    .collection(collection)?
                    .delete_expect(selector, expected)?,
                output_format,
            )?;
        }
        Command::Count {
            collection,
            filter,
            at,
        } => {
            let collection = at_collection(database.collection(collection)?, at.as_deref())?;
            print_value(
                &collection.count(read_optional_json_arg(filter.as_deref())?)?,
                output_format,
            )?;
        }
        Command::Query {
            collection,
            vector,
            text,
            limit,
            filter,
            exact,
            approximate,
            probes,
            candidate_limit,
            with_payload,
            with_vector,
            expected_vector_space,
            at,
        } => {
            if exact && approximate {
                return Err(Error::Invalid(
                    "--exact and --approximate are mutually exclusive".into(),
                ));
            }
            let filter = read_optional_json_arg(filter.as_deref())?;
            let params = QueryParams {
                exact: exact
                    .then_some(true)
                    .or_else(|| approximate.then_some(false)),
                probes,
                candidate_limit,
            };
            match (vector, text) {
                (Some(vector), None) => {
                    let collection =
                        at_collection(database.collection(collection)?, at.as_deref())?;
                    print_value(
                        &collection.query(Query {
                            vector: parse_vector_arg(&vector)?,
                            limit,
                            filter,
                            with_payload,
                            with_vector,
                            expected_vector_space,
                            params,
                        })?,
                        output_format,
                    )?;
                }
                (None, Some(text)) => {
                    if at.is_some() || with_vector || expected_vector_space.is_some() {
                        return Err(Error::Invalid(
                            "text search does not support --at, --with-vector, or --expected-vector-space"
                                .into(),
                        ));
                    }
                    let hits = search_local_text(
                        &repo_path,
                        collection,
                        TextQuery {
                            text,
                            limit,
                            filter,
                            params,
                        },
                    )?;
                    print_value(&hits, output_format)?;
                }
                _ => unreachable!("clap requires exactly one query input"),
            }
        }
        Command::Index {
            collection,
            paths,
            chunk_chars,
        } => {
            let report = index_local_text(&repo_path, collection, paths, chunk_chars)?;
            print_value(&report, output_format)?;
        }
        Command::History { collection, limit } => {
            let collection = database.collection(collection)?;
            let root = collection.root()?;
            print_value(
                &json!({"root": root, "history": collection.history(limit)?}),
                output_format,
            )?;
        }
        Command::Restore {
            collection,
            revision,
        } => {
            let result = database.collection(collection)?.restore(revision)?;
            print_value(&result, output_format)?;
        }
        Command::Push { collection, remote } => {
            sync_push(&database, &collection, &remote)?;
            print_value(
                &json!({"collection": collection, "remote": remote, "pushed": true}),
                output_format,
            )?;
        }
        Command::Pull { collection, remote } => {
            let root = sync_pull(&database, &collection, &remote)?;
            print_value(
                &json!({"collection": collection, "remote": remote, "root": root, "pulled": true}),
                output_format,
            )?;
        }
        Command::Diff {
            collection,
            left_root,
            right_root,
        } => print_value(
            &database
                .collection(collection)?
                .diff(left_root, right_root)?,
            output_format,
        )?,
        Command::Validate {
            collection,
            at,
            full,
        } => {
            let collection = at_collection(database.collection(collection)?, at.as_deref())?;
            print_value(&collection.validate(full)?, output_format)?;
        }
        Command::Doctor => {
            let names = database.list_collections()?;
            let mut reports = Vec::with_capacity(names.len());
            let mut valid = true;
            for name in names {
                match database.collection(&name)?.validate(false) {
                    Ok(report) => reports.push(json!({
                        "name": name,
                        "root": report.root,
                        "points": report.point_count,
                        "valid": report.valid,
                    })),
                    Err(error) => {
                        valid = false;
                        reports.push(
                            json!({"name": name, "valid": false, "error": error.to_string()}),
                        );
                    }
                }
            }
            print_value(
                &json!({
                    "database": repo_path,
                    "collections": reports.len(),
                    "valid": valid,
                    "reports": reports,
                }),
                output_format,
            )?;
        }
        Command::Completions { .. } => unreachable!(),
    }
    Ok(())
}

fn at_collection(collection: Collection, at: Option<&str>) -> Result<Collection> {
    match at {
        Some(revision) => collection.at(revision),
        None => Ok(collection),
    }
}

fn sync_push(database: &Database, collection: &str, remote_name: &str) -> Result<()> {
    Database::validate_collection_name(collection)?;
    database.collection(collection)?;
    let repository = git2::Repository::open(database.path())?;
    let mut remote = repository.find_remote(remote_name).map_err(|error| {
        Error::Invalid(format!(
            "Git remote {remote_name:?} is not configured: {error}"
        ))
    })?;
    let reference = format!("refs/git-vdb/collections/{collection}");
    remote
        .push(&[format!("{reference}:{reference}")], None)
        .map_err(Error::Git)
}

fn sync_pull(database: &Database, collection: &str, remote_name: &str) -> Result<ObjectId> {
    Database::validate_collection_name(collection)?;
    let repository = git2::Repository::open(database.path())?;
    let mut remote = repository.find_remote(remote_name).map_err(|error| {
        Error::Invalid(format!(
            "Git remote {remote_name:?} is not configured: {error}"
        ))
    })?;
    let source = format!("refs/git-vdb/collections/{collection}");
    let tracking = format!("refs/git-vdb/remotes/{remote_name}/{collection}");
    if !git2::Reference::is_valid_name(&tracking) {
        return Err(Error::Invalid(format!(
            "Git remote name {remote_name:?} cannot be used in a tracking ref"
        )));
    }
    remote.fetch(&[format!("{source}:{tracking}")], None, None)?;
    let fetched = repository
        .find_reference(&tracking)?
        .target()
        .ok_or_else(|| Error::Invalid(format!("fetched ref {tracking} has no direct target")))?;
    match repository.find_reference(&source) {
        Ok(mut local) => {
            let current = local.target().ok_or_else(|| {
                Error::Invalid(format!("local ref {source} has no direct target"))
            })?;
            if current != fetched && !repository.graph_descendant_of(fetched, current)? {
                return Err(Error::Invalid(format!(
                    "pull would not fast-forward collection {collection:?}; inspect history and restore explicitly"
                )));
            }
            local.set_target(fetched, "git-vdb pull fast-forward")?;
        }
        Err(error) if error.code() == git2::ErrorCode::NotFound => {
            repository.reference(&source, fetched, false, "git-vdb pull create collection")?;
        }
        Err(error) => return Err(error.into()),
    }
    let commit = repository.find_commit(fetched)?;
    Ok(commit.tree_id().into())
}

#[cfg(feature = "fastembed")]
fn search_local_text(
    path: &Path,
    collection: String,
    query: TextQuery,
) -> Result<Vec<DocumentHit>> {
    Store::open(path)?
        .text_collection(collection, FastEmbedder::try_new()?)?
        .query(query)
}

#[cfg(not(feature = "fastembed"))]
fn search_local_text(_: &Path, _: String, _: TextQuery) -> Result<Vec<DocumentHit>> {
    Err(Error::Invalid(
        "text search requires a binary built with --features fastembed".into(),
    ))
}

#[cfg(feature = "fastembed")]
fn index_local_text(
    path: &Path,
    collection: String,
    paths: Vec<PathBuf>,
    chunk_chars: usize,
) -> Result<Value> {
    if chunk_chars == 0 {
        return Err(Error::Invalid(
            "--chunk-chars must be greater than zero".into(),
        ));
    }
    let mut files = Vec::new();
    for path in paths {
        collect_text_files(&path, &mut files)?;
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err(Error::Invalid("index input contains no files".into()));
    }

    let documents = Store::open(path)?.text_collection(collection, FastEmbedder::try_new()?)?;
    let mut indexed_chunks = 0;
    for file in &files {
        let source = file.to_string_lossy().into_owned();
        let text = fs::read_to_string(file).map_err(|error| {
            Error::Invalid(format!(
                "cannot read UTF-8 file {}: {error}",
                file.display()
            ))
        })?;
        let chunks = chunk_text(&text, chunk_chars);
        let batch = chunks
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                Document::new(format!("{source}#{index:06}"), text).with_metadata(json!({
                    "source": source,
                    "chunk": index,
                }))
            })
            .collect::<Result<Vec<_>>>()?;
        let filter = Filter::must([Condition::matches("source", source.clone())]);
        if batch.is_empty() {
            if documents.vectors().advanced().is_ok() {
                documents.delete(DeleteSelector {
                    filter: Some(filter),
                    ..DeleteSelector::default()
                })?;
            }
        } else if documents.vectors().advanced().is_ok() {
            indexed_chunks += batch.len();
            documents.replace_documents(filter, batch)?;
        } else {
            indexed_chunks += batch.len();
            documents.upsert_documents(batch)?;
        }
    }
    Ok(json!({
        "files": files.len(),
        "chunks": indexed_chunks,
        "root": documents.vectors().root()?,
    }))
}

#[cfg(not(feature = "fastembed"))]
fn index_local_text(_: &Path, _: String, _: Vec<PathBuf>, _: usize) -> Result<Value> {
    Err(Error::Invalid(
        "text indexing requires a binary built with --features fastembed".into(),
    ))
}

#[cfg(feature = "fastembed")]
fn collect_text_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    if path.is_file() {
        files.push(path.to_path_buf());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(Error::Invalid(format!(
            "index path does not exist: {}",
            path.display()
        )));
    }
    let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let entry_path = entry.path();
        if entry_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == ".git" || name == "target")
        {
            continue;
        }
        collect_text_files(&entry_path, files)?;
    }
    Ok(())
}

#[cfg(feature = "fastembed")]
fn chunk_text(text: &str, maximum_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let separator = usize::from(!current.is_empty());
        if !current.is_empty()
            && current.chars().count() + separator + word.chars().count() > maximum_chars
        {
            chunks.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn parse_object_id(value: Option<String>) -> Result<Option<ObjectId>> {
    value
        .map(|value| ObjectId::from_str(&value).map_err(Error::Git))
        .transpose()
}

fn parse_ids(ids: Vec<String>) -> Result<Vec<PointId>> {
    Ok(ids
        .into_iter()
        .map(|id| serde_json::from_str::<PointId>(&id).unwrap_or(PointId::String(id)))
        .collect())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_reader(File::open(path).map_err(
        |error| Error::Invalid(format!("cannot open {}: {error}", path.display())),
    )?)?)
}

fn read_optional_json_arg<T: serde::de::DeserializeOwned>(
    value: Option<&str>,
) -> Result<Option<T>> {
    value.map(read_json_arg).transpose()
}

fn read_json_arg<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    if value == "-" {
        return Ok(serde_json::from_reader(io::stdin().lock())?);
    }
    let path = Path::new(value);
    if path.exists() {
        return read_json(path);
    }
    Ok(serde_json::from_str(value)?)
}

fn read_vector(path: &Path) -> Result<Vec<f32>> {
    let value: Value = read_json(path)?;
    if let Some(vector) = value.get("vector") {
        Ok(serde_json::from_value(vector.clone())?)
    } else {
        Ok(serde_json::from_value(value)?)
    }
}

fn parse_vector_arg(value: &str) -> Result<Vec<f32>> {
    let path = Path::new(value);
    if path.exists() {
        return read_vector(path);
    }
    if value.trim_start().starts_with('[') {
        return Ok(serde_json::from_str(value)?);
    }
    value
        .split(',')
        .map(|component| {
            component.trim().parse::<f32>().map_err(|error| {
                Error::Invalid(format!("invalid vector component {component:?}: {error}"))
            })
        })
        .collect()
}

fn inline_point(id: String, vector: String, payload: Option<String>) -> Result<Vec<Point>> {
    let payload = match payload {
        Some(payload) => match serde_json::from_str::<Value>(&payload)? {
            Value::Object(payload) => payload,
            _ => {
                return Err(Error::Invalid(
                    "inline payload must be a JSON object".into(),
                ))
            }
        },
        None => JsonObject::new(),
    };
    Ok(vec![Point {
        id: parse_ids(vec![id])?.remove(0),
        vector: parse_vector_arg(&vector)?,
        payload,
    }])
}

fn stream_upsert(
    store: &Store,
    database: &Database,
    collection: &str,
    path: &Path,
    mut expected: Option<ObjectId>,
    batch_size: usize,
    progress: bool,
) -> Result<WriteResult> {
    if batch_size == 0 {
        return Err(Error::Invalid(
            "--batch-size must be greater than zero".into(),
        ));
    }
    let mut staged = if path == Path::new("-") {
        let mut file = tempfile::NamedTempFile::new()?;
        io::copy(&mut io::stdin().lock(), file.as_file_mut())?;
        file.as_file_mut().flush()?;
        Some(file)
    } else {
        None
    };
    let input_path = staged.as_mut().map(|file| file.path()).unwrap_or(path);
    let source = if path == Path::new("-") {
        "stdin".to_owned()
    } else {
        path.display().to_string()
    };
    let preflight = File::open(input_path)?;
    preflight_json_lines(BufReader::new(preflight), &source)?;

    let mut affected_points = 0;
    let mut batches = 0;
    let mut final_root = None;
    let mut write = |points: Vec<Point>| -> Result<()> {
        let count = points.len();
        let result = match expected.take() {
            Some(root) => database
                .collection(collection)?
                .upsert_expect(points, Some(root))?,
            None => store.collection(collection).upsert(points)?,
        };
        expected = Some(result.root.clone());
        final_root = Some(result.root);
        affected_points += count;
        batches += 1;
        if progress {
            eprintln!("git-vdb: imported {affected_points} points in {batches} batch(es)");
        }
        Ok(())
    };
    let file = File::open(input_path)
        .map_err(|error| Error::Invalid(format!("cannot open {source}: {error}")))?;
    stream_json_lines(BufReader::new(file), &source, batch_size, &mut write)?;
    let root =
        final_root.ok_or_else(|| Error::Invalid("upsert input contains no points".into()))?;
    Ok(WriteResult {
        root,
        affected_points,
    })
}

fn stream_json_lines(
    reader: impl BufRead,
    source: &str,
    batch_size: usize,
    mut write: impl FnMut(Vec<Point>) -> Result<()>,
) -> Result<()> {
    let mut batch = Vec::with_capacity(batch_size);
    for (line_number, line) in reader.lines().enumerate() {
        let line =
            line.map_err(|error| Error::Invalid(format!("cannot read {source}: {error}")))?;
        if line.trim().is_empty() {
            continue;
        }
        batch.push(parse_point_line(&line, source, line_number + 1)?);
        if batch.len() == batch_size {
            write(std::mem::take(&mut batch))?;
            batch = Vec::with_capacity(batch_size);
        }
    }
    if !batch.is_empty() {
        write(batch)?;
    }
    Ok(())
}

fn preflight_json_lines(reader: impl BufRead, source: &str) -> Result<()> {
    let mut dimension = None;
    let mut points = 0;
    for (line_number, line) in reader.lines().enumerate() {
        let line =
            line.map_err(|error| Error::Invalid(format!("cannot read {source}: {error}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let point = parse_point_line(&line, source, line_number + 1)?;
        if point.vector.is_empty() {
            return Err(Error::Invalid(format!(
                "point {} on {source} line {} has an empty vector",
                point.id,
                line_number + 1
            )));
        }
        match dimension {
            Some(expected) if point.vector.len() != expected => {
                return Err(Error::Invalid(format!(
                    "point {} on {source} line {} has dimension {}, expected {expected}",
                    point.id,
                    line_number + 1,
                    point.vector.len()
                )))
            }
            None => dimension = Some(point.vector.len()),
            _ => {}
        }
        points += 1;
    }
    if points == 0 {
        return Err(Error::Invalid("upsert input contains no points".into()));
    }
    Ok(())
}

fn parse_point_line(line: &str, source: &str, line_number: usize) -> Result<Point> {
    serde_json::from_str(line).map_err(|error| {
        Error::Invalid(format!(
            "invalid JSON on {source} line {line_number}: {error}"
        ))
    })
}

fn print_value(value: &impl Serialize, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string(value)?),
        OutputFormat::Pretty => println!("{}", serde_json::to_string_pretty(value)?),
        OutputFormat::Jsonl => {
            let value = serde_json::to_value(value)?;
            let rows = value.as_object().and_then(|object| {
                ["points", "collections", "history", "reports"]
                    .into_iter()
                    .find_map(|key| object.get(key).and_then(Value::as_array))
            });
            if let Some(rows) = rows {
                for row in rows {
                    println!("{}", serde_json::to_string(row)?);
                }
            } else {
                println!("{}", serde_json::to_string(&value)?);
            }
        }
    }
    Ok(())
}
