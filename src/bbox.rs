use crate::{geo::WEB_MERCATOR_EXTENT, tile::Tile};
use geo::Polygon;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

pub struct TileIterator {
    zoom: u8,
    x: u32,
    y: u32,
    min_tile_x: u32,
    max_tile_x: u32,
    max_tile_y: u32,
}

impl Iterator for TileIterator {
    type Item = Tile;

    fn next(&mut self) -> Option<Self::Item> {
        if self.x > self.max_tile_x {
            self.x = self.min_tile_x;

            self.y += 1;

            if self.y > self.max_tile_y {
                return None;
            }
        } else {
            self.x += 1;
        }

        Some(Tile {
            zoom: self.zoom,
            x: self.x,
            y: self.y,
        })
    }
}

impl BBox {
    pub fn to_polygon(self) -> Polygon<f64> {
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

pub fn covered_tiles(bbox: &BBox, zoom: u8) -> TileIterator {
    let tile_size_meters = (WEB_MERCATOR_EXTENT * 2.0) / f64::from(1 << zoom);

    // Compute the tile range for the given bounding box
    let min_tile_x = ((bbox.min_x + WEB_MERCATOR_EXTENT) / tile_size_meters).floor() as u32;
    let max_tile_x = ((bbox.max_x + WEB_MERCATOR_EXTENT) / tile_size_meters).ceil() as u32 - 1;
    let min_tile_y = ((WEB_MERCATOR_EXTENT - bbox.max_y) / tile_size_meters).floor() as u32;
    let max_tile_y = ((WEB_MERCATOR_EXTENT - bbox.min_y) / tile_size_meters).ceil() as u32 - 1;

    TileIterator {
        zoom,
        x: min_tile_x,
        y: min_tile_y,
        min_tile_x,
        max_tile_x,
        max_tile_y,
    }
}
