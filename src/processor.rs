use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{Sender, SyncSender},
        Mutex,
    },
    time::Instant,
};

use crossbeam_deque::Worker;
use gdal::{Dataset, DriverManager};
use image::{codecs::jpeg::JpegDecoder, imageops::FilterType, ImageDecoder, RgbaImage};
use rusqlite::{Connection, OpenFlags};

use crate::{
    state::State,
    tile::Tile,
    time_track::{Metric, StatsMsg},
    warp::{self, Transform},
    Limits,
};
use std::sync::Arc;

pub struct Processor {
    no_resume: Arc<AtomicBool>,
    buffer_cache: Arc<Mutex<HashMap<Tile, Vec<u8>>>>,
    tile_size: u16,
    max_zoom: u8,
    pool: Arc<Mutex<Vec<Dataset>>>,
    counter: AtomicUsize,
    total: usize,
    select_conn: Arc<Mutex<Connection>>,
    stats_tx: Sender<StatsMsg>,
    debug: bool,
    source_file: PathBuf,
    state: Arc<Mutex<State>>,
    transform: Transform,
    jpeg_quality: u8,
    limits: Arc<Mutex<HashMap<u8, Limits>>>,
    data_tx: SyncSender<(Tile, Vec<u8>, Vec<u8>)>,
    zoom_offset: u8,
}

const BAND_COUNT: usize = 4;

impl Processor {
    pub fn new(
        resume: bool,
        tile_size: u16,
        max_zoom: u8,
        target_file: &Path,
        stats_tx: Sender<StatsMsg>,
        debug: bool,
        source_file: &Path,
        transform: Transform,
        jpeg_quality: u8,
        limits: Arc<Mutex<HashMap<u8, Limits>>>,
        data_tx: SyncSender<(Tile, Vec<u8>, Vec<u8>)>,
        pending_set: HashSet<Tile>,
        pending_vec: Vec<Tile>,
        zoom_offset: u8,
    ) -> Self {
        let total = pending_set.len();

        let state = State::new(pending_vec, pending_set, max_zoom, zoom_offset);

        let no_resume = Arc::new(AtomicBool::new(!resume));

        signal_hook::flag::register(signal_hook::consts::SIGHUP, Arc::clone(&no_resume)).unwrap();

        let pool = Arc::new(Mutex::new(Vec::<Dataset>::new()));

        let select_conn = Arc::new(Mutex::new(
            Connection::open_with_flags(target_file, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| format!("Error creating output: {e}"))
                .unwrap(),
        ));

        Self {
            no_resume,
            buffer_cache: Arc::new(Mutex::new(HashMap::new())),
            tile_size,
            max_zoom,
            pool,
            counter: AtomicUsize::new(0),
            total,
            select_conn,
            stats_tx,
            debug,
            source_file: source_file.to_path_buf(),
            state: Arc::new(Mutex::new(state)),
            transform,
            jpeg_quality,
            limits,
            data_tx,
            zoom_offset,
        }
    }

