use std::{
    fs::{create_dir_all, remove_file},
    path::{Path, PathBuf},
};

use crate::message_api::*;
use crate::project_dirs;
use anyhow::Context;
use async_trait::async_trait;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

fn default_path() -> PathBuf {
    "gallerica.sock".into()
}

#[derive(serde::Deserialize, Debug)]
pub struct UnixListenerConfig {
    #[serde(default = "default_path")]
    pub path_to_socket: PathBuf,
}

impl Default for UnixListenerConfig {
    fn default() -> Self {
        Self {
            path_to_socket: default_path(),
        }
    }
}

struct UnixRequest {
    pub request: anyhow::Result<Request>,
    pub stream: UnixStream,
}

#[async_trait]
impl InflightRequest for UnixRequest {
    fn request(&self) -> anyhow::Result<&Request> {
        self.request
            .as_ref()
            .map_err(|e| anyhow::format_err!(e.to_string()))
    }

    async fn respond(mut self: Box<Self>, response: Response) -> anyhow::Result<()> {
        self.stream.writable().await?;
        self.stream
            .write_all(&serde_json::to_vec(&response)?)
            .await?;
        self.stream.shutdown().await?;
        Ok(())
    }
}

pub struct UnixSocketReceiver {
    path: PathBuf,
    listener: UnixListener,
}

impl UnixSocketReceiver {
    pub async fn new(config: &UnixListenerConfig) -> anyhow::Result<Self> {
        let dirs = project_dirs();
        let path = dirs.runtime_dir().unwrap_or_else(|| Path::new("/tmp"));
        let file = path.join(&config.path_to_socket);

        (|| {
            create_dir_all(path)?;

            let listener = match UnixListener::bind(&file) {
                Ok(l) => l,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                    // there is already a socket. Check if that socket is in use,
                    // if not we assume it's from a previous gallerica run and we can delete it.

                    // check if we can connect. If the socket is live, the connection works and we
                    // need to bail
                    let Err(connection_error) = std::os::unix::net::UnixStream::connect(&file) else { anyhow::bail!(e) };

                    // If we get a connection refused, then the socket is dead
                    if connection_error.kind() == std::io::ErrorKind::ConnectionRefused {
                        std::fs::remove_file(&file)?;
                    }

                    // Try again
                    UnixListener::bind(&file)?
                }
                Err(e) => anyhow::bail!(e),
            };

            anyhow::Ok(Self {
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
    async fn receive_message(&mut self) -> anyhow::Result<Box<dyn InflightRequest>> {
        let (mut stream, _addr) = self.listener.accept().await?;

        stream.readable().await?;
        let mut buf = vec![];
        stream.read_to_end(&mut buf).await?;
        Ok(Box::new(UnixRequest {
            request: serde_json::from_slice(&buf).map_err(|e| e.into()),
            stream,
        }))
    }
}
