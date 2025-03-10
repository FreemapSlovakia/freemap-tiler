use gdal::Dataset;

use crate::bbox::BBox;

pub const EARTH_RADIUS: f64 = 6_378_137.0; // Equatorial radius of the Earth in meters (WGS 84)

pub const WEB_MERCATOR_EXTENT: f64 = std::f64::consts::PI * EARTH_RADIUS;

pub fn compute_bbox(dataset: &Dataset) -> BBox {
    let geo_transform = dataset.geo_transform().unwrap();

    let min_x = geo_transform[0]; // Top-left x
    let max_y = geo_transform[3]; // Top-left y
    let pixel_width = geo_transform[1];
    let pixel_height = geo_transform[5]; // Note: Typically negative for top-down

    // Get dataset size
    let raster_size = dataset.raster_size();

    // Calculate max_x and min_y
    let max_x = (raster_size.0 as f64).mul_add(pixel_width, min_x);
    let min_y = (raster_size.1 as f64).mul_add(pixel_height, max_y);

    BBox {
        min_x,
        max_x,
        min_y,
        max_y,
    }
}
