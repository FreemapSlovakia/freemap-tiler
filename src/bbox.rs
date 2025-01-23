use crate::{geo::WEB_MERCATOR_EXTENT, tile::Tile};
use geo::Polygon;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

impl BBox {
    pub fn compute_covered_tiles(&self, zoom: u8) -> Vec<Tile> {
        let tile_size_meters = (WEB_MERCATOR_EXTENT * 2.0) / f64::from(1 << zoom);

        // Compute the tile range for the given bounding box
        let min_tile_x = ((self.min_x + WEB_MERCATOR_EXTENT) / tile_size_meters).floor() as u32;
        let max_tile_x = ((self.max_x + WEB_MERCATOR_EXTENT) / tile_size_meters).ceil() as u32 - 1;
        let min_tile_y = ((WEB_MERCATOR_EXTENT - self.max_y) / tile_size_meters).floor() as u32;
        let max_tile_y = ((WEB_MERCATOR_EXTENT - self.min_y) / tile_size_meters).ceil() as u32 - 1;

        // Collect all tile coordinates in the range
        let mut tiles = Vec::new();

        for x in min_tile_x..=max_tile_x {
            for y in min_tile_y..=max_tile_y {
                tiles.push(Tile { zoom, x, y });
            }
        }

        tiles
    }

    pub fn to_polygon(&self) -> Polygon<f64> {
        Polygon::new(
            geo::LineString::from(vec![
                (self.min_x, self.min_y),
                (self.max_x, self.min_y),
                (self.max_x, self.max_y),
                (self.min_x, self.max_y),
                (self.min_x, self.min_y),
            ]),
            vec![],
        )
    }
}
