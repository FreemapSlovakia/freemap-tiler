mod args;
mod bbox;
mod geo;
mod tile;
mod warp;

use args::Args;
use bbox::BBox;
use clap::Parser;
use crossbeam_deque::{Steal, Stealer, Worker};
use gdal::{
    spatial_ref::{CoordTransform, CoordTransformOptions, SpatialRef},
    Dataset, DriverManager,
};
use image::{imageops::FilterType, DynamicImage, GrayImage, RgbImage};
use rusqlite::{Connection, Error};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::remove_file,
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, available_parallelism},
    time::{SystemTime, UNIX_EPOCH},
};
use tile::Tile;

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

    let band_count = source_ds.raster_count();

    let source_srs = source_srs.map_or_else(
        || source_ds.spatial_ref().expect("error geting SRS"),
        |source_srs| SpatialRef::from_definition(source_srs).expect("invalid spatial reference"),
    );

    let target_srs = SpatialRef::from_epsg(3857).expect("invalid epsg");

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

    Tile::sort_by_zorder(&mut tiles);

    let workers: Vec<Worker<_>> = (0..num_threads).map(|_| Worker::new_lifo()).collect();

    let stealers: Arc<Vec<_>> = Arc::new(workers.iter().map(Worker::stealer).collect());

    let mut pending_set: HashSet<_> = tiles.iter().copied().collect();
    let mut todo_set: HashSet<_> = tiles.iter().copied().collect();
    let mut todo_dq: VecDeque<_> = tiles.iter().copied().collect();

    while let Some(tile) = todo_dq.pop_front() {
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
            println!(
                "{:.2} % | {}",
                counter as f32 / total as f32 * 100.0,
                buffer_cache.lock().unwrap().len()
            );
        }

        let res = if tile.zoom < max_zoom {
            let mut out_buffer =
                vec![0u8; tile_size as usize * tile_size as usize * band_count * 4];

            let mut has_data = false;

            let mut is_white = true;

            let children = tile.get_children();

            let mut buffer_cache = buffer_cache.lock().unwrap();

            let sectors: Vec<_> = children
                .iter()
                .map(|tile| buffer_cache.remove(tile))
                .collect();

            drop(buffer_cache); // just for sure

            for (i, sector) in sectors.into_iter().enumerate() {
                let Some(sector) = sector else {
                    continue;
                };

                has_data = true;

                let so_y = (i & 1) * tile_size as usize;
                let so_x = ((i >> 1) & 1) * tile_size as usize;

                for x in 0..tile_size as usize {
                    for y in 0..tile_size as usize {
                        let offset1 =
                            ((x + so_x) * tile_size as usize * 2 + (y + so_y)) * band_count;

                        let offset2 = (x * tile_size as usize + y) * band_count;

                        out_buffer[offset1..(band_count + offset1)]
                            .copy_from_slice(&sector[offset2..(band_count + offset2)]);

                        is_white = is_white && (out_buffer[offset1] == 255);
                    }
                }
            }

            if has_data {
                let img: DynamicImage = if band_count == 3 {
                    RgbImage::from_vec(
                        u32::from(tile_size) * 2,
                        u32::from(tile_size) * 2,
                        out_buffer,
                    )
                    .expect("Invalid image dimensions")
                    .into()
                } else {
                    GrayImage::from_vec(
                        u32::from(tile_size) * 2,
                        u32::from(tile_size) * 2,
                        out_buffer,
                    )
                    .expect("Invalid image dimensions")
                    .into()
                };

                Some((
                    image::imageops::resize(
                        &img,
                        u32::from(tile_size),
                        u32::from(tile_size),
                        FilterType::Lanczos3,
                    )
                    .into_raw(),
                    is_white,
                ))
            } else {
                None
            }
        } else {
            let ds = pool.lock().unwrap().pop();

            let source_ds = ds.map_or_else(
                || Dataset::open(source_file).expect("Error opening source"),
                |ds| ds,
            );

            let bbox = tile.bounds_to_epsg3857(tile_size);

            let mut target_ds = DriverManager::get_driver_by_name("MEM")
                .expect("Failed to get MEM driver")
                .create("", tile_size as usize, tile_size as usize, band_count)
                .expect("Failed to create target dataset");

            target_ds
                .set_geo_transform(&[
                    bbox.min_x,                                          // Top-left x
                    (bbox.max_x - bbox.min_x) / f64::from(tile_size),    // Pixel width
                    0.0,                                                 // Rotation (x-axis)
                    bbox.max_y,                                          // Top-left y
                    0.0,                                                 // Rotation (y-axis)
                    -((bbox.max_y - bbox.min_y) / f64::from(tile_size)), // Pixel height (negative for top-down)
                ])
                .expect("error setting geo transform");

            warp::warp(&source_ds, &target_ds, tile_size, pipeline);

            pool.lock().unwrap().push(source_ds);

            let buffers: Vec<_> = (1..=band_count)
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

            let mut out_buffer = vec![0u8; tile_size as usize * tile_size as usize * band_count];

            let mut has_data = false;

            let mut is_white = true;

            for x in 0..tile_size as usize {
                for y in 0..tile_size as usize {
                    let offset = (x * tile_size as usize + y) * band_count;

                    for (i, buffer) in buffers.iter().enumerate() {
                        let b = buffer[(x, y)];
                        out_buffer[offset + i] = b;
                        has_data = has_data || (b != 0);
                        is_white = is_white && (b == 255);
                    }
                }
            }

            if has_data {
                Some((out_buffer, is_white))
            } else {
                None
            }
        };

        if let Some((out_buffer, is_white)) = res {
            let mut vect = Vec::new();

            // produces bigger jpegs
            // JpegEncoder::new_with_quality(Cursor::new(&mut vect), 100)
            //     .write_image(
            //         &out_buffer,
            //         tile_size as u32,
            //         tile_size as u32,
            //         image::ExtendedColorType::Rgb8,
            //     )
            //     .expect("Failed to encode JPEG");

            if !is_white {
                // nothing
            } else if band_count == 3 {
                jpeg_encoder::Encoder::new(&mut vect, args.jpeg_quality)
                    .encode(
                        &out_buffer,
                        tile_size,
                        tile_size,
                        jpeg_encoder::ColorType::Rgb,
                    )
                    .expect("Failed to encode JPEG");
            } else {
                let mut encoder = zstd::stream::write::Encoder::new(&mut vect, 0)
                    .expect("error creating zstd encoder");

                encoder.write(&out_buffer).expect("error zstd encoding");

                encoder.finish().expect("error finishing zstd encoding");
            }

            println!("Inserting {tile}");

            if let Err(error) = conn.lock().unwrap().execute(
                "INSERT INTO tiles VALUES (?1, ?2, ?3, ?4)",
                (tile.zoom, tile.x, (1 << tile.zoom) - 1 - tile.y, vect),
            ) {
                panic!("Err: {tile} {error}");
            }

            buffer_cache.lock().unwrap().insert(tile, out_buffer);
        }

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
                        stealers.iter().map(Stealer::steal).collect::<Steal<_>>()
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

fn compute_bbox(dataset: &Dataset) -> BBox {
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

fn prepare_target(target_file: &Path, max_zoom: u8) -> Result<Connection, Error> {
    if target_file.exists() {
        remove_file(target_file).expect("error deleting file");
    }

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
