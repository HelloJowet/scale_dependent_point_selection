# Scale Dependent Point Selection

This crate finds the distance from a geographic point to the nearest point with a higher importance score.

Each stored point has an ID, geographic coordinates, and a score.
The crate uses an R-tree to find nearby candidates efficiently and calculates geographic distances in meters.

## Installation

```toml
[dependencies]
scale_dependent_point_selection = "0.1.0"
geo-types = "0.7"
```

## Usage

```rust
use geo_types::Point;
use scale_dependent_point_selection::Tree;

let mut tree = Tree::new();

tree.insert_tree_item("central-station", Point::new(13.3694, 52.5251), 10.0);
tree.insert_tree_item("local-stop", Point::new(13.3777, 52.5163), 5.0);

let distance = tree.get_distance_to_nearest_more_important_neighbor(
    4.0,
    Point::new(13.3800, 52.5150),
    5_000.0,
);

println!("The nearest more important point is {distance:.0} meters away.");
```

`Point::new` takes longitude first and latitude second.
The score determines importance, so a point is considered more important only when its score is higher than the queried score.
If no more important point is found within `max_query_distance`, the method returns `max_query_distance`.
