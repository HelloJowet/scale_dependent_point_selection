# Scale-Dependent Point Selection

This Rust library and command-line tool decides when points should appear on a map. It adds an importance rank, the distance to the nearest more-important point, and a minimum map zoom to a GeoPackage point layer.

Important and isolated points can appear at lower zooms. Less-important points close to them wait until the user zooms in.

## Installation

Install the command-line tool with Cargo:

```console
cargo install scale_dependent_point_selection
```

## Requirements

The input must be a GeoPackage containing a Point layer in EPSG:4326. The layer must have one INTEGER primary key and these columns by default:

- `id`: a unique TEXT or INTEGER value.
- `importance`: a unique, finite INTEGER or REAL value. Larger values are more important.

Use command-line options if your columns have different names.

## Usage

Enrich the `places` layer in `points.gpkg` and write the result to `enriched.gpkg`:

```console
scale-dependent-point-selection --layer places points.gpkg enriched.gpkg
```

Choose different input columns:

```console
scale-dependent-point-selection --layer places --id-property place_id --importance-property priority points.gpkg enriched.gpkg
```

The tool adds these columns:

- `rank`: position in descending importance order. Rank `1` is the most important point.
- `distance_metres`: distance to the nearest point with greater importance.
- `min_zoom`: first map zoom at which the point meets the requested spacing.

The complete input GeoPackage is copied to the output. Only the selected layer is changed. Geometry, other columns, other layers, and auxiliary tables are preserved.

The output file must not exist unless `--force` is used. If processing fails, an existing output file is left unchanged.

## Common options

```text
--layer <LAYER>                         Point layer to enrich (required)
--id-property <COLUMN>                  ID column [default: id]
--importance-property <COLUMN>          Importance column [default: importance]
--rank-property <COLUMN>                Rank output column [default: rank]
--distance-property <COLUMN>            Distance output column [default: distance_metres]
--min-zoom-property <COLUMN>            Zoom output column [default: min_zoom]
--min-zoom <INTEGER>                    Lowest generated zoom [default: 0]
--max-zoom <INTEGER>                    Highest generated zoom [default: 22]
--spacing-pixels <NUMBER>               Required point spacing [default: 16]
--max-query-distance-metres <NUMBER>    Search distance limit [default: 20004000]
--overwrite-properties                  Replace generated columns that already exist
--force                                 Replace an existing output file
```

## How minimum zoom is calculated

The tool finds the nearest point with greater importance and converts that distance to a Web Mercator zoom. The calculation uses the point's latitude and the requested pixel spacing. Results are limited to `--min-zoom` and `--max-zoom`.

If no more-important point is found within the search limit, `--max-query-distance-metres` is used as the distance. Coincident points receive the configured maximum zoom.

## Rust library

Use `enrich_geopackage` for the same file-based workflow from Rust:

```rust,no_run
use scale_dependent_point_selection::{EnrichmentOptions, enrich_geopackage};

let options = EnrichmentOptions::default();
enrich_geopackage("points.gpkg", "enriched.gpkg", "places", &options, false).expect("failed to enrich GeoPackage");
```

Use `Tree` directly when you only need nearest-more-important distance queries:

```rust
use geo_types::Point;
use scale_dependent_point_selection::Tree;

let mut tree = Tree::new();
tree.insert_tree_item("a", Point::new(13.40, 52.52), 20.0);
tree.insert_tree_item("b", Point::new(13.41, 52.52), 10.0);

let distance = tree.get_distance_to_nearest_more_important_neighbor(10.0, Point::new(13.41, 52.52), 5_000.0);
println!("{distance:.0} metres");
```
