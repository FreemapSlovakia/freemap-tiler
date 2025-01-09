mod bbox;
mod geo;
mod tile;

use bbox::BBox;
use clap::Parser;
use crossbeam_deque::{Steal, Worker};
use gdal::spatial_ref::{CoordTransform, CoordTransformOptions, SpatialRef};
use gdal::{Dataset, DriverManager};
use gdal_sys::{
    CPLErr, GDALChunkAndWarpImage, GDALCreateGenImgProjTransformer2, GDALCreateWarpOperation,
    GDALCreateWarpOptions, GDALDestroyGenImgProjTransformer, GDALDestroyWarpOperation,
    GDALDestroyWarpOptions, GDALGenImgProjTransform, GDALResampleAlg,
    GDALWarpInitDefaultBandMapping,
};
use image::imageops::FilterType;
use image::{ImageBuffer, RgbImage};
use jpeg_encoder::{ColorType, Encoder};
use rusqlite::{Connection, Error};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, available_parallelism};
use std::time::{SystemTime, UNIX_EPOCH};
use tile::Tile;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Input raster geofile
    #[arg(long)]
    source_file: PathBuf,

    /// Output *.mbtiles file
    #[arg(long)]
    target_file: PathBuf,

    /// Max zoom level
    #[arg(long)]
    max_zoom: u8,

    /// Source SRS
    #[arg(long)]
    source_srs: Option<String>,

    /// Projection transformation pipeline
    #[arg(long)]
    transform_pipeline: Option<String>,

    /// Tile size
    #[arg(long, default_value_t = 256)]
    tile_size: u16,

    /// Number of threads for parallel processing
    #[arg(long, default_value = "available parallelism")]
    num_threads: Option<u16>,

    /// JPEG quality
    #[arg(long, default_value_t = 85)]
    jpeg_quality: u8,
}

fn compute_bbox(dataset: &Dataset) -> BBox {
    let geo_transform = dataset.geo_transform().unwrap();

    // Extract values from the GeoTransform
    let min_x = geo_transform[0]; // Top-left x
    let max_y = geo_transform[3]; // Top-left y
    let pixel_width = geo_transform[1];
    let pixel_height = geo_transform[5]; // Note: Typically negative for top-down

    // Get dataset size
    let raster_size = dataset.raster_size();

    // Calculate max_x and min_y
    let max_x = min_x + (raster_size.0 as f64) * pixel_width;
    let min_y = max_y + (raster_size.1 as f64) * pixel_height;

    BBox {
        min_x,
        min_y,
        max_x,
        max_y,
    }
}

fn prepare_target(target_file: &Path, max_zoom: u8) -> Result<Connection, Error> {
    let conn = Connection::open(target_file)?;

    conn.pragma_update(None, "synchronous", "OFF")?;

    conn.execute(
        "CREATE TABLE metadata (
            name TEXT NOT NULL,
            value TEXT NOT NULL,
            UNIQUE(name)
        )",
        (),
    )?;

    conn.execute(
        "CREATE TABLE tiles (
            zoom_level INTEGER NOT NULL,
            tile_column INTEGER NOT NULL,
            tile_row INTEGER NOT NULL,
            tile_data BLOB NOT NULL,
            PRIMARY KEY (zoom_level, tile_column, tile_row)
        )",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('name', 'Snina');",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('format', 'jpeg');",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('minzoom', 0);",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('maxzoom', ?1);",
        [max_zoom],
    )?;

    Ok(conn)
}

struct Status {
    pending_set: HashSet<Tile>,
    processed_set: HashSet<Tile>,
    waiting_set: HashSet<Tile>,
    pending_vec: Vec<Tile>,
}

