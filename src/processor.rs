use crate::{
    Limits,
    args::Format,
    state::State,
    time_track::{Metric, StatsMsg},
    warp::{self, Transform},
};
use crossbeam_deque::Worker;
use gdal::{Dataset, DriverManager, raster::ColorInterpretation};
use image::{
    GrayAlphaImage, ImageDecoder, ImageEncoder, RgbaImage,
    codecs::{jpeg::JpegDecoder, png::PngEncoder},
    imageops::FilterType,
};
use rusqlite::{Connection, OpenFlags};
use std::sync::Arc;
use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{Sender, SyncSender},
    },
    time::Instant,
};
use tilemath::Tile;

pub struct Processor {
    buffer_cache: Arc<Mutex<HashMap<Tile, Vec<u8>>>>,
    tile_size: u16,
    max_zoom: u8,
    pool: Arc<Mutex<Vec<Dataset>>>,
    counter: AtomicUsize,
    total: usize,
    select_conn: Option<Arc<Mutex<Connection>>>,
    stats_tx: Sender<StatsMsg>,
    debug: bool,
    source_file: PathBuf,
    state: Arc<Mutex<State>>,
    transform: Transform,
    jpeg_quality: u8,
    limits: Arc<Mutex<HashMap<u8, Limits>>>,
    data_tx: SyncSender<(Tile, Vec<u8>, Vec<u8>)>,
    zoom_offset: u8,
    insert_empty: bool,
    format: Format,
    band_count: usize,
}

impl Processor {
    pub fn new(
        tile_size: u16,
        max_zoom: u8,
        continue_file: Option<&Path>,
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
        insert_empty: bool,
        format: Format,
        no_data: Vec<Option<u8>>,
    ) -> Self {
        let total = pending_set.len();

        let state = State::new(pending_vec, pending_set, max_zoom, zoom_offset);

        // signal_hook::flag::register(signal_hook::consts::SIGHUP, Arc::clone(&no_resume)).unwrap();

        let pool = Arc::new(Mutex::new(Vec::<Dataset>::new()));

        let select_conn = continue_file.map(|continue_file| {
            Arc::new(Mutex::new(
                Connection::open_with_flags(continue_file, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .expect("error opening continue mbtiles connection"),
            ))
        });

        let band_count = ((no_data.len() + 1) / 2) * 2;

        Self {
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
            insert_empty,
            format,
            band_count,
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
                    self.buffer_cache
                        .lock()
                        .expect("error locking buffer_cache")
                        .len(),
                    tile,
                ))
                .expect("error sending stats");

            let mut steps = Vec::new();

            'out: {
                'resume: {
                    if let Some(ref select_conn) = self.select_conn {
                        let (rgb, alpha) = {
                            let select_instant = Instant::now();

                            let conn = select_conn.lock().expect("error locking select_conn");

                            let mut stmt = conn
                                .prepare("SELECT tile_data, tile_alpha FROM tiles WHERE zoom_level = ?1 AND tile_column = ?2 AND tile_row = ?3")
                                .expect("select statement should be prepared");

                            let mut rows = stmt
                                .query((tile.zoom, tile.x, tile.reversed_y()))
                                .expect("tile should be queried");

                            let Some(row) = rows.next().expect("error getting selected tile")
                            else {
                                break 'resume;
                            };

                            let rgb = row
                                .get::<_, Vec<u8>>(0)
                                .expect("error getting selected rgb");

                            let alpha = row
                                .get::<_, Vec<u8>>(1)
                                .expect("error getting selected alpha");

                            self.stats_tx
                                .send(StatsMsg::Duration(
                                    Metric::Select,
                                    Instant::now().duration_since(select_instant),
                                ))
                                .expect("error sending stats");

                            (rgb, alpha)
                        };

                        if tile.zoom < self.max_zoom {
                            let children = tile.children();

                            let mut buffer_cache = self
                                .buffer_cache
                                .lock()
                                .expect("error locking buffer_cache");

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

                        let decoder =
                            JpegDecoder::new(cursor).expect("error creading jpeg decoder");

                        let mut tile_data = vec![0; decoder.total_bytes() as usize];

                        decoder
                            .read_image(&mut tile_data)
                            .expect("error image-decoding");

                        let alpha = if alpha.is_empty() {
                            vec![255; 256 * 256]
                        } else {
                            zstd::stream::decode_all(alpha.as_slice()).expect("error zstd-decoding")
                        };

                        let rgba = tile_data
                            .chunks(3)
                            .zip(alpha.chunks(1))
                            .flat_map(|(a, b)| a.iter().chain(b))
                            .copied()
                            .collect::<Vec<u8>>();

                        self.buffer_cache
                            .lock()
                            .expect("error locking buffer_cache")
                            .insert(tile, rgba);

                        break 'out;
                    }
                } // 'resume

                let rgba = if tile.zoom < self.max_zoom {
                    steps.push('C');

                    let mut out_buffer =
                        vec![
                            0u8;
                            self.tile_size as usize * self.tile_size as usize * self.band_count * 4
                        ];

                    let mut has_data = false;

                    let children = tile.children();

                    let sectors: Vec<_> = {
                        let mut buffer_cache = self
                            .buffer_cache
                            .lock()
                            .expect("error locking buffer_cache");

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
                                    * self.band_count;

                                let offset2 = (x + y * self.tile_size as usize) * self.band_count;

                                out_buffer[offset1..(self.band_count + offset1)]
                                    .copy_from_slice(&sector[offset2..(self.band_count + offset2)]);
                            }
                        }
                    }

