use geo::Polygon;
use geojson::GeoJson;
use proj::{Proj, Transform};
use std::fs::File;
use std::io::Read;
use std::path::Path;

// Read GeoJSON and parse into a Polygon
pub fn parse_geojson_polygon(file_path: &Path) -> Result<Polygon<f64>, String> {
    let mut file = File::open(file_path).map_err(|e| format!("Failed to open file: {e}"))?;

    let mut geojson_str = String::new();

    file.read_to_string(&mut geojson_str)
        .map_err(|e| format!("Failed to read file: {e}"))?;

    let geojson: GeoJson = geojson_str
        .parse()
        .map_err(|e| format!("Invalid GeoJSON: {e}"))?;

    match geojson {
        GeoJson::Feature(feature) => {
            if let Some(geometry) = feature.geometry {
                Polygon::try_from(geometry.value).map_err(|_| "No polygon found".into())
            } else {
                Err("Feature has no geometry".into())
            }
        }
        GeoJson::FeatureCollection(collection) => {
            for feature in collection.features {
                if let Some(geometry) = feature.geometry {
                    if let Ok(polygon) = Polygon::try_from(geometry.value) {
                        return Ok(polygon);
                    }
                }
            }
            Err("No polygons found in collection".into())
        }
        GeoJson::Geometry(_) => Err("GeoJSON does not contain features".into()),
    }
}

// Reproject a Polygon from EPSG:4326 to EPSG:3857 using geo's Transform
pub fn reproject_polygon(polygon: &mut Polygon<f64>) -> Result<(), String> {
    // Create a Proj instance for EPSG:4326 -> EPSG:3857
    let proj = Proj::new_known_crs("EPSG:4326", "EPSG:3857", None)
        .map_err(|e| format!("Failed to create projection: {e}"))?;

    // Use geo's Transform trait to reproject the polygon
    polygon
        .transform(&proj)
        .map_err(|e| format!("Reprojection failed: {e}"))?;

    Ok(())
}
