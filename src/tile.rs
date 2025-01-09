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
            max_x,
            min_y,
            max_y,
        }
    }

    pub const fn get_parent(&self) -> Option<Self> {
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

    pub const fn get_children(&self) -> [Self; 4] {
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

    pub fn sort_by_zorder(tiles: &mut [Tile]) {
        tiles.sort_by_key(|tile| Self::morton_code(tile.x, tile.y));
    }

    fn interleave(v: u32) -> u64 {
        let mut result = 0u64;

        for i in 0..32 {
            result |= ((u64::from(v) >> i) & 1) << (2 * i);
        }

        result
    }

    fn morton_code(x: u32, y: u32) -> u64 {
        Self::interleave(x) | (Self::interleave(y) << 1)
    }
}

impl Display for Tile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.zoom, self.x, self.y)
    }
}
