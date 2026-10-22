//! `syncsound`: listen to the same music, at the same time, in genre rooms.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use syncsound::{client, server};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the server and one room per genre in the catalog
    Server {
        /// Address to listen on (use 0.0.0.0:7878 to accept LAN clients)
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
        /// JSON file describing the songs
        #[arg(long, default_value = "catalog.json")]
        catalog: PathBuf,
        /// Where songs given by URL are downloaded to
        #[arg(long, default_value = "cache")]
        cache_dir: PathBuf,
        /// Number of worker threads handling requests
        #[arg(long, default_value_t = 4)]
        threads: usize,
    },
    /// List the rooms available on a server
    Rooms {
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
    },
    /// Join a room and listen in sync with everyone else in it
    Join {
        /// Room name (a genre, see `syncsound rooms`)
        room: String,
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: String,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Server { addr, catalog, cache_dir, threads } => {
            server::run(&addr, &catalog, &cache_dir, threads)
        }
        Command::Rooms { addr } => client::list_rooms(&addr),
        Command::Join { room, addr } => client::join(&addr, &room),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nerror: {e}");
            ExitCode::FAILURE
        }
    }
}
