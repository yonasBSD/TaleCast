use crate::config::GlobalConfig;
use crate::config::PodcastConfigs;
use clap::Parser;
use regex::Regex;
use std::path::PathBuf;

mod cache;
mod config;
mod display;
mod download_tracker;
mod episode;
mod opml;
mod patterns;
mod podcast;
mod tags;
mod utils;

pub const APPNAME: &'static str = "talecast";

#[derive(Parser)]
#[command(
    name = "TaleCast",
    version,
    about = "A simple CLI podcast manager.",
    long_about = None
)]
struct Args {
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Import podcasts from an OPML file"
    )]
    import: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Export your podcasts to an OPML file"
    )]
    export: Option<PathBuf>,
    #[arg(short, long, help = "Print the downloaded paths to stdout")]
    print: bool,
    #[arg(
        short,
        long,
        help = "Configure to skip episodes published prior to current time. Can be combined with filter, add, and import"
    )]
    catch_up: bool,
    #[arg(short, long, num_args = 1..=2, value_names = &["URL", "NAME"], help = "Add new podcast")]
    add: Vec<String>,
    #[arg(
        short,
        long,
        help = "Filter which podcasts to sync or export with a regex pattern"
    )]
    filter: Option<String>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Override the path to the config file"
    )]
    config: Option<PathBuf>,
    #[arg(long, help = "Edit the config.toml file")]
    edit_config: bool,
    #[arg(long, help = "Edit the podcasts.toml file")]
    edit_podcasts: bool,
    #[arg(short, long, value_name = "QUERY",  num_args = 1.., help = "Search for podcasts to add")]
    search: Option<Vec<String>>,
    #[arg(long, help = "Print your podcasts to stdout")]
    list: bool,
}

impl From<Args> for Action {
    fn from(args: Args) -> Self {
        let filter = args.filter.map(|filter| {
            let filter = format!("(?i){}", filter); // Case insensitive
            Regex::new(&filter).unwrap()
        });

        let print = args.print;
        let catch_up = args.catch_up;

        if args.list {
            return Self::List { filter };
        }

        if args.edit_config {
            let path = GlobalConfig::default_path();
            return Self::Edit { path };
        }

        if args.edit_podcasts {
            let path = config::PodcastConfigs::path();
            return Self::Edit { path };
        }

        if let Some(query) = args.search {
            let query = query.join(" ");
            return Self::Search { query, catch_up };
        }

        if let Some(path) = args.import {
            return Self::Import { path, catch_up };
        }

        if let Some(path) = args.export {
            return Self::Export { path, filter };
        }

        if !args.add.is_empty() {
            let url = args.add[0].to_string();
            let name = args.add.get(1).cloned();

            return Self::Add {
                url,
                name,
                catch_up,
            };
        }

        if catch_up {
            return Self::CatchUp { filter };
        }

        Self::Sync { filter, print }
    }
}

enum Action {
    List {
        filter: Option<Regex>,
    },
    CatchUp {
        filter: Option<Regex>,
    },
    Edit {
        path: PathBuf,
    },
    Import {
        path: PathBuf,
        catch_up: bool,
    },
    Export {
        path: PathBuf,
        filter: Option<Regex>,
    },
    Add {
        url: String,
        name: Option<String>,
        catch_up: bool,
    },
    Search {
        query: String,
        catch_up: bool,
    },
    Sync {
        filter: Option<Regex>,
        print: bool,
    },
}

use chrono::Local;
use fern::Dispatch;

fn setup_logging(config: &config::LogConfig) -> Result<PathBuf, fern::InitError> {
    let base_config = Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{} [{}] {}",
                Local::now().format("%Y-%m-%d %H:%M:%S"),
                record.level(),
                message
            ))
        })
        .level(config.level());

    let log_dir = PathBuf::from("/tmp/talecast");
    utils::create_dir(&log_dir);
    let log_path = log_dir.join(chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string());

    let file_config = base_config.chain(fern::log_file(&log_path)?);

    file_config.apply()?;
    Ok(log_path)
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let global_config = match args.config.as_ref() {
        Some(path) => GlobalConfig::load_from_path(path),
        None => GlobalConfig::load(),
    };

    let log_path = setup_logging(&global_config.log()).unwrap();

    match Action::from(args) {
        Action::Import { path, catch_up } => opml::import(&path, catch_up),

        Action::Edit { path } => utils::edit_file(&path),

        Action::CatchUp { filter } => config::PodcastConfigs::catch_up(filter),

        Action::List { filter } => {
            for (name, _) in config::PodcastConfigs::load().filter(filter) {
                println!("{}", name);
            }
        }

        Action::Search { query, catch_up } => {
            utils::search_podcasts(&global_config, query, catch_up).await
        }

        Action::Export { path, filter } => opml::export(&path, filter).await,

        Action::Add {
            name,
            url,
            catch_up,
        } => {
            let name = match name {
                Some(name) => name,
                None => match utils::get_input(Some("enter name of podcast: ")) {
                    Some(name) => name,
                    None => return,
                },
            };

            let podcast = config::PodcastConfig::new(url);

            if config::PodcastConfigs::push(name.clone(), podcast) {
                eprintln!("'{}' added!", name);
                if catch_up {
                    // Matches only the added podcast.
                    let filter = Regex::new(&format!("^{}$", &name)).unwrap();
                    config::PodcastConfigs::catch_up(Some(filter));
                }
            } else {
                eprintln!("'{}' already exists!", name);
            }
        }

        Action::Sync { filter, print } => {
            let paths = PodcastConfigs::load()
                .assert_not_empty()
                .filter(filter)
                .sync(global_config, &log_path)
                .await;

            eprintln!("Syncing complete!");
            eprintln!("{} episodes downloaded.", paths.len());

            if print {
                for path in paths {
                    println!("{}", path.to_str().unwrap());
                }
            }
        }
    }
}
