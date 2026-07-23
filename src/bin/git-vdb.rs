use clap::{Args, Parser, Subcommand, ValueEnum};
use git_vdb::*;
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;

#[derive(Parser)]
#[command(name = "git-vdb", version, about)]
struct Cli {
    /// Git repository. Overrides GIT_VDB_REPO.
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
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
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        expect_root: Option<String>,
    },
    /// Retrieve points without similarity ranking.
    Get {
        collection: String,
        #[arg(long, num_args = 1..)]
        ids: Vec<String>,
        #[arg(long)]
        filter: Option<PathBuf>,
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
        filter: Option<PathBuf>,
        #[arg(long)]
        expect_root: Option<String>,
    },
    /// Count points, optionally under a filter.
    Count {
        collection: String,
        #[arg(long)]
        filter: Option<PathBuf>,
        #[arg(long)]
        at: Option<String>,
    },
    /// Run a cosine nearest-neighbor query.
    Query {
        collection: String,
        #[arg(long)]
        vector: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long)]
        filter: Option<PathBuf>,
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
    /// Show collection commit history.
    History {
        collection: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
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
    if let Command::Init { repo, bare } = cli.command {
        let database = Database::init_with_options(&repo, bare)?;
        print_json(&json!({
            "repository": repo,
            "bare": bare,
            "collections": database.list_collections()?,
        }))?;
        return Ok(());
    }

    let repo_path = cli
        .repo
        .or_else(|| std::env::var_os("GIT_VDB_REPO").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let database = Database::open(&repo_path)?;
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
                print_json(&collection.info()?)?;
            }
            CollectionCommand::List => {
                let collections = database
                    .list_collections()?
                    .into_iter()
                    .map(|name| database.collection(&name)?.info())
                    .collect::<Result<Vec<_>>>()?;
                print_json(&json!({"collections": collections}))?;
            }
            CollectionCommand::Info { name, at } => {
                let collection = at_collection(database.collection(name)?, at.as_deref())?;
                print_json(&collection.info()?)?;
            }
            CollectionCommand::Delete { name } => {
                let root = database.delete_collection(&name)?;
                print_json(&json!({"name": name, "root": root, "deleted": true}))?;
            }
        },
        Command::Upsert {
            collection,
            input,
            expect_root,
        } => {
            let points = read_json_lines(&input)?;
            let expected = parse_object_id(expect_root)?;
            print_json(
                &database
                    .collection(collection)?
                    .upsert_expect(points, expected)?,
            )?;
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
            print_json(&collection.get(GetRequest {
                ids: parse_ids(ids)?,
                filter: read_optional_json(filter.as_deref())?,
                offset,
                limit,
                with_payload,
                with_vector,
            })?)?;
        }
        Command::Delete {
            collection,
            ids,
            filter,
            expect_root,
        } => {
            let selector = DeleteSelector {
                ids: parse_ids(ids)?,
                filter: read_optional_json(filter.as_deref())?,
            };
            let expected = parse_object_id(expect_root)?;
            print_json(
                &database
                    .collection(collection)?
                    .delete_expect(selector, expected)?,
            )?;
        }
        Command::Count {
            collection,
            filter,
            at,
        } => {
            let collection = at_collection(database.collection(collection)?, at.as_deref())?;
            print_json(&collection.count(read_optional_json(filter.as_deref())?)?)?;
        }
        Command::Query {
            collection,
            vector,
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
            let collection = at_collection(database.collection(collection)?, at.as_deref())?;
            print_json(
                &collection.query(Query {
                    vector: read_vector(&vector)?,
                    limit,
                    filter: read_optional_json(filter.as_deref())?,
                    with_payload,
                    with_vector,
                    expected_vector_space,
                    params: QueryParams {
                        exact: exact
                            .then_some(true)
                            .or_else(|| approximate.then_some(false)),
                        probes,
                        candidate_limit,
                    },
                })?,
            )?;
        }
        Command::History { collection, limit } => {
            let collection = database.collection(collection)?;
            let root = collection.root()?;
            print_json(&json!({"root": root, "history": collection.history(limit)?}))?;
        }
        Command::Diff {
            collection,
            left_root,
            right_root,
        } => print_json(
            &database
                .collection(collection)?
                .diff(left_root, right_root)?,
        )?,
        Command::Validate {
            collection,
            at,
            full,
        } => {
            let collection = at_collection(database.collection(collection)?, at.as_deref())?;
            print_json(&collection.validate(full)?)?;
        }
    }
    Ok(())
}

fn at_collection(collection: Collection, at: Option<&str>) -> Result<Collection> {
    match at {
        Some(revision) => collection.at(revision),
        None => Ok(collection),
    }
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

fn read_optional_json<T: serde::de::DeserializeOwned>(path: Option<&Path>) -> Result<Option<T>> {
    path.map(read_json).transpose()
}

fn read_vector(path: &Path) -> Result<Vec<f32>> {
    let value: Value = read_json(path)?;
    if let Some(vector) = value.get("vector") {
        Ok(serde_json::from_value(vector.clone())?)
    } else {
        Ok(serde_json::from_value(value)?)
    }
}

fn read_json_lines(path: &Path) -> Result<Vec<Point>> {
    let file = File::open(path)
        .map_err(|error| Error::Invalid(format!("cannot open {}: {error}", path.display())))?;
    BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(line_number, line)| match line {
            Ok(line) if line.trim().is_empty() => None,
            Ok(line) => Some(serde_json::from_str(&line).map_err(|error| {
                Error::Invalid(format!(
                    "invalid JSON on {} line {}: {error}",
                    path.display(),
                    line_number + 1
                ))
            })),
            Err(error) => Some(Err(Error::Invalid(format!(
                "cannot read {}: {error}",
                path.display()
            )))),
        })
        .collect()
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}
