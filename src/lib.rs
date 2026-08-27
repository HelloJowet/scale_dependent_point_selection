//! Tools for enriching geographic point features with scale-dependent selection values.
//!
//! Use [`Tree`] when you only need spatial queries in Rust.
//! Use [`enrich_geojson_text_sequence`] and [`write_geojson_text_sequence`] when you want to validate, enrich, and serialize GeoJSON Text Sequence records.

mod processing;
mod tree;

pub use processing::{EnrichmentOptions, enrich_geojson_text_sequence, write_geojson_text_sequence};
pub use tree::Tree;
