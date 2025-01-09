use crate::{bbox::BBox, geo::WEB_MERCATOR_EXTENT};
use std::fmt::Display;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tile {
    pub zoom: u8,
    pub x: u32,
    pub y: u32,
}

impl Tile {
    pub fn bounds_to_epsg3857(&self, tile_size: u16) -> BBox {
        let tile_size = f64::from(tile_size);

        let total_pixels = tile_size * f64::from(self.zoom).exp2();
        let pixel_size = (2.0 * WEB_MERCATOR_EXTENT) / total_pixels;

        let min_x = (f64::from(self.x) * tile_size).mul_add(pixel_size, -WEB_MERCATOR_EXTENT);
        let max_y = (f64::from(self.y) * tile_size).mul_add(-pixel_size, WEB_MERCATOR_EXTENT);

        let max_x = tile_size.mul_add(pixel_size, min_x);
        let min_y = tile_size.mul_add(-pixel_size, max_y);

        BBox {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn get_parent(&self) -> Option<Self> {
        if self.zoom == 0 {
            None
        } else {
            Some(Self {
                x: self.x / 2,
                y: self.y / 2,
                zoom: self.zoom - 1,
            })
        }
    }

    pub fn get_children(&self) -> [Self; 4] {
        let zoom = self.zoom + 1;

        [
            Self {
                x: self.x * 2,
                y: self.y * 2,
                zoom,
            },
            Self {
                x: self.x * 2 + 1,
                y: self.y * 2,
                zoom,
            },
            Self {
                x: self.x * 2,
                y: self.y * 2 + 1,
                zoom,
            },
            Self {
                x: self.x * 2 + 1,
                y: self.y * 2 + 1,
                zoom,
            },
        ]
    }
}

impl Display for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.zoom, self.x, self.y)
    }
}
