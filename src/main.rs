mod cli;
mod model;
mod normalize;
mod pipeline;

use std::env;
use std::process::ExitCode;

use cli::Config;

fn main() -> ExitCode {
    let config = match Config::parse(env::args_os().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}\n\n{}", Config::usage());
            return ExitCode::FAILURE;
        }
    };

    match pipeline::build(&config) {
        Ok(stats) => {
            println!(
                "Built {} entries in {} shards from {} accepted input records ({} skipped)",
                stats.entries, stats.shards, stats.accepted_records, stats.skipped_records
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Build failed: {error}");
            ExitCode::FAILURE
        }
    }
}

