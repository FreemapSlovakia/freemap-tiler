use clap::{ArgAction, Parser};
use serde::Serialize;
use std::path::PathBuf;

#[derive(clap::ValueEnum, Clone, Default, Debug, Serialize, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    JPEG,
    PNG,
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Input raster geofile
    #[arg(long)]
    pub source_file: PathBuf,

    /// Output *.mbtiles file
    #[arg(long)]
    pub target_file: PathBuf,

    /// Continue *.mbtiles file, use same as target-file to continue to the same file.
    #[arg(long)]
    pub continue_file: Option<PathBuf>,

    /// Max zoom level
    #[arg(long)]
    pub max_zoom: u8,

    /// Source SRS
    #[arg(long)]
    pub source_srs: Option<String>,

    /// Projection transformation pipeline
    #[arg(long)]
    pub transform_pipeline: Option<String>,

    /// Bounding polygon in `GeoJSON` file
    #[arg(long)]
    pub bounding_polygon: Option<PathBuf>,

    /// Tile size
    #[arg(long, default_value_t = 256)]
    pub tile_size: u16,

    /// Number of threads for parallel processing [default: available parallelism]
    #[arg(long)]
    pub num_threads: Option<u16>,

    #[arg(long, default_value_t, value_enum)]
    pub format: Format,

    /// JPEG quality
    #[arg(long, default_value_t = 85)]
    pub jpeg_quality: u8,

    /// Advanced: zoom offset of a parent tile to reproject at once. Modify to fine-tune the performance.
    #[arg(long, default_value_t = 3)]
    pub warp_zoom_offset: u8,

    /// Debug
    #[arg(long, default_value_t = false)]
    pub debug: bool,

    /// Insert empty
    #[arg(long, action = ArgAction::Set, default_value_t = true, default_missing_value = "true", num_args = 0..=1, require_equals = false)]
    pub insert_empty: bool,
}
