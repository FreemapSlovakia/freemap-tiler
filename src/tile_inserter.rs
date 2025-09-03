use crate::{
    args::Format,
    schema::create_schema,
    tile::Tile,
    time_track::{Metric, StatsMsg},
};
use rusqlite::Connection;
use std::{
    path::Path,
    sync::mpsc::{Sender, SyncSender, sync_channel},
    thread::{self, JoinHandle},
    time::Instant,
};

pub fn new(
    target_file: &Path,
    max_zoom: Option<u8>,
    num_threads: u16,
    stats_tx: Sender<StatsMsg>,
    format: Format,
) -> rusqlite::Result<(JoinHandle<()>, SyncSender<(Tile, Vec<u8>, Vec<u8>)>)> {
    let (data_tx, data_rx) = sync_channel::<(Tile, Vec<u8>, Vec<u8>)>(num_threads as usize * 16);

    let conn = Connection::open(target_file)?;

    if let Some(max_zoom) = max_zoom {
        create_schema(&conn, max_zoom, format)?;
    }

    conn.pragma_update(None, "synchronous", "OFF")?;

    conn.pragma_update(None, "journal_mode", "WAL")?;

    let insert_thread = thread::spawn(move || {
        let mut stmt = conn
            .prepare(match format {
                Format::JPEG => concat!(
                    "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data, tile_alpha) ",
                    "VALUES (?1, ?2, ?3, ?4, ?5)"
                ),
                Format::PNG => concat!(
                    "INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data) ",
                    "VALUES (?1, ?2, ?3, ?4)"
                ),
            })
            .expect("Insert statement should be prepared");

        for msg in data_rx {
            let instant = Instant::now();

            match format {
                Format::JPEG => {
                    stmt.execute((msg.0.zoom, msg.0.x, msg.0.reversed_y(), msg.1, msg.2))
                }
                Format::PNG => stmt.execute((msg.0.zoom, msg.0.x, msg.0.reversed_y(), msg.1)),
            }
            .expect("Tile should be inserted");

            stats_tx
                .send(StatsMsg::Duration(
                    Metric::Insert,
                    Instant::now().duration_since(instant),
                ))
                .expect("Insert duration stats should be sent");
        }
    });

    Ok((insert_thread, data_tx))
}
