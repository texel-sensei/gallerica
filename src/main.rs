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
    sync::Mutex,
};

use anyhow::{anyhow, bail, Context, Result};

use circular_queue::CircularQueue;
use clap::Parser;
use directories::UserDirs;
use rand::prelude::*;
use serde::{Deserialize, Serialize};

use tokio::{
    pin,
    process::Command,
    select, signal,
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
    time::Duration,
};

mod message_api;
pub use gallerica::project_dirs;
use message_api::{InflightRequest, MessageReceiver, MessageSource};
pub use message_api::{Request, Response};

mod unix_socket_listener;
use unix_socket_listener::{UnixListenerConfig, UnixSocketReceiver};

mod mqtt_listener;
use mqtt_listener::{MqttListenerConfig, MqttReceiver};

mod timer;
use timer::{PausableInterval, TickResult};

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
    update_interval: PausableInterval,
    display_command: OsString,
    display_args: Vec<CmdLinePart>,

    message_sources: Vec<MessageSource>,
    message_queue: Receiver<anyhow::Result<Box<dyn InflightRequest>>>,
    message_input: Sender<anyhow::Result<Box<dyn InflightRequest>>>,

    /// Task which runs the update subprocess
    update_task: Option<JoinHandle<io::Result<ExitStatus>>>,

    /// In case a new update is requested, while an existing one is still running, this will buffer
    /// the next update, in order to execute it once the first one finishes.
    /// Only one update is buffered, if a third update arrives, while the first is still running,
    /// the second one is discarded in favor for the third.
    pending_update: Option<Command>,

    number_retries: u32,

    /// File where persistent state should be stored
    /// If None, then no persistent state is stored
    storage_file: Option<PathBuf>,
    /// Part of the state that can be persisted to the disk and loaded on restart
    persistent: PersistentState,
}

#[derive(Serialize, Deserialize)]
struct PersistentState {
    /// Name of the currently selected gallery, if there is one
    pub current_gallery: Option<String>,

    /// Buffer of recently selected items.
    /// If a path would be selected by `select_random_image` that's in this buffer, a new item will
    /// be chosen instead.
    /// Up to `number_retries` attempts will be done at selecting an image.
    pub recenty_selected: Mutex<CircularQueue<PathBuf>>,

    /// Whether the daemon is currently paused (true) or cycling through images (false).
    /// See `Request::Pause`
    #[serde(default = "default_paused")]
    pub is_paused: bool,
}