fn main() {
    let args = Args::parse();

    let max_zoom = args.max_zoom;

    let source_file = args.source_file.as_path();

    let target_file = args.target_file.as_path();

    let source_srs = args.source_srs.as_deref();

    let target_srs = 3857;

    let pipeline = args.transform_pipeline.as_deref();

    let tile_size = args.tile_size;

    let num_threads = args.num_threads.unwrap_or_else(|| {
        available_parallelism()
            .expect("errro getting available parallelism")
            .get() as u16
    });

    let conn = Arc::new(Mutex::new(
        prepare_target(target_file, max_zoom).expect("error initializing mbtiles"),
    ));

    let pool = Arc::new(Mutex::new(Vec::<Dataset>::new()));

    let source_ds = Dataset::open(source_file).expect("Error opening source");

    let source_srs = if let Some(source_srs) = source_srs {
        SpatialRef::from_proj4(source_srs).expect("invalid proj4 SRS")
    } else {
        source_ds.spatial_ref().expect("error geting SRS")
    };

    let target_srs = SpatialRef::from_epsg(target_srs).expect("invalid epsg");

    let bbox = compute_bbox(&source_ds);

    let mut options = CoordTransformOptions::new().unwrap();

    if let Some(pipeline) = pipeline {
        options.set_coordinate_operation(pipeline, false).unwrap();
    }

    let trans = CoordTransform::new_with_options(&source_srs, &target_srs, &options)
        .expect("Failed to create coordinate transform")
        .transform_bounds(&[bbox.min_x, bbox.min_y, bbox.max_x, bbox.max_y], 21)
        .expect("error transforming bounds");

    let mut tiles = BBox {
        min_x: trans[0],
        max_x: trans[2],
        min_y: trans[1],
        max_y: trans[3],
    }
    .compute_covered_tiles(max_zoom);

    sort_by_zorder(&mut tiles);

    let workers: Vec<Worker<_>> = (0..num_threads).map(|_| Worker::new_lifo()).collect();

    let stealers: Arc<Vec<_>> = Arc::new(workers.iter().map(Worker::stealer).collect());

    let mut pending_set: HashSet<_> = tiles.iter().copied().collect();
    let mut todo_set: HashSet<_> = tiles.iter().copied().collect();
    let mut todo_dq: VecDeque<_> = tiles.iter().copied().collect();

    loop {
        let Some(tile) = todo_dq.pop_front() else {
            break;
        };

        todo_set.remove(&tile);

        if tile.zoom == 0 {
            continue;
        }

        if let Some(parent_tile) = tile.get_parent() {
            if todo_set.insert(parent_tile) {
                todo_dq.push_back(parent_tile);

                pending_set.insert(parent_tile);
            }
        }
    }

    for _ in 0..num_threads {
        let Some(tile) = tiles.pop() else {
            break;
        };

        workers[0].push(tile);
    }

    let total = pending_set.len();
    let counter = AtomicUsize::new(0);
    let lg_ts = AtomicUsize::new(0);

    let status = Arc::new(Mutex::new(Status {
        pending_set,
        processed_set: HashSet::new(),
        waiting_set: HashSet::new(),
        pending_vec: tiles,
    }));

    let buffer_cache = Arc::new(Mutex::new(HashMap::<Tile, Vec<u8>>::new()));

    let process_task = &move |tile: Tile, worker: &Worker<Tile>| {
        // println!("Processing {tile}");

        let counter = counter.fetch_add(1, Ordering::Relaxed);

        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let old = lg_ts.load(Ordering::Relaxed);

        if secs as usize != old
            && lg_ts
                .compare_exchange(old, secs as usize, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            println!("{} %", counter as f32 / total as f32 * 100.0);
        }

        let rgb_buffer = if tile.zoom < max_zoom {
            let mut rgb_buffer = vec![0u8; tile_size as usize * tile_size as usize * 3 * 4];

            let mut buffer_cache = buffer_cache.lock().unwrap();

            for (i, sector) in tile
                .get_children()
                .iter()
                .map(|tile| buffer_cache.remove(&tile))
                .enumerate()
            {
                let Some(sector) = sector else {
                    continue;
                };

                let soy = (i & 1) * tile_size as usize;
                let sox = ((i >> 1) & 1) * tile_size as usize;

                for x in 0..tile_size as usize {
                    for y in 0..tile_size as usize {
                        let offset1 = ((x + sox) * tile_size as usize * 2 + (y + soy)) * 3;

                        let offset2 = (x * tile_size as usize + y) * 3;

                        for band in 0..3 {
                            rgb_buffer[offset1 + band] = sector[offset2 + band];
                        }
                    }
                }
            }

            let img: RgbImage =
                ImageBuffer::from_vec(tile_size as u32 * 2, tile_size as u32 * 2, rgb_buffer)
                    .expect("Invalid image dimensions");

            image::imageops::resize(
                &img,
                tile_size as u32,
                tile_size as u32,
                FilterType::Lanczos3,
            )
            .into_raw()
        } else {
            let ds = pool.lock().unwrap().pop();

            let source_ds = ds.map_or_else(
                || Dataset::open(source_file).expect("Error opening source"),
                |ds| ds,
            );

            let bbox = tile.bounds_to_epsg3857(tile_size);

            let mut target_ds = DriverManager::get_driver_by_name("MEM")
                .expect("Failed to get MEM driver")
                .create("", tile_size as usize, tile_size as usize, 3)
                .expect("Failed to create target dataset");

            target_ds
                .set_geo_transform(&[
                    bbox.min_x,                                      // Top-left x
                    (bbox.max_x - bbox.min_x) / tile_size as f64,    // Pixel width
                    0.0,                                             // Rotation (x-axis)
                    bbox.max_y,                                      // Top-left y
                    0.0,                                             // Rotation (y-axis)
                    -((bbox.max_y - bbox.min_y) / tile_size as f64), // Pixel height (negative for top-down)
                ])
                .expect("error setting geo transform");

            unsafe {
                let warp_options = GDALCreateWarpOptions();

                if let Some(pipeline) = pipeline {
                    let option = CString::new(format!("COORDINATE_OPERATION={pipeline}")).unwrap();

                    let mut options: Vec<*mut i8> = vec![option.into_raw(), ptr::null_mut()];

                    let gen_img_proj_transformer = GDALCreateGenImgProjTransformer2(
                        source_ds.c_dataset(),
                        target_ds.c_dataset(),
                        options.as_mut_ptr(),
                    );

                    if gen_img_proj_transformer.is_null() {
                        panic!("Failed to create image projection transformer");
                    }

                    (*warp_options).pTransformerArg = gen_img_proj_transformer;

                    (*warp_options).pfnTransformer = Some(GDALGenImgProjTransform);
                }

                (*warp_options).eResampleAlg = GDALResampleAlg::GRA_Lanczos;

                (*warp_options).hSrcDS = source_ds.c_dataset();

                (*warp_options).hDstDS = target_ds.c_dataset();

                (*warp_options).nDstAlphaBand = 0;

                (*warp_options).nSrcAlphaBand = 0;

                GDALWarpInitDefaultBandMapping(warp_options, 3);

                let warp_operation = GDALCreateWarpOperation(warp_options);

                let result =
                    GDALChunkAndWarpImage(warp_operation, 0, 0, tile_size.into(), tile_size.into());

                if (*warp_options).pTransformerArg.is_null() {
                    GDALDestroyGenImgProjTransformer((*warp_options).pTransformerArg);
                }

                GDALDestroyWarpOperation(warp_operation);

                GDALDestroyWarpOptions(warp_options);

                assert!(
                    result == CPLErr::CE_None,
                    "ChunkAndWarpImage failed with error code: {:?}",
                    result
                );
            }

            pool.lock().unwrap().push(source_ds);

            let buffers: Vec<_> = (1..=3)
                .map(|band| {
                    target_ds
                        .rasterband(band)
                        .expect("error getting raster band")
                        .read_as::<u8>(
                            (0, 0),
                            (tile_size as usize, tile_size as usize),
                            (tile_size as usize, tile_size as usize),
                            None,
                        )
                        .expect("error reading from band")
                })
                .collect();

            let mut rgb_buffer = vec![0u8; tile_size as usize * tile_size as usize * 3];

            for x in 0..tile_size as usize {
                for y in 0..tile_size as usize {
                    let offset = (x * tile_size as usize + y) * 3;

                    for (i, buffer) in buffers.iter().enumerate() {
                        rgb_buffer[offset + i] = buffer[(x, y)];
                    }
                }
            }

            rgb_buffer
        };

        let mut vect = Vec::new();

        // produces bigger jpegs
        // JpegEncoder::new_with_quality(Cursor::new(&mut vect), 100)
        //     .write_image(
        //         &rgb_buffer,
        //         tile_size as u32,
        //         tile_size as u32,
        //         image::ExtendedColorType::Rgb8,
        //     )
        //     .expect("Failed to encode JPEG");

        Encoder::new(&mut vect, args.jpeg_quality)
            .encode(
                &rgb_buffer,
                tile_size as u16,
                tile_size as u16,
                ColorType::Rgb,
            )
            .expect("Failed to encode JPEG");

        conn.lock()
            .unwrap()
            .execute(
                "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                (tile.zoom, tile.x, (1 << tile.zoom) - 1 - tile.y, vect),
            )
            .expect("error inserting a tile");

        buffer_cache.lock().unwrap().insert(tile, rgb_buffer);

        let mut status = status.lock().unwrap();

        status.pending_set.remove(&tile);
        status.waiting_set.remove(&tile);
        status.processed_set.insert(tile);

        if let Some(parent) = tile.get_parent() {
            if !status.waiting_set.contains(&parent) && !status.processed_set.contains(&parent) {
                let children = parent.get_children();

                if children
                    .iter()
                    .all(|tile| !status.pending_set.contains(tile))
                {
                    status.pending_vec.push(parent);
                    status.waiting_set.insert(parent);
                }
            }
        }

        if let Some(tile) = status.pending_vec.pop() {
            worker.push(tile);
        }
    };

    thread::scope(|scope| {
        for worker in workers {
            let stealers = Arc::clone(&stealers);

            scope.spawn(move || {
                loop {
                    // First, try to pop a task from the local worker (LIFO)
                    if let Some(task) = worker.pop() {
                        process_task(task, &worker);
                    }
                    // If no tasks locally, try to steal from other threads
                    else if let Steal::Success(task) =
                        stealers.iter().map(|s| s.steal()).collect::<Steal<_>>()
                    {
                        process_task(task, &worker);
                    }
                    // If no tasks are left anywhere, exit the loop
                    else {
                        break;
                    }
                }
            });
        }
    });
}

fn sort_by_zorder(tiles: &mut Vec<Tile>) {
    tiles.sort_by_key(|tile| morton_code(tile.x, tile.y));
}

fn interleave(v: u32) -> u64 {
    let mut result = 0u64;

    for i in 0..32 {
        result |= ((v as u64 >> i) & 1) << (2 * i);
    }

    result
}

fn morton_code(x: u32, y: u32) -> u64 {
    interleave(x) | (interleave(y) << 1)
}
