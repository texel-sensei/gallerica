use std::{fs::remove_file, io, path::PathBuf};

use anyhow::bail;
use async_trait::async_trait;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
};

#[derive(Debug, Serialize, Deserialize, Subcommand)]
#[serde(tag = "method")]
pub enum Message {
    /// Immediately show the next image, no matter the update rate.
    NextImage,
    UpdateInterval {
        millis: u64,
    },
}

#[async_trait]
pub trait MessageReceiver {
    async fn receive_message(&mut self) -> anyhow::Result<Message>;
}

pub struct MQTTReceiver {
    client: AsyncClient,
    connection: EventLoop,
}

impl MQTTReceiver {
    pub async fn new() -> anyhow::Result<Self> {
        let (client, connection) =
            AsyncClient::new(MqttOptions::new("test", "localhost", 1883), 10);

        client.subscribe("#", QoS::AtLeastOnce).await?;

        Ok(MQTTReceiver { client, connection })
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
        let (stream, addr) = self.listener.accept().await?;

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
