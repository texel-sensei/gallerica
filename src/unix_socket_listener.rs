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
    async fn receive_message(&mut self) -> anyhow::Result<Box<dyn InflightRequest + Send>> {
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
