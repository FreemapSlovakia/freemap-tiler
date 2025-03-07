use crate::{
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
    num_threads: u16,
    stats_tx: Sender<StatsMsg>,
) -> rusqlite::Result<(JoinHandle<()>, SyncSender<(Tile, Vec<u8>, Vec<u8>)>)> {
    let (data_tx, data_rx) = sync_channel::<(Tile, Vec<u8>, Vec<u8>)>(num_threads as usize * 16);

    let insert_conn = Connection::open(target_file)?;

    insert_conn.pragma_update(None, "synchronous", "OFF")?;

    insert_conn.pragma_update(None, "journal_mode", "WAL")?;

    let insert_thread = thread::spawn(move || {
        let mut stmt = insert_conn
          .prepare("INSERT INTO tiles (zoom_level, tile_column, tile_row, tile_data, tile_alpha) VALUES (?1, ?2, ?3, ?4, ?5)")
          .expect("Error preparing insert statement");

        for msg in data_rx {
            let instant = Instant::now();

            stmt.execute((msg.0.zoom, msg.0.x, msg.0.reversed_y(), msg.1, msg.2))
                .expect("Error inserting");

            stats_tx
                .send(StatsMsg::Duration(
                    Metric::Insert,
                    Instant::now().duration_since(instant),
                ))
                .expect("Error sending insert duration stats");
        }
    });

    Ok((insert_thread, data_tx))
}
