use std::{fs::remove_file, io, path::PathBuf};

use anyhow::bail;
use async_trait::async_trait;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tokio::net::UnixListener;

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
        #[clap(long, parse(try_from_str), default_value="true")]
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

        Ok(MQTTReceiver {connection })
    }
}

#[async_trait]
impl MessageReceiver for MQTTReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message> {
        use rumqttc::{Event::Incoming, Packet::Publish};

        loop {
            match self.connection.poll().await? {
                Incoming(Publish(publish)) => {
                    let text = String::from_utf8_lossy(&publish.payload);
                    return Ok(serde_json::from_str(&text)?);
                }
                _ => {}
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
        let path = "pipe".into();
        let listener = UnixListener::bind(&path)?;
        Ok(Self { path, listener })
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
        let (stream, _addr) = self.listener.accept().await?;

        loop {
            stream.readable().await?;
            let mut buf = [0; 4096];
            match stream.try_read(&mut buf) {
                Ok(n) => {
                    return Ok(serde_json::from_slice(&buf[0..n])?);
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => bail!(e),
            }
        }
    }
}
