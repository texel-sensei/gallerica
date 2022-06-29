use std::{
    fs::{create_dir_all, remove_file},
    path::{Path, PathBuf},
};

use anyhow::Context;
use async_trait::async_trait;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tokio::{io::AsyncReadExt, net::UnixListener};

use crate::project_dirs;

#[derive(Debug, Serialize, Deserialize, Subcommand)]
#[serde(tag = "method")]
pub enum Message {
    /// Immediately show the next image, no matter the update rate.
    NextImage,

    /// Change the time between two images
    UpdateInterval {
        /// Number of milliseconds to wait before showing the next image
        millis: u64,
    },

    /// Choose a new gallery from which images are selected
    SelectGallery {
        /// Name of the new gallery to use
        name: String,

        /// Whether to immediately refresh the display or wait till the next scheduled update
        #[clap(long, parse(try_from_str), default_value = "true")]
        refresh: bool,
    },
}

#[async_trait]
pub trait MessageReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message>;
}

pub struct MQTTReceiver {
    connection: EventLoop,
}

impl MQTTReceiver {
    pub async fn new() -> anyhow::Result<Self> {
        let (client, connection) =
            AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 10);

        client.subscribe("#", QoS::AtLeastOnce).await?;

        Ok(MQTTReceiver { connection })
    }
}

#[async_trait]
impl MessageReceiver for MQTTReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            if let Incoming(Publish(publish)) = self.connection.poll().await? {
                let text = String::from_utf8_lossy(&publish.payload);
                return Ok(serde_json::from_str(&text)?);
            }
        }
    }
}

pub struct UnixSocketReceiver {
    path: PathBuf,
    listener: UnixListener,
}

impl UnixSocketReceiver {
    pub async fn new() -> anyhow::Result<Self> {
        let dirs = project_dirs();
        let path = dirs.runtime_dir().unwrap_or_else(|| Path::new("/tmp"));
        let file = path.join("gallerica.sock");

        use anyhow::Ok;
        (|| {
            create_dir_all(path)?;

            let listener = UnixListener::bind(&file)?;

            Ok(Self {
                path: file.clone(),
                listener,
            })
        })()
        .with_context(|| format!("Failed to create Unix socket at '{}'", file.display()))
    }
}

impl Drop for UnixSocketReceiver {
    fn drop(&mut self) {
        remove_file(&self.path).unwrap();
    }
}

#[async_trait]
impl MessageReceiver for UnixSocketReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message> {
        let (mut stream, _addr) = self.listener.accept().await?;

        stream.readable().await?;
        let mut buf = vec![];
        stream.read_to_end(&mut buf).await?;
        Ok(serde_json::from_slice(&buf)?)
    }
}
