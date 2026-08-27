# Scale-Dependent Point Selection

This project provides a Rust library and command-line tool for deciding when geographic points should appear on a map.
It enriches each GeoJSON point with an importance rank, the distance to the nearest more-important point, and a minimum map zoom.

This is useful when a map contains more points than can be displayed clearly at once.
Important and isolated points can appear at lower zooms, while less-important points that are close to them can wait until the user zooms in.

## How it works

The tool performs the following steps:

1. It reads and validates every input point.
2. It gives rank `1` to the point with the highest importance, rank `2` to the next point, and so on.
3. It measures the shortest distance from each point to any point with a higher importance value.
4. It converts that distance into a minimum zoom using the requested pixel spacing and the point's latitude.
5. It writes the original feature with three additional properties.

The distance in step 3 is called the isolation distance.
A large isolation distance means that a point has plenty of space around it compared with more-important points.
A small isolation distance means that the point is close to something that should appear first.

## Installation

Install the command-line program from crates.io with Cargo:

```console
cargo install scale_dependent_point_selection
```

Run this command to confirm that the program is available:

```console
scale-dependent-point-selection --help
```

To use the project as a Rust library, add these dependencies:

```toml
[dependencies]
scale_dependent_point_selection = "0.2"
geo-types = "0.7"
```

## Common commands

Read `points.geojsonseq` and create `enriched.geojsonseq`:

```console
scale-dependent-point-selection points.geojsonseq enriched.geojsonseq
```

Choose different input property names:

```console
scale-dependent-point-selection --id-property point_id --importance-property priority points.geojsonseq enriched.geojsonseq
```

Use standard input and standard output in a Unix pipeline:

```console
cat points.geojsonseq | scale-dependent-point-selection - - > enriched.geojsonseq
```

The path `-` means standard input when used as `INPUT` and standard output when used as `OUTPUT`.
Using `- -` is supported because the complete input is read before output begins.

Diagnostics are written only to stderr, so GeoJSON written to stdout can safely be redirected or passed to another command.

## Example

The following input contains two generic points.
The `␞` symbol represents the ASCII record separator byte and is shown visibly here only for clarity.

```text
␞{"type":"Feature","geometry":{"type":"Point","coordinates":[13.40,52.52]},"properties":{"id":"point-a","priority":20,"note":null}}
␞{"type":"Feature","geometry":{"type":"Point","coordinates":[13.41,52.52]},"properties":{"id":"point-b","priority":10,"note":"example"}}
```

Run the tool with `priority` as the importance property and request 24 pixels of separation:

```console
scale-dependent-point-selection \
  --importance-property priority \
  --spacing-pixels 24 \
  points.geojsonseq enriched.geojsonseq
```

The compact output is equivalent to the following records:

```text
␞{"geometry":{"coordinates":[13.4,52.52],"type":"Point"},"properties":{"distance_metres":20004000.0,"id":"point-a","min_zoom":0,"note":null,"priority":20,"rank":1},"type":"Feature"}
␞{"geometry":{"coordinates":[13.41,52.52],"type":"Point"},"properties":{"distance_metres":678.7941301122039,"id":"point-b","min_zoom":12,"note":"example","priority":10,"rank":2},"type":"Feature"}
```

`point-a` has the highest priority, so it receives rank `1`.
No point has a greater priority, so its distance uses the configured maximum query distance and its calculated zoom is clamped to `0`.
`point-b` receives rank `2`, is about 679 metres from the more-important `point-a`, and receives minimum zoom `12`.

The original coordinates, `priority` values, `note` values, and other feature data remain present in the output.

## Understanding the generated properties

The default generated properties are:

- `rank` is the position in descending importance order, with `1` representing the highest importance.
- `distance_metres` is the geodesic distance in metres to the nearest point with a strictly greater importance value.
- `min_zoom` is the first map zoom at which the point meets the requested pixel separation, limited to the configured zoom range.

Pixel spacing describes how far apart two points should appear on screen.
A larger `--spacing-pixels` value generally makes less-important nearby points appear at a higher zoom.

If no more-important point is found within `--max-query-distance-metres`, the generated distance is the configured maximum query distance.
If a less-important point has exactly the same coordinates as a more-important point, its distance is zero and its minimum zoom is the configured maximum zoom.

## Input and output format

Both input and output use GeoJSON Text Sequence format.
In this project, every nonempty line contains exactly one GeoJSON Feature.

