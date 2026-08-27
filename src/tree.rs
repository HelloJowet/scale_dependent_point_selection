use geo::{Distance, Geodesic};
use geo_types::Point;
use geographiclib_rs::{DirectGeodesic, Geodesic as GeographicLibGeodesic};
use rstar::{AABB, RTree, RTreeObject};

#[derive(Debug, Clone)]
pub struct RTreeItem {
    pub id: String,
    pub geom: Point,
    pub score: f32,
}

impl RTreeObject for RTreeItem {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.geom.x(), self.geom.y()])
    }
}

#[derive(Clone)]
pub struct Tree {
    rtree: RTree<RTreeItem>,
    geodesic_object: GeographicLibGeodesic,
}

impl Tree {
    pub fn new() -> Tree {
        let rtree = RTree::new();
        let geodesic_object = GeographicLibGeodesic::wgs84();

        return Tree {
            rtree: rtree,
            geodesic_object: geodesic_object,
        };
    }

    pub fn insert_tree_item(&mut self, id: &str, geom: Point, score: f32) {
        self.rtree.insert(RTreeItem {
            id: id.to_string(),
            geom: geom,
            score: score,
        })
    }

    pub fn get_distance_to_nearest_more_important_neighbor(&self, score: f32, geom: Point, max_query_distance: f64) -> f64 {
        let mut nearest_more_important_neighbor_distance = max_query_distance;

        let lat = geom.y();
        let lon = geom.x();

        let (maxy, _, _) = self.geodesic_object.direct(lat, lon, 0.0, max_query_distance);
        let (_, maxx, _) = self.geodesic_object.direct(lat, lon, 90.0, max_query_distance);
        let (miny, _, _) = self.geodesic_object.direct(lat, lon, 180.0, max_query_distance);
        let (_, minx, _) = self.geodesic_object.direct(lat, lon, 270.0, max_query_distance);

        let bounding_box = AABB::from_corners([minx, miny], [maxx, maxy]);
        for public_transpot_stop_in_bbox in self.rtree.locate_in_envelope(bounding_box) {
            if public_transpot_stop_in_bbox.score > score {
                let distance = Geodesic.distance(public_transpot_stop_in_bbox.geom, geom);

                if distance < nearest_more_important_neighbor_distance {
                    nearest_more_important_neighbor_distance = distance;
                }
            }
        }

        nearest_more_important_neighbor_distance
    }
}
