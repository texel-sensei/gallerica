use std::{
    fs::create_dir_all,
    os::unix::{net::UnixStream, prelude::FileTypeExt},
    path::{Path, PathBuf},
};

use anyhow::Context;
use clap::Parser;

use gallerica::{project_dirs, Request, Response};

#[derive(Parser)]
#[clap(author, version)]
#[clap(about = "Control a running gallerica daemon")]
struct Cli {
    #[clap(subcommand)]
    command: Request,

    /// Path to the unix socket file on which a gallerica daemon is listening.
    /// May be an absolute or relative path.
    /// Relative paths are relative to the system runtime directory (XDG_RUNTIME_DIR).
    #[clap(short, long, default_value = "gallerica.sock")]
    socket: PathBuf,

    /// Send command to all sockets in the runtime directory instead of the default one.
    /// If this option is set, then the value of <socket> is ignored.
    #[clap(short, long)]
    all: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let dirs = project_dirs();
    let path = dirs.runtime_dir().unwrap_or_else(|| Path::new("/tmp"));
    create_dir_all(path)?;

    if cli.all {
        for file in path.read_dir()?.filter_map(|e| e.ok()) {
            if file.file_type()?.is_socket() {
                match send_via_file(&file.path(), &cli.command) {
                    Ok(response) => println!("{}: {:?}", file.file_name().to_string_lossy(), response),
                    Err(e) => eprintln!("Failed sending to {}: {e}", file.path().display()),
                }
            }
        }
    } else {
        let file = path.join(cli.socket);
        let response = send_via_file(&file, &cli.command)?;
        println!("{response:?}");
    }

    Ok(())
}

fn send_via_file(file: &Path, command: &Request) -> anyhow::Result<Response> {
    let stream = UnixStream::connect(file)
        .with_context(|| format!("Failed to connect to Unix socket at '{}'", file.display()))?;

    serde_json::to_writer(&stream, &command)?;
    stream.shutdown(std::net::Shutdown::Write)?;

    Ok(serde_json::from_reader(&stream)?)
}
