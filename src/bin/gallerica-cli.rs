use std::os::unix::net::UnixStream;

use clap::Parser;

use gallerica::Message;

#[derive(Parser)]
struct Cli {
    #[clap(subcommand)]
    command: Option<Message>
}


fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        let stream = UnixStream::connect("pipe")?;
        serde_json::to_writer(&stream, &cmd)?;
    }

    Ok(())
}
