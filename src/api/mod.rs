pub mod client;
pub mod grouper;
pub mod models;
pub mod parser;
pub mod resource;

pub use client::ApiClient;
pub use grouper::{
    deduplicate_anime, franchise_display_name, group_into_franchises, representative_idx,
};
pub use models::*;
pub use parser::AshdiParser;
pub use resource::{
    RequestId, ResourceEvent, ResourceKey, ResourceValue, ResourceWorker, ResourceWorkerRuntime,
    ViewGeneration,
};
