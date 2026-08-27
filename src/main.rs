use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::Parser;
use scale_dependent_point_selection::{EnrichmentOptions, enrich_geojson_text_sequence, write_geojson_text_sequence};

#[derive(Debug, Parser)]
#[command(version, about = "Enrich GeoJSON point features with scale-dependent selection values")]
struct Arguments {
    /// GeoJSON Text Sequence input path, or '-' for standard input.
    #[arg(value_name = "INPUT")]
    input: PathBuf,
    /// GeoJSON Text Sequence output path, or '-' for standard output.
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,

    /// Property containing each feature's unique ID.
    #[arg(long, default_value = "id", value_name = "PROPERTY")]
    id_property: String,
    /// Property containing each feature's unique numeric importance.
    #[arg(long, default_value = "importance", value_name = "PROPERTY")]
    importance_property: String,
    /// Property in which to write the generated importance rank.
    #[arg(long, default_value = "rank", value_name = "PROPERTY")]
    rank_property: String,
    /// Property in which to write the generated isolation distance in metres.
    #[arg(long, default_value = "distance_metres", value_name = "PROPERTY")]
    distance_property: String,
    /// Property in which to write the generated minimum zoom.
    #[arg(long, default_value = "min_zoom", value_name = "PROPERTY")]
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
    /// Replace configured generated properties that already exist in a feature.
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
    let input_is_stdio = arguments.input == Path::new("-");
    let output_is_stdio = arguments.output == Path::new("-");

    if !input_is_stdio && !output_is_stdio && arguments.output.exists() {
        if same_file::is_same_file(&arguments.input, &arguments.output)
            .with_context(|| format!("failed to compare input '{}' and output '{}'", arguments.input.display(), arguments.output.display()))?
        {
            bail!("input and output refer to the same physical file");
        }
        if !arguments.force {
            bail!("output file '{}' already exists; use --force to replace it", arguments.output.display());
        }
    }

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

    let features = if input_is_stdio {
        enrich_geojson_text_sequence(BufReader::new(io::stdin().lock()), &options).context("failed to process standard input")?
    } else {
        let file = File::open(&arguments.input).with_context(|| format!("failed to open input file '{}'", arguments.input.display()))?;
        enrich_geojson_text_sequence(BufReader::new(file), &options).with_context(|| format!("failed to process input file '{}'", arguments.input.display()))?
    };

    if output_is_stdio {
        let mut output = BufWriter::new(io::stdout().lock());
        write_geojson_text_sequence(&features, &mut output).context("failed to write standard output")?;
        output.flush().context("failed to flush standard output")?;
        return Ok(());
    }

    write_atomic(&arguments.output, &features, arguments.force)
}

fn write_atomic(destination: &Path, features: &[serde_json::Value], force: bool) -> Result<()> {
    let parent = destination.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    // Keeping the temporary file beside the destination allows the final rename to stay on one filesystem and therefore remain atomic.
    let mut temporary = tempfile::Builder::new()
        .prefix(".scale-dependent-point-selection-")
        .tempfile_in(parent)
        .with_context(|| format!("failed to create temporary output in '{}'", parent.display()))?;

    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        write_geojson_text_sequence(features, &mut writer).with_context(|| format!("failed to write temporary output for '{}'", destination.display()))?;
        writer.flush().context("failed to flush temporary output")?;
    }
    temporary.as_file().sync_all().context("failed to synchronize temporary output")?;

    // The no-clobber operation closes the race between the earlier existence check and the final rename.
    if force {
        temporary
            .persist(destination)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to replace output file '{}'", destination.display()))?;
    } else {
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to create output file '{}'; it may already exist", destination.display()))?;
    }

    Ok(())
}
