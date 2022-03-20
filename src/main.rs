//! Tool to randomly select images from a set of folders.
//! And supporting an MQTT API to be configured on the fly.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::PathBuf, fs::read_dir,
};

use rand::prelude::*;

use tokio::{
    process::Command,
    time::{self, Duration, Interval}, select,
};

enum CmdLinePart {
    Literal(OsString),
    Placeholder,
}

struct Gallery {
    sources: Vec<PathBuf>,
}

impl Gallery {
    fn new() -> Self { Self { sources: vec![] } }
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
    ) -> Result<Self, Box<dyn std::error::Error>>
    where
        T: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut cmdline = update_command.into_iter();

        let cmd = cmdline
            .next()
            .ok_or("Need a command".to_string())?
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

    pub fn register_folder(&mut self, gallery: String, folder: PathBuf)
    {
        // TODO: figure out how to get rid of this clone
        let selected_gallery = self.galleries.entry(gallery.clone()).or_insert(Gallery::new());
        selected_gallery.sources.push(folder);

        if self.current_gallery.is_none() {
            self.current_gallery = Some(gallery);
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
            },
        }));

        cmd.spawn().unwrap().wait().await.unwrap();
    }

    fn select_random_image(&self) -> Option<PathBuf> {
        let source_folders = &self.galleries.get(self.current_gallery.as_ref()?)?.sources;

        let mut rng = rand::thread_rng();

        let all_files: Vec<PathBuf> = 
            source_folders.iter()
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

#[tokio::main]
async fn main() {
    let mut state = ApplicationState::new(
        "echo {image}".split(' '),
        time::interval(Duration::from_millis(1000)),
    )
    .unwrap();

    state.register_folder("test".to_owned(), ".".to_string().into());

    state.run().await;
}