                    if has_data {
                        let img = if self.band_count == 2 {
                            let image = GrayAlphaImage::from_vec(
                                u32::from(self.tile_size) * 2,
                                u32::from(self.tile_size) * 2,
                                out_buffer,
                            )
                            .expect("rgba image should be created");

                            image::imageops::resize(
                                &image,
                                u32::from(self.tile_size),
                                u32::from(self.tile_size),
                                FilterType::Lanczos3,
                            )
                            .into_raw()
                        } else {
                            let image = RgbaImage::from_vec(
                                u32::from(self.tile_size) * 2,
                                u32::from(self.tile_size) * 2,
                                out_buffer,
                            )
                            .expect("rgba image should be created");

                            image::imageops::resize(
                                &image,
                                u32::from(self.tile_size),
                                u32::from(self.tile_size),
                                FilterType::Lanczos3,
                            )
                            .into_raw()
                        };

                        self.stats_tx
                            .send(StatsMsg::Duration(
                                Metric::Compose,
                                Instant::now().duration_since(compose_instant),
                            ))
                            .expect("error sending stats");

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
                        let ds = self.pool.lock().expect("error locking dataset pool").pop();

                        let source_ds = ds.map_or_else(
                            || Dataset::open(&self.source_file).expect("Error opening source"),
                            |ds| ds,
                        );

                        let warp_instant = Instant::now();

                        let bbox = tile
                            .ancestor(self.zoom_offset)
                            .expect("shold have tile ancestor")
                            .bounds(mega_size);

                        let mut target_ds = DriverManager::get_driver_by_name("MEM")
                            .expect("MEM driver should be obtained")
                            .create(
                                "",
                                (self.tile_size as usize) << self.zoom_offset,
                                (self.tile_size as usize) << self.zoom_offset,
                                self.band_count,
                            )
                            .expect("target dataset should be created");

                        let colors = if self.band_count == 2 {
                            vec![
                                ColorInterpretation::GrayIndex,
                                ColorInterpretation::AlphaBand,
                            ]
                        } else {
                            vec![
                                ColorInterpretation::RedBand,
                                ColorInterpretation::GreenBand,
                                ColorInterpretation::BlueBand,
                                ColorInterpretation::AlphaBand,
                            ]
                        };

                        for (i, color) in colors.into_iter().enumerate() {
                            target_ds
                                .rasterband(i + 1)
                                .unwrap()
                                .set_color_interpretation(color)
                                .unwrap();
                        }

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

                        let buffers: Vec<_> = target_ds
                            .rasterbands()
                            .map(|band| {
                                band.expect("raster band should be obtained")
                                    .read_as::<u8>(
                                        (0, 0),
                                        (mega_size as usize, mega_size as usize),
                                        (mega_size as usize, mega_size as usize),
                                        None,
                                    )
                                    .expect("band should be read")
                            })
                            .collect();

                        let no_data: Vec<_> = target_ds
                            .rasterbands()
                            .map(|band| band.unwrap().no_data_value().map(|nd| nd as u8))
                            .collect();

                        self.pool
                            .lock()
                            .expect("error locking dataset pool")
                            .push(source_ds);

                        let mut megatile1 = vec![
                            0u8;
                            ((mega_size as usize) * (mega_size as usize))
                                * self.band_count
                        ];

                        for x in 0..mega_size as usize {
                            for y in 0..mega_size as usize {
                                let offset = (x + y * mega_size as usize) * self.band_count;

                                for (i, buffer) in buffers.iter().enumerate() {
                                    let b = buffer[(y, x)];

                                    if no_data[i].map_or(false, |v| b == v) {
                                        for j in 0..buffers.len() {
                                            megatile1[offset + j] = 0;
                                        }

                                        break;
                                    }

                                    megatile1[offset + i] = b;
                                }
                            }
                        }

                        self.stats_tx
                            .send(StatsMsg::Duration(
                                Metric::Warp,
                                Instant::now().duration_since(warp_instant),
                            ))
                            .expect("error sending stats");

                        megatile = Some(megatile1);

                        megatile.as_ref().unwrap()
                    };

