//! Tool to randomly select images from a set of folders.
//! And supporting an MQTT API to be configured on the fly.

use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::read_dir,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    process::ExitStatus,
};

use anyhow::{anyhow, bail, Context, Result};

use clap::Parser;
use directories::UserDirs;
use rand::prelude::*;
use serde::Deserialize;

use tokio::{
    pin,
    process::Command,
    select, signal,
    task::JoinHandle,
    time::{self, Duration, Interval},
};

pub mod message_api;
pub use gallerica::project_dirs;
pub use message_api::Message;

#[derive(Parser)]
struct Cli {
    /// Config file to use. If this argument is not given, then it will read
    /// $XDG_DATA_HOME/gallerica/config.toml (or equivalent) by default
    #[clap(short)]
    config_file: Option<PathBuf>,
}

enum CmdLinePart {
    Literal(OsString),
    Placeholder,
}

#[derive(Deserialize, Debug, Clone)]
struct Gallery {
    name: String,
    #[serde(rename = "folders")]
    sources: Vec<PathBuf>,
}

struct ApplicationState {
    galleries: HashMap<String, Gallery>,
    current_gallery: Option<String>,
    update_interval: Interval,
    display_command: OsString,
    display_args: Vec<CmdLinePart>,

    message_interface: Option<Box<dyn message_api::MessageReceiver>>,

    /// Task which runs the update subprocess
    update_task: Option<JoinHandle<io::Result<ExitStatus>>>,

    /// In case a new update is requested, while an existing one is still running, this will buffer
    /// the next update, in order to execute it once the first one finishes.
    /// Only one update is buffered, if a third update arrives, while the first is still running,
    /// the second one is discarded in favor for the third.
    pending_update: Option<Command>,
}

fn parse_args<T, S>(args: T) -> impl Iterator<Item = CmdLinePart>
where
    T: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    use CmdLinePart::*;
    args.into_iter().map(|e| match e {
        text if text.as_ref().to_str() == Some("{image}") => Placeholder,
        text => Literal(text.as_ref().to_os_string()),
    })
}

impl ApplicationState {
    pub fn new<T, S>(update_command: T, update_interval: Interval) -> Result<Self>
    where
        T: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut cmdline = update_command.into_iter();

        let cmd = cmdline
            .next()
            .ok_or_else(|| anyhow!("Need a command"))?
            .as_ref()
            .to_os_string();

