use rusqlite::{Connection, Error};

use crate::args::Format;

pub fn create_schema(conn: &Connection, max_zoom: u8, format: Format) -> Result<(), Error> {
    conn.execute(
        "CREATE TABLE metadata (
          name TEXT NOT NULL,
          value TEXT NOT NULL,
          UNIQUE(name)
      )",
        (),
    )?;

    conn.execute(
        &format!(
            "CREATE TABLE tiles (
          zoom_level INTEGER NOT NULL,
          tile_column INTEGER NOT NULL,
          tile_row INTEGER NOT NULL,
          tile_data BLOB NOT NULL
          {}
        )",
            match format {
                Format::JPEG => ", tile_alpha BLOB NOT NULL",
                Format::PNG => "",
            }
        ),
        (),
    )?;

    conn.execute(
        "CREATE UNIQUE INDEX idx_tiles ON tiles (zoom_level, tile_column, tile_row)",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('name', 'Tiles')",
        (),
    )?;

    conn.execute(
        "INSERT INTO metadata (name, value) VALUES ('format', ?1)",
        [match format {
            Format::JPEG => "jpeg",
            Format::PNG => "png",
        }],
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
