mod args;
mod geo;
mod geojson;
mod processor;
mod schema;
mod state;
mod tile_inserter;
mod time_track;
mod warp;

use ::geo::{Intersects, LineString, Polygon};
use args::Args;
use clap::Parser;
use crossbeam_deque::{Steal, Stealer, Worker};
use gdal::{
    Dataset,
    spatial_ref::{CoordTransform, CoordTransformOptions, SpatialRef},
};
use geo::compute_bbox;
use geojson::{parse_geojson_polygon, reproject_polygon};
use processor::Processor;
use rayon::iter::{ParallelBridge, ParallelIterator};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    process::ExitCode,
    sync::{Arc, Mutex},
    thread::{self, available_parallelism},
};
use tilemath::{BBox, Tile, bbox_covered_tiles};
use warp::Transform;

#[derive(Serialize, Deserialize, Debug)]
struct Limits {
    pub min_x: u32,
    pub max_x: u32,
    pub min_y: u32,
    pub max_y: u32,
}

fn main() -> ExitCode {
    if let Err(e) = try_main() {
        eprintln!("{e}");

        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn try_main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let target_file = args.target_file.as_path();

    if target_file.exists() && args.continue_file.is_none() {
        return Err("Target file exists".into());
    }

    let num_threads = args.num_threads.unwrap_or_else(|| {
        available_parallelism()
            .expect("errro getting available parallelism")
            .get() as u16
    });

    let mut bounding_polygon = args
        .bounding_polygon
        .map(|path| parse_geojson_polygon(&path))
        .transpose()
        .map_err(|e| format!("Error reading GeoJSON: {e}"))?;

    bounding_polygon
        .as_mut()
        .map(reproject_polygon)
        .transpose()
        .map_err(|e| format!("Error reprojecting polygon: {e}"))?;

    let source_ds = Dataset::open(&args.source_file).expect("Error opening source");

    let band_count = source_ds.raster_count();

    if band_count != 4 {
        return Err("Expecting 4 bands".into());
    }

    // // delete a tile and parents
    // {
    //     let conn =
    //         Connection::open(target_file).map_err(|e| format!("Error opening output: {e}"))?;

    //     let mut tile = Tile {
    //         zoom: 20,
    //         x: 569618,
    //         y: 360443,
    //     };

    //     loop {
    //         conn.execute(
    //             "DELETE FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3",
    //             (tile.zoom, tile.x, tile.reversed_y()),
    //         )
    //         .map_err(|e| format!("Error inserting limits: {e}"))?;

    //         let Some(parent) = tile.get_parent() else {
    //             break;
    //         };

    //         tile = parent;
    //     }
    // }

    let source_srs = args.source_srs.as_deref().map_or_else(
        || {
            source_ds
                .spatial_ref()
                .map_err(|e| format!("Error geting SRS: {e}"))
        },
        |source_srs| {
            SpatialRef::from_definition(source_srs)
                .map_err(|e| format!("Invalid spatial reference: {e}"))
        },
    )?;

    let target_srs = SpatialRef::from_epsg(3857)?;

    let bbox = compute_bbox(&source_ds);

    let mut options = CoordTransformOptions::new()?;

    let transform = if let Some(ref pipeline) = args.transform_pipeline {
        options.set_coordinate_operation(pipeline, false)?;

        Transform::Pipeline(pipeline.to_string())
    } else {
        Transform::Srs(source_srs.to_wkt()?, target_srs.to_wkt()?)
    };

    println!("Computing tile coverage");

    let trans = CoordTransform::new_with_options(&source_srs, &target_srs, &options)
        .map_err(|e| format!("Failed to create coordinate transform: {e}"))?
        .transform_bounds(&[bbox.min_x, bbox.min_y, bbox.max_x, bbox.max_y], 21)
        .map_err(|e| format!("Error transforming bounds: {e}"))?;

    println!("TRANS {:?}", trans);

    let bounding_polygon = bounding_polygon.as_ref();

    let mut tiles: Vec<_> = bbox_covered_tiles(
        &BBox {
            min_x: trans[0],
            max_x: trans[2],
            min_y: trans[1],
            max_y: trans[3],
        },
        args.max_zoom,
    )
    .par_bridge()
    .filter(|tile| {
        bounding_polygon.is_none_or(|bounding_polygon| {
            let bounds = tile.bounds(args.tile_size);

            Polygon::new(
                LineString::from(vec![
                    (bounds.min_x, bounds.min_y),
                    (bounds.max_x, bounds.min_y),
                    (bounds.max_x, bounds.max_y),
                    (bounds.min_x, bounds.max_y),
                    (bounds.min_x, bounds.min_y),
                ]),
                vec![],
            )
            .intersects(bounding_polygon)
        })
    })
    .collect();

    println!("Sorting tiles");

    Tile::sort_by_zorder(&mut tiles);

    println!("Preparing queues");

    let mut pending_set: HashSet<_> = tiles.iter().copied().collect();

    {
        let mut todo_set: HashSet<_> = tiles.iter().copied().collect();
        let mut todo_dq: VecDeque<_> = tiles.iter().copied().collect();

        while let Some(tile) = todo_dq.pop_front() {
            todo_set.remove(&tile);

            if tile.zoom == 0 {
                continue;
            }

            if let Some(parent_tile) = tile.parent()
                && todo_set.insert(parent_tile)
            {
                todo_dq.push_back(parent_tile);

                pending_set.insert(parent_tile);
            }
        }
    }

    let workers: Vec<_> = (0..num_threads).map(|_| Worker::new_lifo()).collect();

    // populate workers
    'outer: for _ in 0..num_threads {
        let mut task_tiles = Vec::new();

        let mut key: Option<Tile> = None;

        loop {
            let Some(tile) = tiles.pop() else {
                if !task_tiles.is_empty() {
                    workers[0].push(task_tiles);
                }

                break 'outer;
            };

            let curr_key = tile.ancestor(args.warp_zoom_offset);

            let Some(curr_key) = curr_key else {
                // no parent
                workers[0].push(vec![tile]);

                break;
            };

            if key.is_none() {
                key = Some(curr_key);
            }

            if Some(curr_key) == key {
                task_tiles.push(tile);
            } else {
                tiles.push(tile); // return it back

                workers[0].push(task_tiles);

                break;
            }
        }
    }

    let limits = Arc::new(Mutex::new(HashMap::<u8, Limits>::new()));

    let limits_clone = Arc::clone(&limits);

    let (stats_tx, stats_collector_thread) = time_track::new(args.debug);

    let (insert_thread, data_tx) = tile_inserter::new(
        target_file,
        if args.continue_file.is_none() || args.continue_file.as_deref() != Some(target_file) {
            Some(args.max_zoom)
        } else {
            None
        },
        num_threads,
        stats_tx.clone(),
        args.format,
    )?;

    {
        let processor = &Processor::new(
            args.tile_size,
            args.max_zoom,
            args.continue_file.as_deref(),
            stats_tx,
            args.debug,
            &args.source_file,
            transform,
            args.jpeg_quality,
            limits,
            data_tx,
            pending_set,
            tiles,
            args.warp_zoom_offset,
            args.insert_empty,
            args.format,
        );

        println!("Generating tiles");

        thread::scope(|scope| {
            let stealers: Arc<Vec<_>> = Arc::new(workers.iter().map(Worker::stealer).collect());

            for worker in workers {
                let stealers = Arc::clone(&stealers);

                scope.spawn(move || {
                    loop {
                        // First, try to pop a task from the local worker (LIFO)
                        if let Some(task) = worker.pop() {
                            processor.process_task(task, &worker);
                        }
                        // If no tasks locally, try to steal from other threads
                        else if let Steal::Success(task) =
                            stealers.iter().map(Stealer::steal).collect::<Steal<_>>()
                        {
                            processor.process_task(task, &worker);
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

    insert_thread.join().expect("error joining insert_thread");

    stats_collector_thread
        .join()
        .expect("error joining stats_collector_thread");

    let limits = {
        let limits = limits_clone.lock().unwrap();

        serde_json::to_string(&*limits).expect("Error serializing limits")
    };

    let conn =
        Connection::open(args.target_file).map_err(|e| format!("Error creating output: {e}"))?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('limits', ?1)",
        [limits],
    )
    .map_err(|e| format!("Error inserting limits: {e}"))?;

    Ok(())
}