Input records may optionally start with the ASCII record separator byte (`0x1e`).
Every output record always starts with `0x1e` and ends with a line feed, which provides standards-compliant GeoJSON Text Sequence framing.

Every input feature must meet these rules:

- `type` must be `Feature`.
- Geometry must be a non-null `Point`.
- Coordinates must contain at least longitude and latitude in that order.
- Longitude must be between `-180` and `180`.
- Latitude must be between `-90` and `90`.
- The configured ID property must contain a non-null string or JSON integer.
- IDs must be unique, with numeric IDs compared by integer value.
- The configured importance property must contain a finite number representable as `f64`.
- Importance values must be unique after conversion to `f64`.

Unique importance values are required because a point is more important only when its score is strictly greater.
Rejecting ties prevents results from depending on input order or spatial-index iteration order.

The complete original feature is preserved, including additional coordinate elements such as altitude, arbitrary properties, null values, GeoJSON foreign members, and signed 64-bit integers.
The original importance property is not replaced or recalculated.

Generated property names must be nonempty, distinct from one another, and different from the configured ID and importance property names.
An input feature that already contains a generated property is rejected unless `--overwrite-properties` is supplied.

## Command-line reference

```text
scale-dependent-point-selection [OPTIONS] <INPUT> <OUTPUT>

Arguments:
  <INPUT>   GeoJSON Text Sequence input path, or - for stdin
  <OUTPUT>  GeoJSON Text Sequence output path, or - for stdout

Options:
  --id-property <PROPERTY>                    ID property [default: id]
  --importance-property <PROPERTY>            Numeric importance property [default: importance]
  --rank-property <PROPERTY>                  Generated rank property [default: rank]
  --distance-property <PROPERTY>              Generated distance property [default: distance_metres]
  --min-zoom-property <PROPERTY>              Generated zoom property [default: min_zoom]
  --min-zoom <INTEGER>                        Minimum allowed zoom [default: 0]
  --max-zoom <INTEGER>                        Maximum allowed zoom [default: 22]
  --spacing-pixels <NUMBER>                   Requested pixel separation [default: 16]
  --max-query-distance-metres <NUMBER>        Neighbor search cap in metres [default: 20004000]
  --force                                     Replace an existing output file
  --overwrite-properties                      Replace generated properties already present in a feature
  -h, --help                                  Print help
  -V, --version                               Print version
```

The minimum zoom must not exceed the maximum zoom.
Pixel spacing and maximum query distance must both be positive finite numbers.

## Safe file handling

The tool refuses to replace an existing output file unless `--force` is supplied.
It also rejects input and output paths that refer to the same physical file, including aliases created with symbolic links or hard links.

When writing to a named file, the tool first writes to a temporary file in the destination directory.
It renames that file to the requested destination only after processing, writing, flushing, and synchronization succeed.
If an error occurs, the temporary file is removed and an existing destination remains unchanged.

## Minimum-zoom calculation

Web Mercator ground resolution changes with latitude, so the same geographic distance can occupy a different number of screen pixels at different locations.
The tool uses the latitude of each point in the following calculation:

```text
initial_resolution = 156543.03392804097
resolution_at_latitude = initial_resolution * cos(latitude_in_radians)
required_zoom = ceil(log2(
    spacing_pixels * resolution_at_latitude / isolation_metres
))
minimum_zoom = clamp(required_zoom, configured_min_zoom, configured_max_zoom)
```

If the isolation distance is non-finite or less than or equal to zero, the point receives the configured maximum zoom.

## Library usage

Use `Tree` when you already have Rust point values and only need nearest-more-important distance queries:

```rust
use geo_types::Point;
use scale_dependent_point_selection::Tree;

let mut tree = Tree::new();
tree.insert_tree_item("point-a", Point::new(13.40, 52.52), 20.0);
tree.insert_tree_item("point-b", Point::new(13.41, 52.52), 10.0);

let distance = tree.get_distance_to_nearest_more_important_neighbor(
    10.0,
    Point::new(13.41, 52.52),
    5_000.0,
);

println!("Nearest more-important point: {distance:.0} metres");
```

`Point::new` receives longitude first and latitude second.
The query returns `max_query_distance` when no point with a strictly greater score is found within that distance.

For reusable GeoJSON processing, the crate also exports `EnrichmentOptions`, `enrich_geojson_text_sequence`, and `write_geojson_text_sequence`.
