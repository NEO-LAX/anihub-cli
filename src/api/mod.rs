pub mod client;
pub mod franchise;
pub mod grouper;
pub mod models;
pub mod moonanime;
pub mod parser;
pub mod resource;

pub use client::ApiClient;
pub use franchise::{
    AniListMedia, FranchiseCatalog, ReleaseAvailability, ReleaseClassification, ReleaseEntry,
    build_franchise_catalogs,
};
pub use grouper::{deduplicate_anime, franchise_display_name};
pub use models::*;
pub use moonanime::MoonAnimeParser;
pub use parser::AshdiParser;
pub use resource::{
    RequestId, ResourceEvent, ResourceKey, ResourceValue, ResourceWorker, ResourceWorkerRuntime,
    ViewGeneration,
};