                    let (sx, sy) = tile.sector_in_ancestor(self.zoom_offset);

                    let mut out_buffer =
                        vec![
                            0u8;
                            self.tile_size as usize * self.tile_size as usize * self.band_count
                        ];

                    let mut is_empty = true;

                    for x in 0..self.tile_size as usize {
                        for y in 0..self.tile_size as usize {
                            let in_offset = (x
                                + (sx as usize) * (self.tile_size as usize)
                                + (y + (sy as usize) * (self.tile_size as usize))
                                    * (mega_size as usize))
                                * self.band_count;

                            let out_offset = (x + y * self.tile_size as usize) * self.band_count;

                            // TODO alternative - mask
                            if megatile[in_offset + self.band_count - 1] > 0 {
                                is_empty = false;

                                for i in 0..self.band_count {
                                    let b = megatile[in_offset + i];

                                    out_buffer[out_offset + i] = b;

                                    // if i == self.band_count - 1 {
                                    //     no_data &= b == 0; // TODO use proper nodata
                                    // }
                                }
                            }
                        }
                    }

                    if is_empty { None } else { Some(out_buffer) }
                }; // tile.zoom < max_zoom

                if let Some(rgba) = rgba {
                    steps.push('●');

                    let mut encoded = Vec::new();

                    let alpha_enc = match self.format {
                        Format::JPEG => {
                            let mut rgb =
                                Vec::with_capacity(rgba.len() - rgba.len() / self.band_count);

                            let mut alpha = Vec::with_capacity(rgba.len() / self.band_count);

                            let mut fully_opaque = true;

                            for chunk in rgba.chunks_exact(self.band_count) {
                                rgb.extend_from_slice(&chunk[0..self.band_count - 1]);

                                alpha.push(chunk[self.band_count - 1]);

                                fully_opaque = fully_opaque && chunk[self.band_count - 1] == 255;
                            }

                            let mut alpha_enc = Vec::new();

                            if !fully_opaque {
                                let mut encoder = zstd::Encoder::new(&mut alpha_enc, 0)
                                    .expect("zstd encoder should be created");

                                encoder
                                    .write_all(&alpha)
                                    .expect("data should be zstd encoded");

                                encoder.finish().expect("zstd encoding should be finished");
                            }

                            jpeg_encoder::Encoder::new(&mut encoded, self.jpeg_quality)
                                .encode(
                                    &rgb,
                                    self.tile_size,
                                    self.tile_size,
                                    if self.band_count == 2 {
                                        jpeg_encoder::ColorType::Luma
                                    } else {
                                        jpeg_encoder::ColorType::Rgb
                                    },
                                )
                                .expect("JPEG should be encoded");

                            alpha_enc
                        }
                        Format::PNG => {
                            PngEncoder::new_with_quality(
                                &mut encoded,
                                image::codecs::png::CompressionType::Best,
                                image::codecs::png::FilterType::Adaptive,
                            )
                            .write_image(
                                &rgba,
                                self.tile_size as u32,
                                self.tile_size as u32,
                                if self.band_count == 2 {
                                    image::ExtendedColorType::La8
                                } else {
                                    image::ExtendedColorType::Rgba8
                                },
                            )
                            .expect("PNG should be encoded");

                            vec![]
                        }
                    };

                    // println!("Inserting {tile}");

                    let y = tile.reversed_y();

                    self.limits
                        .lock()
                        .expect("limits should be locked")
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

                    self.data_tx
                        .send((tile, encoded, alpha_enc))
                        .expect("data shouuld be sent");

                    self.buffer_cache
                        .lock()
                        .expect("buffer_cache should be locked")
                        .insert(tile, rgba);
                } else if self.insert_empty {
                    steps.push('○');

                    // insert "nothing" - used for resuming
                    self.data_tx
                        .send((tile, vec![], vec![]))
                        .expect("data shouuld be sent");
                }
            }; // 'out

            let mut status = self.state.lock().expect("state should be locked");

            todo -= 1;

            status.processed(tile);

            if todo == 0 {
                if let Some(tiles) = status.next() {
                    worker.push(tiles);
                }
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
                .expect("error sending stats");
        }
    }
}
