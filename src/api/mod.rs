pub mod client;
pub mod models;
pub mod parser;
pub mod grouper;
pub mod anilist;

pub use client::ApiClient;
pub use models::*;
pub use parser::AshdiParser;
pub use grouper::{deduplicate_anime, group_into_franchises, franchise_display_name, representative_idx};