fn default_paused() -> bool { false }

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
    pub fn new<T, S>(update_command: T, update_interval: Duration) -> Result<Self>
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

        // arbitrary message limit
        let (sender, receiver) = mpsc::channel(42);

        Ok(ApplicationState {
            galleries: HashMap::new(),
            update_interval: PausableInterval::new(update_interval),
            display_command: cmd,
            display_args: parse_args(cmdline).collect(),
            message_sources: Vec::new(),
            message_queue: receiver,
            message_input: sender,
            update_task: None,
            pending_update: None,
            number_retries: default_retries(),
            storage_file: Some("gallerica.json".into()),
            persistent: PersistentState {
                current_gallery: None,
                recenty_selected: Mutex::new(CircularQueue::with_capacity(default_buffer_size())),
                is_paused: false,
            },
        })
    }

    pub fn add_gallery(&mut self, gallery: Gallery) {
        self.galleries.insert(gallery.name.clone(), gallery);
    }

    pub fn update_persistent_state(&mut self, new_state: PersistentState) -> Result<()> {
        let target_capacity = self.persistent.recenty_selected.lock().unwrap().capacity();
        let actual_capacity = new_state.recenty_selected.lock().unwrap().capacity();

        if actual_capacity != target_capacity {
            let new: &mut CircularQueue<_> = &mut new_state.recenty_selected.lock().unwrap();
            let old = std::mem::replace(new, CircularQueue::with_capacity(target_capacity));

            for item in old.asc_iter() {
                new.push(item.to_path_buf());
            }
        }

        if let Some(gallery) = &new_state.current_gallery {
            if !self.galleries.contains_key(gallery) {
                bail!("State uses invalid gallery '{}'", gallery);
            }
        }

        self.persistent = new_state;
        self.update_interval.pause(self.persistent.is_paused);
        Ok(())
    }

    pub fn change_gallery(&mut self, name: &str) -> Result<()> {
        if !self.galleries.contains_key(name) {
            bail!("Invalid gallery '{}'", name);
        }
        self.persistent.current_gallery = Some(name.to_owned());
        Ok(())
    }

    pub async fn connect_listener(
        &mut self,
        listener: &ListenerConfiguration,
    ) -> anyhow::Result<()> {
        let source: Box<dyn MessageReceiver + Send> = match listener {
            ListenerConfiguration::UnixSocket(cfg) => Box::new(UnixSocketReceiver::new(cfg).await?),
            ListenerConfiguration::Mqtt(cfg) => Box::new(MqttReceiver::new(cfg).await?),
        };

        self.message_sources
            .push(MessageSource::new(source, self.message_input.clone()));
        Ok(())
    }

    pub async fn update(&mut self) {
        let mut cmd = Command::new(&self.display_command);

        let replacement = match self.select_random_image().await {
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

        self.persist();

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

    /// Iterate all folders of the `current_gallery` and select one file at random.
    /// Previously selected files will be buffered in `recenty_selected` and are less likely to be
    /// selected again.
    async fn select_random_image(&self) -> Option<PathBuf> {
        let source_folders = &self
            .galleries
            .get(self.persistent.current_gallery.as_ref()?)?
            .sources;

        let mut rng = rand::thread_rng();

        let all_files: Vec<_> = source_folders
            .iter()
            .filter_map(|dir| read_dir(dir).ok())
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.path())
            .collect();

        let mut tries_left = self.number_retries;
        loop {
            let selection = match all_files.choose(&mut rng) {
                Some(p) => p,
                None => return None,
            };

            if tries_left == 0 {
                return Some(selection.to_path_buf());
            }

            let mut buf = self.persistent.recenty_selected.lock().unwrap();

            if !buf.iter().any(|e| e == selection) {
                buf.push(selection.to_path_buf());
                return Some(selection.to_path_buf());
            }

            tries_left -= 1;
        }
    }

    async fn handle_message(&mut self, msg: Box<dyn InflightRequest>) {
        use Request::*;

        let response = match msg.request() {
            Ok(NextImage) => {
                self.update().await;
                self.update_interval.reset();
                Response::NewImage
            }
            Ok(UpdateInterval { millis }) => {
                let was_paused = self.update_interval.is_paused();
                self.update_interval = PausableInterval::new(Duration::from_millis(*millis));
                self.update_interval.pause(was_paused);
                Response::NewImage
            }
            Ok(SelectGallery { name, refresh }) => {
                if let Err(err) = self.change_gallery(name) {
                    eprintln!("Failed to change gallery to '{name}': {err}");
                    Response::InvalidGallery
                } else {
                    if *refresh {
                        self.update().await;
                    }

                    Response::NewImage
                }
            }
            Ok(s @ Pause | s @ Resume) => {
                self.update_interval.pause(matches!(s, Pause));
                self.persistent.is_paused = self.update_interval.is_paused();
                self.persist();
                Response::Ok
            }
            Err(err) => Response::BadRequest {
                message: err.to_string(),
            },
        };

        let result = msg.respond(response).await;

        if let Err(err) = result {
            eprintln!("Error responding to request: {err}");
        }
    }

    pub async fn run(&mut self) {
        pin! {
            let shutdown_task = tokio::spawn(shutdown_signal_received());
        }
        loop {
            select! {
                TickResult::Completed = self.update_interval.tick() => {
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

                Some(message) = self.message_queue.recv() => {
                    match message {
                        Ok(message) => self.handle_message(message).await,
                        Err(err) => { eprintln!("Error while receiving messages!: {err}"); return; },
                    }
                },

                _ = &mut shutdown_task => break,
            }
        }
    }

    pub async fn update_configuration(&mut self, config: &Configuration) -> Result<()> {
        for mut gallery in config.galleries.iter().cloned() {
            for folder in gallery.sources.iter_mut() {
                if let Cow::Owned(path) = expand_tilde(folder)? {
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

        self.update_interval =
            PausableInterval::new(Duration::from_millis(config.update_interval_ms));

        for listener in &config.listeners {
            self.connect_listener(listener).await?;
        }

        self.number_retries = config.number_retries;
        {
            let mut buf = self.persistent.recenty_selected.lock().unwrap();
            if config.recent_image_buffer_size != buf.capacity() {
                *buf = CircularQueue::with_capacity(config.recent_image_buffer_size);
            }
        }

        self.storage_file = config.storage_file.clone();

        if let Some(filename) = &self.storage_file {
            let dirs = project_dirs();
            let state_dir = dirs.state_dir().unwrap_or_else(|| dirs.cache_dir());
            std::fs::create_dir_all(state_dir)?;
            let state_file = state_dir.join(filename);
            if state_file.exists() {
                let make_ctx = || anyhow!("Failed to open state file '{}'", state_file.display());
                let mut state = std::fs::File::open(&state_file).with_context(make_ctx)?;

                let mut text = String::new();
                state.read_to_string(&mut text).with_context(make_ctx)?;

                let state: PersistentState =
                    serde_json::from_str(&text).context("Failed to parse persisted state")?;
                if self.update_persistent_state(state).is_err() {
                    std::fs::remove_file(&state_file).with_context(|| {
                        anyhow!(
                            "Failed to delete corrupt state file '{}'",
                            state_file.display()
                        )
                    })?;
                }
            }
        }

        if !config.update_immediately {
            self.update_interval.tick().await;
        }

        Ok(())
    }

    /// Store persistent state to disk if configured as such.
    /// If state persistence is turned off, then this is as no-op.
    fn persist(&self) {
        if let Some(filename) = &self.storage_file {
            let dirs = project_dirs();
            let state_file = dirs
                .state_dir()
                .unwrap_or_else(|| dirs.cache_dir())
                .join(filename);
            let result: Result<()> = (|| {
                let state_file = std::fs::File::create(state_file)?;
                serde_json::to_writer(state_file, &self.persistent)?;
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("Error persisting state: '{e}'");
            }
        }

    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum ListenerConfiguration {
    UnixSocket(UnixListenerConfig),
    #[serde(rename = "MQTT")]
    Mqtt(MqttListenerConfig),
}

fn default_listeners() -> Vec<ListenerConfiguration> {
    vec![ListenerConfiguration::UnixSocket(Default::default())]
}

fn default_buffer_size() -> usize {
    3
}
fn default_retries() -> u32 {
    3
}
fn default_update_immediately() -> bool {
    true
}

#[derive(Deserialize, Debug)]
struct Configuration {
    pub command_line: String,
    pub update_interval_ms: u64,
    pub default_gallery: String,
    pub galleries: Vec<Gallery>,

    /// Whether a new image should be selected immediately on startup (true) or only after the first
    /// time interval has passed (false).
    #[serde(default = "default_update_immediately")]
    pub update_immediately: bool,

    #[serde(default = "default_listeners")]
    pub listeners: Vec<ListenerConfiguration>,

    /// Number of image paths the daemon will remember.
    /// Each time an image is selected, the path to that image will be cached.
    /// If selecting a new random image would result an image in this cache,
    /// then the daemon will reroll and select a new one.
    /// Up to `number_retries` tries at selecting an image are performed.
    #[serde(default = "default_buffer_size")]
    pub recent_image_buffer_size: usize,

    /// Number of tries when avoiding recent images.
    /// Set to zero to disable this feature.
    #[serde(default = "default_retries")]
    pub number_retries: u32,

    /// File where persistent state should be stored.
    /// Relative paths are interpreted relative to the state directory,
    /// or the cache directory if the state directory is not available.
    /// If this option is omitted, no state is persisted.
    pub storage_file: Option<PathBuf>,
}

async fn read_configuration(app: &mut ApplicationState, config_file: &Path) -> Result<()> {
    let make_ctx = || anyhow!("Failed to open config file '{}'", config_file.display());

    let mut cfg = std::fs::File::open(config_file).context(make_ctx())?;

    let mut text = String::new();
    cfg.read_to_string(&mut text).context(make_ctx())?;

    let cfg: Configuration = toml::from_str(&text).context("Failed to parse configuration")?;

    app.update_configuration(&cfg)
        .await
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
    let mut state = ApplicationState::new("echo {image}".split(' '), Duration::from_millis(10000))?;

    let cli = Cli::parse();

    let config_path = if let Some(ref path) = cli.config_file {
        path.clone()
    } else {
        gallerica::project_dirs().config_dir().join("config.toml")
    };

    read_configuration(&mut state, &config_path).await?;

    state.run().await;
    Ok(())
}
