//! Tool to randomly select images from a set of folders.
//! And supporting an MQTT API to be configured on the fly.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::read_dir,
    path::{Path, PathBuf}, io::Read,
};

use anyhow::{Result, anyhow, Context};

use rand::prelude::*;
use serde::Deserialize;

use tokio::{
    process::Command,
    select,
    time::{self, Duration, Interval},
};

enum CmdLinePart {
    Literal(OsString),
    Placeholder,
}

struct Gallery {
    sources: Vec<PathBuf>,
}

impl Gallery {
    fn new() -> Self {
        Self { sources: vec![] }
    }
}

struct ApplicationState {
    galleries: HashMap<String, Gallery>,
    current_gallery: Option<String>,
    update_interval: Interval,
    display_command: OsString,
    display_args: Vec<CmdLinePart>,
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
    pub fn new<'a, T, S>(
        update_command: T,
        update_interval: Interval,
    ) -> Result<Self>
    where
        T: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut cmdline = update_command.into_iter();

        let cmd = cmdline
            .next()
            .ok_or(anyhow!("Need a command"))?
            .as_ref()
            .to_os_string();

        Ok(ApplicationState {
            galleries: HashMap::new(),
            current_gallery: None,
            update_interval,
            display_command: cmd,
            display_args: parse_args(cmdline).collect(),
        })
    }

    pub fn register_folder(&mut self, gallery: &str, folder: &Path) {
        let selected_gallery = self
            .galleries
            .entry(gallery.to_owned())
            .or_insert(Gallery::new());
        selected_gallery.sources.push(folder.to_path_buf());

        if self.current_gallery.is_none() {
            self.current_gallery = Some(gallery.to_owned());
        }
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

        cmd.spawn().unwrap().wait().await.unwrap();
    }

    fn select_random_image(&self) -> Option<PathBuf> {
        let source_folders = &self.galleries.get(self.current_gallery.as_ref()?)?.sources;

        let mut rng = rand::thread_rng();

        let all_files: Vec<PathBuf> = source_folders
            .iter()
            .filter_map(|dir| read_dir(dir).ok())
            .flatten()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect();

        all_files.into_iter().choose(&mut rng)
    }

    pub async fn run(&mut self) {
        loop {
            select! {
                _ = self.update_interval.tick() => {
                    self.update().await;
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct GalleryConfig {
    pub name: String,
    pub folders: Vec<PathBuf>
}

#[derive(Deserialize)]
struct Configuration {
    pub command_line: String,
    pub galleries: Vec<GalleryConfig>
}

fn read_configuration(
    app: &mut ApplicationState,
    config_file: &Path,
) -> Result<()> {
    let mut cfg = std::fs::File::open(config_file)?;

    let mut text = String::new();
    cfg.read_to_string(&mut text).context("Failed to read configuration file")?;

    let cfg: Configuration = toml::from_str(&text).context("Failed to parse configuration")?;

    for GalleryConfig{name, folders} in cfg.galleries.iter() {
        for folder in folders.iter() {
            app.register_folder(&name, folder);
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut state = ApplicationState::new(
        "echo {image}".split(' '),
        time::interval(Duration::from_millis(1000)),
    )?;

    read_configuration(&mut state, Path::new("test.toml"))?;

    state.run().await;
    Ok(())
}
