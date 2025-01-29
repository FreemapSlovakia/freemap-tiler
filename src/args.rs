use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Input raster geofile
    #[arg(long)]
    pub source_file: PathBuf,

    /// Output *.mbtiles file
    #[arg(long)]
    pub target_file: PathBuf,

    /// Max zoom level
    #[arg(long)]
    pub max_zoom: u8,

    /// Source SRS
    #[arg(long)]
    pub source_srs: Option<String>,

    /// Projection transformation pipeline
    #[arg(long)]
    pub transform_pipeline: Option<String>,

    /// Bounding polygon in `GeoJSON`` file
    #[arg(long)]
    pub bounding_polygon: Option<PathBuf>,

    /// Tile size
    #[arg(long, default_value_t = 256)]
    pub tile_size: u16,

    /// Number of threads for parallel processing [default: available parallelism]
    #[arg(long)]
    pub num_threads: Option<u16>,

    /// JPEG quality
    #[arg(long, default_value_t = 85)]
    pub jpeg_quality: u8,

    /// Resume
    #[arg(long, default_value_t = false)]
    pub resume: bool,

    /// Debug
    #[arg(long, default_value_t = false)]
    pub debug: bool,
}
