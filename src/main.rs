//! Tool to randomly select images from a set of folders.
//! And supporting an MQTT API to be configured on the fly.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::read_dir,
    path::{Path, PathBuf}, io::Read,
};

use anyhow::{Result, anyhow, Context, bail};

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

#[derive(Deserialize, Debug)]
struct Gallery {
    name: String,
    #[serde(rename = "folders")]
    sources: Vec<PathBuf>,
}

impl Gallery {
    fn new(name: &str) -> Self {
        Self { name: name.to_owned(), sources: vec![] }
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

#[derive(Deserialize, Debug)]
struct Configuration {
    pub command_line: String,
    pub default_gallery: String,
    pub galleries: Vec<Gallery>
}

fn read_configuration(
    app: &mut ApplicationState,
    config_file: &Path,
) -> Result<()> {
    let mut cfg = std::fs::File::open(config_file)?;

    let mut text = String::new();
    cfg.read_to_string(&mut text).context("Failed to read configuration file")?;

    let cfg: Configuration = toml::from_str(&text).context("Failed to parse configuration")?;

    for gallery in cfg.galleries.into_iter() {
        app.add_gallery(gallery);
    }

    app.change_gallery(&cfg.default_gallery)?;
    dbg!(cfg.command_line);

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
