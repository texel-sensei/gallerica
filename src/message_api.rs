use async_trait::async_trait;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Subcommand)]
#[serde(tag = "method")]
pub enum Request {
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
    async fn receive_message(&mut self) -> anyhow::Result<Request>;
}
