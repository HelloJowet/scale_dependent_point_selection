use geo::{Distance, Geodesic};
use geo_types::Point;
use rstar::{AABB, RTree, RTreeObject};

// This rounded-down minimum radius of curvature makes the spherical search envelope conservative for WGS84 geodesic distances.
const MINIMUM_WGS84_RADIUS_METRES: f64 = 6_335_439.0;

#[derive(Debug, Clone)]
pub struct RTreeItem {
    _id: String,
    pub geom: Point,
    pub score: f64,
}

impl RTreeObject for RTreeItem {
    type Envelope = AABB<[f64; 3]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point(unit_vector(self.geom))
    }
}

#[derive(Clone)]
/// A spatial index of geographic points and their importance scores.
///
/// Coordinates use longitude as `x` and latitude as `y`, both in degrees.
/// Higher scores represent more-important points.
pub struct Tree {
    rtree: RTree<RTreeItem>,
}

impl Tree {
    /// Creates an empty spatial index.
    pub fn new() -> Self {
        Self { rtree: RTree::new() }
    }

    /// Inserts a point with its identifier and importance score.
    ///
    /// `geom` must contain longitude as `x` and latitude as `y`.
    /// The score is stored as `f64` so callers do not lose the precision that an `f32` score would discard.
    pub fn insert_tree_item(&mut self, id: &str, geom: Point, score: f64) {
        self.rtree.insert(RTreeItem { _id: id.to_string(), geom, score })
    }

    /// Returns the geodesic distance in metres to the nearest point with a strictly greater score.
    ///
    /// Only points within `max_query_distance` metres are considered.
    /// The method returns `max_query_distance` when no more-important point is found within that radius.
    /// A coincident more-important point produces a distance of zero.
    pub fn get_distance_to_nearest_more_important_neighbor(&self, score: f64, geom: Point, max_query_distance: f64) -> f64 {
        if !max_query_distance.is_finite() || max_query_distance <= 0.0 {
            return max_query_distance;
        }

        let mut nearest_more_important_neighbor_distance = max_query_distance;

        // Unit-sphere coordinates avoid longitude discontinuities at the antimeridian and remain well behaved near the poles.
        let centre = unit_vector(geom);
        let angular_distance = (max_query_distance / MINIMUM_WGS84_RADIUS_METRES).min(std::f64::consts::PI);
        let chord_distance = 2.0 * (angular_distance / 2.0).sin();
        let lower = centre.map(|coordinate| coordinate - chord_distance);
        let upper = centre.map(|coordinate| coordinate + chord_distance);
        let bounding_box = AABB::from_corners(lower, upper);

        for candidate in self.rtree.locate_in_envelope(bounding_box) {
            if candidate.score > score {
                // The R-tree envelope only finds candidates, so the final comparison uses an exact WGS84 geodesic distance.
                let distance = Geodesic.distance(candidate.geom, geom);

                if distance < nearest_more_important_neighbor_distance {
                    nearest_more_important_neighbor_distance = distance;
                }
            }
        }

        nearest_more_important_neighbor_distance
    }
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

fn unit_vector(point: Point) -> [f64; 3] {
    let longitude = point.x().to_radians();
    let latitude = point.y().to_radians();
    let latitude_cosine = latitude.cos();

    [latitude_cosine * longitude.cos(), latitude_cosine * longitude.sin(), latitude.sin()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_neighbors_across_the_antimeridian() {
        let mut tree = Tree::new();
        tree.insert_tree_item("important", Point::new(179.9, 0.0), 2.0);
        let distance = tree.get_distance_to_nearest_more_important_neighbor(1.0, Point::new(-179.9, 0.0), 50_000.0);
        assert!((22_000.0..23_000.0).contains(&distance));
    }

    #[test]
    fn finds_neighbors_across_the_pole() {
        let mut tree = Tree::new();
        tree.insert_tree_item("important", Point::new(0.0, 89.9), 2.0);
        let distance = tree.get_distance_to_nearest_more_important_neighbor(1.0, Point::new(180.0, 89.9), 50_000.0);
        assert!((22_000.0..23_000.0).contains(&distance));
    }

    #[test]
    fn coincident_less_important_point_has_zero_distance() {
        let point = Point::new(12.0, 48.0);
        let mut tree = Tree::new();
        tree.insert_tree_item("important", point, 2.0);
        assert_eq!(tree.get_distance_to_nearest_more_important_neighbor(1.0, point, 1_000.0), 0.0);
    }
}
