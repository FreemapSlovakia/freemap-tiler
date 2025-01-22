# Freemap Tiler

Tool to create MBTiles from raster geodata with a full pyramid overview from zoom 0.
Uses Z-order curve to efficiently create lower-zoom tiles storing minimal tiles in RAM.

## Building and installing

```sh
cargo install --path .
```

## Command options

Use `-h` or `--help` to get description of all available options:

```
Usage: freemap-tiler [OPTIONS] --source-file <SOURCE_FILE> --target-file <TARGET_FILE> --max-zoom <MAX_ZOOM>

Options:
      --source-file <SOURCE_FILE>
          Input raster geofile
      --target-file <TARGET_FILE>
          Output *.mbtiles file
      --max-zoom <MAX_ZOOM>
          Max zoom level
      --source-srs <SOURCE_SRS>
          Source SRS
      --transform-pipeline <TRANSFORM_PIPELINE>
          Projection transformation pipeline
      --tile-size <TILE_SIZE>
          Tile size [default: 256]
      --num-threads <NUM_THREADS>
          Number of threads for parallel processing [default: available parallelism]
      --jpeg-quality <JPEG_QUALITY>
          JPEG quality [default: 85]
  -h, --help
          Print help
  -V, --version
          Print version
```

## Example

```sh
freemap-tiler \
  --source-file /home/martin/14TB/ofmozaika/Ortofoto_2022_vychod_jtsk_rgb/orto2022_vychod_rgb/all.vrt \
  --target-file /home/martin/OSM/vychod.mbtiles \
  --max-zoom 19 \
  --source-srs EPSG:8353 \
  --jpeg-quality 90 \
  --transform-pipeline "+proj=pipeline +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +inv +proj=hgridshift +grids=Slovakia_JTSK03_to_JTSK.gsb +step +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +proj=push +v_3 +step +proj=cart +ellps=bessel +step +proj=helmert +x=485.021 +y=169.465 +z=483.839 +rx=-7.786342 +ry=-4.397554 +rz=-4.102655 +s=0 +convention=coordinate_frame +step +inv +proj=cart +ellps=WGS84 +step +proj=pop +v_3 +step +proj=webmerc +lat_0=0 +lon_0=0 +x_0=0 +y_0=0 +ellps=WGS84"
```

## Creating composed mbtiles (imagery+alpha)

Stred:

```sh
sqlite3 stred-with-mask.mbtiles
```

```sql
ATTACH 'stred.mbtiles' AS db1;
ATTACH 'stred-mask.mbtiles' AS db2;
CREATE TABLE tiles (
    tile_data BLOB,
    zoom_level INTEGER,
    tile_column INTEGER,
    tile_row INTEGER,
    tile_alpha BLOB
);
INSERT INTO tiles (tile_data, zoom_level, tile_column, tile_row, tile_alpha)
SELECT db1.tiles.tile_data, db1.tiles.zoom_level, db1.tiles.tile_column, db1.tiles.tile_row, db2.tiles.tile_data AS tile_alpha
FROM db1.tiles
JOIN db2.tiles
ON (db1.tiles.zoom_level = db2.tiles.zoom_level AND db1.tiles.tile_column = db2.tiles.tile_column AND db1.tiles.tile_row = db2.tiles.tile_row);
CREATE TABLE metadata AS SELECT * FROM db1.metadata;
CREATE UNIQUE INDEX idx_tiles ON tiles (zoom_level, tile_column, tile_row);
CREATE UNIQUE INDEX idx_metadata ON metadata (name);
```

Vychod:

```sh
sqlite3 vychod-with-mask.mbtiles
```

```sql
ATTACH 'vychod.mbtiles' AS db1;
ATTACH 'vychod-mask.mbtiles' AS db2;
CREATE TABLE tiles (
    tile_data BLOB,
    zoom_level INTEGER,
    tile_column INTEGER,
    tile_row INTEGER,
    tile_alpha BLOB
);
INSERT INTO tiles (tile_data, zoom_level, tile_column, tile_row, tile_alpha)
SELECT db1.tiles.tile_data, db1.tiles.zoom_level, db1.tiles.tile_column, db1.tiles.tile_row, db2.tiles.tile_data AS tile_alpha
FROM db1.tiles
JOIN db2.tiles
ON (db1.tiles.zoom_level = db2.tiles.zoom_level AND db1.tiles.tile_column = db2.tiles.tile_column AND db1.tiles.tile_row = db2.tiles.tile_row);
CREATE TABLE metadata AS SELECT * FROM db1.metadata;
CREATE UNIQUE INDEX idx_tiles ON tiles (zoom_level, tile_column, tile_row);
CREATE UNIQUE INDEX idx_metadata ON metadata (name);
```
