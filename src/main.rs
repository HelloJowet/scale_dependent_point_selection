use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use scale_dependent_point_selection::{EnrichmentOptions, enrich_geopackage};

#[derive(Debug, Parser)]
#[command(version, about = "Enrich a GeoPackage point layer with scale-dependent selection values")]
struct Arguments {
    /// Input GeoPackage path.
    #[arg(value_name = "INPUT")]
    input: PathBuf,
    /// Output GeoPackage path.
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,

    /// Point layer to enrich.
    #[arg(long, value_name = "LAYER", required = true)]
    layer: String,
    /// Column containing each feature's unique ID.
    #[arg(long, default_value = "id", value_name = "COLUMN")]
    id_property: String,
    /// Column containing each feature's unique numeric importance.
    #[arg(long, default_value = "importance", value_name = "COLUMN")]
    importance_property: String,
    /// Column in which to write the generated importance rank.
    #[arg(long, default_value = "rank", value_name = "COLUMN")]
    rank_property: String,
    /// Column in which to write the generated isolation distance in metres.
    #[arg(long, default_value = "distance_metres", value_name = "COLUMN")]
    distance_property: String,
    /// Column in which to write the generated minimum zoom.
    #[arg(long, default_value = "min_zoom", value_name = "COLUMN")]
    min_zoom_property: String,
    /// Lowest minimum-zoom value that may be generated.
    #[arg(long, default_value_t = 0, value_name = "INTEGER")]
    min_zoom: i32,
    /// Highest minimum-zoom value that may be generated.
    #[arg(long, default_value_t = 22, value_name = "INTEGER")]
    max_zoom: i32,
    /// Required separation between points in screen pixels.
    #[arg(long, default_value_t = 16.0, value_name = "NUMBER")]
    spacing_pixels: f64,
    /// Maximum neighbor-search distance in metres.
    #[arg(long, default_value_t = 20_004_000.0, value_name = "NUMBER")]
    max_query_distance_metres: f64,
    /// Replace an existing output file.
    #[arg(long)]
    force: bool,
    /// Replace generated columns that already exist.
    #[arg(long)]
    overwrite_properties: bool,
}

fn main() -> ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> Result<()> {
    let options = EnrichmentOptions {
        id_property: arguments.id_property,
        importance_property: arguments.importance_property,
        rank_property: arguments.rank_property,
        distance_property: arguments.distance_property,
        min_zoom_property: arguments.min_zoom_property,
        min_zoom: arguments.min_zoom,
        max_zoom: arguments.max_zoom,
        spacing_pixels: arguments.spacing_pixels,
        max_query_distance_metres: arguments.max_query_distance_metres,
        overwrite_properties: arguments.overwrite_properties,
    };

    enrich_geopackage(arguments.input, arguments.output, &arguments.layer, &options, arguments.force)
}
