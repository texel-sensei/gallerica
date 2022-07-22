use std::{
    fs::{create_dir_all, remove_file},
    path::{Path, PathBuf},
};

use crate::project_dirs;
use crate::{message_api::MessageReceiver, Request};
use anyhow::Context;
use async_trait::async_trait;
use tokio::{io::AsyncReadExt, net::UnixListener};

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
    async fn receive_message(&mut self) -> anyhow::Result<Request> {
        let (mut stream, _addr) = self.listener.accept().await?;

        stream.readable().await?;
        let mut buf = vec![];
        stream.read_to_end(&mut buf).await?;
        Ok(serde_json::from_slice(&buf)?)
    }
}
