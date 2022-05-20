pub mod message_api;

use directories::ProjectDirs;
pub use message_api::Message;

pub fn project_dirs() -> ProjectDirs {
    ProjectDirs::from("at", "texel", "gallerica").expect("Failed to grab base directory paths!")
}