    pub fn process_task(&self, task: Vec<Tile>, worker: &Worker<Vec<Tile>>) {
        let mut megatile: Option<Vec<u8>> = None;

        let mut todo = task.len();

        for tile in task {
            let counter = self.counter.fetch_add(1, Ordering::Relaxed);

            let top_instant = Instant::now();

            self.stats_tx
                .send(StatsMsg::Stats(
                    counter as f32 / self.total as f32 * 100.0,
                    self.buffer_cache.lock().unwrap().len(),
                    tile,
                ))
                .unwrap();

            let mut steps = Vec::new();

            'out: {
                'resume: {
                    if self.no_resume.load(Ordering::Relaxed) {
                        break 'resume;
                    }

                    let (rgb, alpha) = {
                        let select_instant = Instant::now();

                        let conn = self.select_conn.lock().unwrap();

                        let mut stmt = conn.prepare(
                            "SELECT tile_data, tile_alpha FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3").unwrap();

                        let mut rows = stmt.query((tile.zoom, tile.x, tile.reversed_y())).unwrap();

                        let Some(row) = rows.next().unwrap() else {
                            break 'resume;
                        };

                        let rgb = row.get::<_, Vec<u8>>(0).unwrap();

                        let alpha = row.get::<_, Vec<u8>>(1).unwrap();

                        self.stats_tx
                            .send(StatsMsg::Duration(
                                Metric::Select,
                                Instant::now().duration_since(select_instant),
                            ))
                            .unwrap();

                        (rgb, alpha)
                    };

                    if tile.zoom < self.max_zoom {
                        let children = tile.get_children();

                        let mut buffer_cache = self.buffer_cache.lock().unwrap();

                        for tile in children {
                            buffer_cache.remove(&tile);
                        }
                    }

                    if rgb.is_empty() {
                        steps.push('○');

                        break 'out;
                    }

                    steps.push('●');

                    let cursor = Cursor::new(&rgb);

                    let decoder = JpegDecoder::new(cursor).unwrap();

                    let mut tile_data = vec![0; decoder.total_bytes() as usize];

                    decoder.read_image(&mut tile_data).unwrap();

                    let alpha = if alpha.is_empty() {
                        vec![255; 256 * 256]
                    } else {
                        zstd::stream::decode_all(alpha.as_slice()).unwrap()
                    };

                    let rgba = tile_data
                        .chunks(3)
                        .zip(alpha.chunks(1))
                        .flat_map(|(a, b)| a.iter().chain(b))
                        .copied()
                        .collect::<Vec<u8>>();

                    self.buffer_cache.lock().unwrap().insert(tile, rgba);

                    break 'out;
                } // 'resume

                let rgba = if tile.zoom < self.max_zoom {
                    steps.push('C');

                    let mut out_buffer =
                        vec![
                            0u8;
                            self.tile_size as usize * self.tile_size as usize * BAND_COUNT * 4
                        ];

                    let mut has_data = false;

                    let children = tile.get_children();

                    let sectors: Vec<_> = {
                        let mut buffer_cache = self.buffer_cache.lock().unwrap();

                        children
                            .iter()
                            .map(|tile| buffer_cache.remove(tile))
                            .collect()
                    };

                    let compose_instant = Instant::now();

                    for (i, sector) in sectors.into_iter().enumerate() {
                        let Some(sector) = sector else {
                            continue;
                        };

                        has_data = true;

                        let so_x = (i & 1) * self.tile_size as usize;
                        let so_y = (i >> 1) * self.tile_size as usize;

                        for x in 0..self.tile_size as usize {
                            for y in 0..self.tile_size as usize {
                                let offset1 = ((x + so_x)
                                    + (y + so_y) * self.tile_size as usize * 2)
                                    * BAND_COUNT;

                                let offset2 = (x + y * self.tile_size as usize) * BAND_COUNT;

                                out_buffer[offset1..(BAND_COUNT + offset1)]
                                    .copy_from_slice(&sector[offset2..(BAND_COUNT + offset2)]);
                            }
                        }
                    }

                    if has_data {
                        let image = RgbaImage::from_vec(
                            u32::from(self.tile_size) * 2,
                            u32::from(self.tile_size) * 2,
                            out_buffer,
                        )
                        .expect("Invalid image dimensions");

                        let img = image::imageops::resize(
                            &image,
                            u32::from(self.tile_size),
                            u32::from(self.tile_size),
                            FilterType::Lanczos3,
                        )
                        .into_raw();

                        self.stats_tx
                            .send(StatsMsg::Duration(
                                Metric::Compose,
                                Instant::now().duration_since(compose_instant),
                            ))
                            .unwrap();

                        Some(img)
                    } else {
                        None
                    }
                } else
                // tile.zoom == max_zoom
                {
                    let mega_size = self.tile_size << self.zoom_offset;

                    let megatile = if let Some(ref megatile) = megatile {
                        megatile
                    } else {
                        let ds = self.pool.lock().unwrap().pop();

                        let source_ds = ds.map_or_else(
                            || Dataset::open(&self.source_file).expect("Error opening source"),
                            |ds| ds,
                        );

                        let warp_instant = Instant::now();

                        let bbox = tile
                            .get_ancestor(self.zoom_offset)
                            .unwrap() // TODO
                            .bounds_to_epsg3857(mega_size);

                        let mut target_ds = DriverManager::get_driver_by_name("MEM")
                            .expect("Failed to get MEM driver")
                            .create(
                                "",
                                (self.tile_size as usize) << self.zoom_offset,
                                (self.tile_size as usize) << self.zoom_offset,
                                BAND_COUNT,
                            )
                            .expect("Failed to create target dataset");

                        target_ds
                            .set_geo_transform(&[
                                bbox.min_x,                                          // Top-left x
                                (bbox.max_x - bbox.min_x) / f64::from(mega_size),    // Pixel width
                                0.0,        // Rotation (x-axis)
                                bbox.max_y, // Top-left y
                                0.0,        // Rotation (y-axis)
                                -((bbox.max_y - bbox.min_y) / f64::from(mega_size)), // Pixel height (negative for top-down)
                            ])
                            .expect("error setting geo transform");

                        steps.push('W');

                        warp::warp(&source_ds, &target_ds, mega_size, &self.transform);

                        self.pool.lock().unwrap().push(source_ds);

                        let buffers: Vec<_> = (1..=BAND_COUNT)
                            .map(|band| {
                                target_ds
                                    .rasterband(band)
                                    .expect("error getting raster band")
                                    .read_as::<u8>(
                                        (0, 0),
                                        (mega_size as usize, mega_size as usize),
                                        (mega_size as usize, mega_size as usize),
                                        None,
                                    )
                                    .expect("error reading from band")
                            })
                            .collect();

                        let mut megatile1 =
                            vec![0u8; ((mega_size as usize) * (mega_size as usize)) * BAND_COUNT];

                        for x in 0..mega_size as usize {
                            for y in 0..mega_size as usize {
                                let offset = (x + y * mega_size as usize) * BAND_COUNT;

                                for (i, buffer) in buffers.iter().enumerate() {
                                    let b = buffer[(y, x)];

                                    megatile1[offset + i] = b;
                                }
                            }
                        }

                        self.stats_tx
                            .send(StatsMsg::Duration(
                                Metric::Warp,
                                Instant::now().duration_since(warp_instant),
                            ))
                            .unwrap();

                        megatile = Some(megatile1);

                        &megatile.as_ref().unwrap()
                    };

                    let (sx, sy) = tile.get_sector_in_parent(self.zoom_offset);

                    let mut out_buffer =
                        vec![0u8; self.tile_size as usize * self.tile_size as usize * BAND_COUNT];

                    let mut no_data = true;

                    for x in 0..self.tile_size as usize {
                        for y in 0..self.tile_size as usize {
                            let in_offset = (x
                                + (sx as usize) * (self.tile_size as usize)
                                + (y + (sy as usize) * (self.tile_size as usize))
                                    * (mega_size as usize))
                                * BAND_COUNT;

                            let out_offset = (x + y * self.tile_size as usize) * BAND_COUNT;

                            for i in 0..BAND_COUNT {
                                let b = megatile[in_offset + i];

                                out_buffer[out_offset + i] = b;

                                if i == BAND_COUNT - 1 {
                                    no_data &= b == 0;
                                }
                            }
                        }
                    }

                    if no_data {
                        None
                    } else {
                        Some(out_buffer)
                    }
                }; // tile.zoom < max_zoom

                if let Some(rgba) = rgba {
                    steps.push('●');

                    // produces bigger jpegs
                    // JpegEncoder::new_with_quality(Cursor::new(&mut vect), 100)
                    //     .write_image(
                    //         &out_buffer,
                    //         tile_size as u32,
                    //         tile_size as u32,
                    //         image::ExtendedColorType::Rgb8,
                    //     )
                    //     .expect("Failed to encode JPEG");

                    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);

                    let mut alpha = Vec::with_capacity(rgba.len() / 4);

                    let mut is_full = true;

                    for chunk in rgba.chunks_exact(4) {
                        rgb.extend_from_slice(&chunk[0..3]);

                        alpha.push(chunk[3]);

                        is_full = is_full && chunk[3] == 255;
                    }

                    let mut alpha_enc = Vec::new();

                    if !is_full {
                        let mut encoder = zstd::Encoder::new(&mut alpha_enc, 0)
                            .expect("error creating zstd encoder");

                        encoder.write_all(&alpha).expect("error zstd encoding");

                        encoder.finish().expect("error finishing zstd encoding");
                    }

                    let mut rgb_enc = Vec::new();

                    jpeg_encoder::Encoder::new(&mut rgb_enc, self.jpeg_quality)
                        .encode(
                            &rgb,
                            self.tile_size,
                            self.tile_size,
                            jpeg_encoder::ColorType::Rgb,
                        )
                        .expect("Failed to encode JPEG");

                    // println!("Inserting {tile}");

                    let y = tile.reversed_y();

                    self.limits
                        .lock()
                        .unwrap()
                        .entry(tile.zoom)
                        .and_modify(|limits: &mut Limits| {
                            limits.max_x = limits.max_x.max(tile.x);
                            limits.min_x = limits.min_x.min(tile.x);
                            limits.max_y = limits.max_y.max(y);
                            limits.min_y = limits.min_y.min(y);
                        })
                        .or_insert_with(move || Limits {
                            min_x: tile.x,
                            max_x: tile.x,
                            min_y: y,
                            max_y: y,
                        });

                    self.data_tx.send((tile, rgb_enc, alpha_enc)).unwrap();

                    self.buffer_cache.lock().unwrap().insert(tile, rgba);
                } else {
                    steps.push('○');

                    // insert "nothing" - used for resuming
                    self.data_tx.send((tile, vec![], vec![])).unwrap();
                }
            }; // 'out

            let mut status = self.state.lock().unwrap();

            todo -= 1;

            if let Some(tiles) = status.next(tile, todo == 0) {
                worker.push(tiles);
            }

            drop(status);

            if self.debug {
                print!("|{}", steps.iter().collect::<String>());
            }

            self.stats_tx
                .send(StatsMsg::Duration(
                    Metric::Encode,
                    Instant::now().duration_since(top_instant),
                ))
                .unwrap();
        }
    }
}
