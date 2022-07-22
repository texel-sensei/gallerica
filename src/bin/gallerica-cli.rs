use std::{fs::create_dir_all, os::unix::net::UnixStream, path::Path};

use anyhow::Context;
use clap::Parser;

use gallerica::{project_dirs, Request, Response};

#[derive(Parser)]
#[clap(author, version)]
#[clap(about = "Control a running gallerica daemon")]
struct Cli {
    #[clap(subcommand)]
    command: Request,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let dirs = project_dirs();
    let path = dirs.runtime_dir().unwrap_or_else(|| Path::new("/tmp"));
    let file = path.join("gallerica.sock");

    let stream = (|| {
        create_dir_all(path)?;

        UnixStream::connect(file.clone())
    })()
    .with_context(|| format!("Failed to connect to Unix socket at '{}'", file.display()))?;

    serde_json::to_writer(&stream, &cli.command)?;
    stream.shutdown(std::net::Shutdown::Write)?;

    let response: Response = serde_json::from_reader(&stream)?;

    println!("{response:?}");

    Ok(())
}