        Ok(ApplicationState {
            galleries: HashMap::new(),
            current_gallery: None,
            update_interval,
            display_command: cmd,
            display_args: parse_args(cmdline).collect(),
            message_interface: None,
            update_task: None,
            pending_update: None,
        })
    }

    pub fn add_gallery(&mut self, gallery: Gallery) {
        self.galleries.insert(gallery.name.clone(), gallery);
    }

    pub fn change_gallery(&mut self, name: &str) -> Result<()> {
        if !self.galleries.contains_key(name) {
            bail!("Invalid gallery '{}'", name);
        }
        self.current_gallery = Some(name.to_owned());
        Ok(())
    }

    pub async fn connect_listener(&mut self) -> anyhow::Result<()> {
        //self.message_interface = Some(Box::new(message_api::MQTTReceiver::new().await?));
        self.message_interface = Some(Box::new(message_api::UnixSocketReceiver::new().await?));
        Ok(())
    }

    pub async fn update(&mut self) {
        let mut cmd = Command::new(&self.display_command);

        let replacement = match self.select_random_image() {
            Some(path) => path,
            None => return,
        };

        use CmdLinePart::*;
        cmd.args(self.display_args.iter().map(|ref a| match a {
            Literal(t) => t.as_ref(),
            Placeholder => {
                let repl: &OsStr = replacement.as_ref();
                repl
            }
        }));

        match self.update_task {
            Some(_) => {
                if self.pending_update.is_some() {
                    eprintln!("Discarding pending update");
                }
                self.pending_update = Some(cmd);
            }
            None => {
                self.update_task = Some(tokio::spawn(
                    async move { cmd.spawn().unwrap().wait().await },
                ));
            }
        }
    }

    fn select_random_image(&self) -> Option<PathBuf> {
        let source_folders = &self.galleries.get(self.current_gallery.as_ref()?)?.sources;

        let mut rng = rand::thread_rng();

        let all_files = source_folders
            .iter()
            .filter_map(|dir| read_dir(dir).ok())
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path());

        all_files.choose(&mut rng)
    }

    async fn handle_message(&mut self, msg: Message) {
        match msg {
            Message::NextImage => {
                self.update().await;
                self.update_interval.reset();
            }
            Message::UpdateInterval { millis } => {
                self.update_interval = time::interval(Duration::from_millis(millis));
            }
            Message::SelectGallery { name, refresh } => {
                if let Err(err) = self.change_gallery(&name) {
                    eprintln!("Failed to change gallery to '{name}': {err}");
                    return;
                }

                if refresh {
                    self.update().await;
                }
            }
        }
    }

    pub async fn run(&mut self) {
        pin! {
            let shutdown_task = tokio::spawn(shutdown_signal_received());
        }
        loop {
            select! {
                _ = self.update_interval.tick() => {
                    self.update().await;
                },

                // If an update finished, then reset the update task back to none
                _ = async {self.update_task.as_mut().unwrap().await}, if self.update_task.is_some() => {
                    self.update_task = self.pending_update.take().map(|mut cmd| {
                        tokio::spawn(
                            async move { cmd.spawn().unwrap().wait().await },
                        )
                    });
                },

                message = self.message_interface.as_mut().unwrap().receive_message() => {
                    match message {
                        Ok(message) => self.handle_message(message).await,
                        Err(err) => { eprintln!("Error while receiving messages!: {err}"); return; },
                    }
                },

                _ = &mut shutdown_task => break,
            }
        }
    }

    pub fn update_configuration(&mut self, config: &Configuration) -> Result<()> {
        for mut gallery in config.galleries.iter().cloned() {
            for folder in gallery.sources.iter_mut() {
                if let Cow::Owned(path) = expand_tilde(&folder)? {
                    *folder = path;
                }
            }
            self.add_gallery(gallery);
        }

        self.change_gallery(&config.default_gallery)?;

        let mut cmdline = config.command_line.split(' ');

        let cmd = AsRef::<OsStr>::as_ref(&cmdline.next().ok_or_else(|| anyhow!("Need a command"))?)
            .to_os_string();

        self.display_command = cmd;
        self.display_args = parse_args(cmdline).collect();

        self.update_interval = time::interval(Duration::from_millis(config.update_interval_ms));

        Ok(())
    }
}

#[derive(Deserialize, Debug)]
struct Configuration {
    pub command_line: String,
    pub update_interval_ms: u64,
    pub default_gallery: String,
    pub galleries: Vec<Gallery>,
}

fn read_configuration(app: &mut ApplicationState, config_file: &Path) -> Result<()> {
    let make_ctx = || anyhow!("Failed to open config file '{}'", config_file.display());

    let mut cfg = std::fs::File::open(config_file).context(make_ctx())?;

    let mut text = String::new();
    cfg.read_to_string(&mut text).context(make_ctx())?;

    let cfg: Configuration = toml::from_str(&text).context("Failed to parse configuration")?;

    app.update_configuration(&cfg)
        .context("Failed to apply configuration")?;

    Ok(())
}

/// Block until the process received a shutdown signal, e.g. CTRL-C.
async fn shutdown_signal_received() {
    use signal::unix::{self, SignalKind};
    let mut sigterm =
        unix::signal(SignalKind::terminate()).expect("Failed to install sigterm handler");
    select! {
       _ = signal::ctrl_c() => {}
       _ = sigterm.recv() => {}
    }
}

/// Given a path, expand a leading `~` to the user home directory, if any.
/// Returns Err if the user does not have a home.
fn expand_tilde(path: &Path) -> Result<Cow<Path>> {
    let mut components = path.components();

    if let Some(Component::Normal(start)) = components.next() {
        if let Some("~") = start.to_str() {
            return Ok(Cow::Owned(
                UserDirs::new()
                    .ok_or_else(|| anyhow!("User has no home directory!"))?
                    .home_dir()
                    .join(components),
            ));
        }
    }

    Ok(Cow::Borrowed(path))
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut state = ApplicationState::new(
        "echo {image}".split(' '),
        time::interval(Duration::from_millis(10000)),
    )?;

    let cli = Cli::parse();

    let config_path = if let Some(ref path) = cli.config_file {
        path.clone()
    } else {
        gallerica::project_dirs().config_dir().join("config.toml")
    };

    read_configuration(&mut state, &config_path)?;

    state.connect_listener().await?;

    state.run().await;
    Ok(())
}
