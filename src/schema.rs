use rusqlite::{Connection, Error};

pub fn create_schema(conn: &Connection, max_zoom: u8) -> Result<(), Error> {
    conn.pragma_update(None, "synchronous", "OFF")?;

    conn.pragma_update(None, "journal_mode", "WAL")?;

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
          tile_alpha BLOB NOT NULL
        )",
        (),
    )?;

    // conn.execute(
    //     "CREATE UNIQUE INDEX idx_tiles ON tiles (zoom_level, tile_column, tile_row)",
    //     (),
    // )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('name', 'Tiles')",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('format', 'jpeg')",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('minzoom', 0)",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('maxzoom', ?1)",
        [max_zoom],
    )?;

    Ok(())
}
