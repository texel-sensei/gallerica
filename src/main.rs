//! Tool to randomly select images from a set of folders.
//! And supporting an MQTT API to be configured on the fly.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use tokio::{
    process::Command,
    time::{self, Duration, Interval},
};

enum CmdLinePart {
    Literal(OsString),
    Placeholder,
}

struct Gallery {
    name: String,
    sources: Vec<PathBuf>,
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
        text if text.as_ref().to_str() == Some("{}") => Placeholder,
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

    pub async fn update(&mut self) {
        let mut cmd = Command::new(&self.display_command);

        let replacement: &OsStr = "Test image".as_ref();

        use CmdLinePart::*;
        cmd.args(self.display_args.iter().map(|ref a| match a {
            Literal(t) => t.as_ref(),
            Placeholder => replacement,
        }));

        cmd.spawn().unwrap().wait().await.unwrap();
    }
}

#[tokio::main]
async fn main() {
    let mut state = ApplicationState::new(
        "echo hello {}".split(' '),
        time::interval(Duration::from_millis(500)),
    )
    .unwrap();
    state.update().await;
    state.update().await;
}
