# Freemap Tiler

Tool to create MBTiles from raster geodata with a full pyramid overview from to 0.
Uses Z-order curve to efficiently create lower-zoom tiles storing minimal tiles in RAM.

Source can be any raster GDAL source containing one of RGB, RGBA, Gray, Gray+Alpha.
The tool takes care of reprojection, slicing to tiles including all lowzoom (overview) tiles and storing it to MBTile format optional cusom extension to store alpha if JPEG is used.

## Extensions of MBTile format

Prodiced MBTile has following extensions:

- column `tile_alpha` in `tiles` table contains ZSTD compressed alpha channel (layer mask)
- `limits` metadata contains JSON encoded column/row bounds for every zoom level: `{ [zoom_level: string]: min_x: number, max_x: number, min_y: number, max_y: number }`

These extensions are supported by [`freemap-tileserver`](https://github.com/FreemapSlovakia/freemap-tileserver) which should be used for serving the tiles.

## Building and installing

If necessary, first clone, compile and install latest GDAL locally and then use it with `GDAL_HOME=/usr/local` environment variable.

```sh
GDAL_HOME=/usr/local cargo install --path .
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
      --continue-file <CONTINUE_FILE>
          Continue *.mbtiles file, use same as target-file to continue to the same file
      --max-zoom <MAX_ZOOM>
          Max zoom level
      --source-srs <SOURCE_SRS>
          Source SRS
      --transform-pipeline <TRANSFORM_PIPELINE>
          Projection transformation pipeline
      --bounding-polygon <BOUNDING_POLYGON>
          Bounding polygon in `GeoJSON` file
      --tile-size <TILE_SIZE>
          Tile size [default: 256]
      --num-threads <NUM_THREADS>
          Number of threads for parallel processing [default: available parallelism]
      --format <FORMAT>
          [default: jpeg] [possible values: jpeg, png]
      --jpeg-quality <JPEG_QUALITY>
          JPEG quality [default: 85]
      --warp-zoom-offset <WARP_ZOOM_OFFSET>
          Advanced: zoom offset of a parent tile to reproject at once. Modify to fine-tune the performance [default: 3]
      --debug
          Debug
      --insert-empty [<INSERT_EMPTY>]
          Insert empty [default: true] [possible values: true, false]
  -h, --help
          Print help
  -V, --version
          Print version
```

## Example

```sh
freemap-tiler \
  --source-file vychod-with-mask.vrt \
  --target-file vychod.mbtiles \
  --max-zoom 19 \
  --source-srs EPSG:8353 \
  --jpeg-quality 90 \
  --transform-pipeline "+proj=pipeline +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +inv +proj=hgridshift +grids=Slovakia_JTSK03_to_JTSK.gsb +step +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +proj=push +v_3 +step +proj=cart +ellps=bessel +step +proj=helmert +x=485.021 +y=169.465 +z=483.839 +rx=-7.786342 +ry=-4.397554 +rz=-4.102655 +s=0 +convention=coordinate_frame +step +inv +proj=cart +ellps=WGS84 +step +proj=pop +v_3 +step +proj=webmerc +lat_0=0 +lon_0=0 +x_0=0 +y_0=0 +ellps=WGS84"
```

## Cookbook

### Slovakia

1. download part of [Ortofotomozaika SR](https://www.geoportal.sk/sk/zbgis/ortofotomozaika/) you want to process from and extract it, for example _východ_ (`vychod`)
1. build VRT:
   ```sh
   gdalbuildvrt -a_srs EPSG:8353 vychod.vrt vychod-extracted/*.tif
   ```
1. create a tile index file `gdaltindex tmp.gpkg vychod-extracted/*.tif && ogr2ogr -f GPKG -t_srs EPSG:8353 index.gpkg tmp.gpkg && rm tmp.gpkg`
1. download `lms_datum_snimkovania_#.zip` where `#` is currently 2 or 3
1. dissolve `lms_datum_snimkovania`:
   ```sh
   ogr2ogr \
     -f GPKG \
     sk-area.gpkg \
     /vsizip/lms_datum_snimkovania_2.zip/lms_datum_snimkovania_2_cyklus.shp \
     -nln dissolved \
     -nlt POLYGON \
     -dialect sqlite \
     -sql "SELECT ST_Simplify(ST_MakePolygon(ST_ExteriorRing(ST_Buffer(ST_Union(geometry), 0.00001, 1))), 0.1) AS geometry FROM lms_datum_snimkovania_2_cyklus" \
     -a_srs EPSG:8353
   ```
1. dissolve the tile index
   ```sh
   ogr2ogr \
     -f GPKG \
     vychod-tiles.gpkg \
     index.gpkg \
     -nln tiles \
     -nlt POLYGON\
     -dialect sqlite \
     -sql "SELECT ST_Union(geom) AS geometry FROM 'index'" \
     -a_srs EPSG:8353
   ```
1. create a vector mask
   ```sh
   ogr2ogr -f GPKG combined.gpkg sk-area.gpkg -nln dissolved
   ogr2ogr -f GPKG -update -append combined.gpkg vychod-tiles.gpkg -nln tiles
   ogr2ogr -f GPKG vychod-mask.gpkg combined.gpkg \
     -dialect sqlite \
     -sql "
       SELECT ST_Intersection(a.geometry, b.geometry) AS geometry
       FROM tiles a, dissolved b
       WHERE ST_Intersects(a.geometry, b.geometry)
     " \
     -nln intersection \
     -nlt POLYGON \
     -a_srs EPSG:8353
   ```
1. rasterize the mask
   ```sh
   gdal_rasterize \
     -burn 0 \
     -at -i \
     -init 255 \
     -tap \
     $(gdalinfo -json vychod.vrt | jq -r '"-te \(.cornerCoordinates.upperLeft[0]) \(.cornerCoordinates.lowerRight[1]) \(.cornerCoordinates.lowerRight[0]) \(.cornerCoordinates.upperLeft[1]) -tr \(.geoTransform[1]) \(-.geoTransform[5])"') \
     -ot Byte \
     -of GTiff \
     -co TILED=YES \
     -co COMPRESS=ZSTD \
     -co BIGTIFF=YES \
     vychod-mask.gpkg vychod-mask.tif
   ```
1. generate VRT of the mask
   ```sh
   gdalbuildvrt vychod-mask.vrt vychod-mask.tif
   ```
1. edit `vychod.vrt` and add `VRTRasterBand` from `vychod-mask.vrt` and save to `vychod-with-mask.vrt`; example:
   ```xml
     ...
     <VRTRasterBand dataType="Byte" band="4">
       <ColorInterp>Alpha</ColorInterp>
       <SimpleSource>
         <SourceFilename relativeToVRT="1">vychod-mask.tif</SourceFilename>
         <SourceBand>1</SourceBand>
         <SourceProperties RasterXSize="775000" RasterYSize="898014" DataType="Byte" BlockXSize="256" BlockYSize="256" />
         <SrcRect xOff="0" yOff="0" xSize="775000" ySize="898014" />
         <DstRect xOff="0" yOff="0" xSize="775000" ySize="898014" />
       </SimpleSource>
     </VRTRasterBand>
   </VRTDataset>
   <!-- end of file -->
   ```
1. create MBTiles
   ```sh
   nice freemap-tiler --source-file vychod-with-mask.vrt --target-file vychod.mbtiles --max-zoom 19 --source-srs EPSG:8353 --jpeg-quality 85 --transform-pipeline "+proj=pipeline +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +inv +proj=hgridshift +grids=Slovakia_JTSK03_to_JTSK.gsb +step +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +proj=push +v_3 +step +proj=cart +ellps=bessel +step +proj=helmert +x=485.021 +y=169.465 +z=483.839 +rx=-7.786342 +ry=-4.397554 +rz=-4.102655 +s=0 +convention=coordinate_frame +step +inv +proj=cart +ellps=WGS84 +step +proj=pop +v_3 +step +proj=webmerc +lat_0=0 +lon_0=0 +x_0=0 +y_0=0 +ellps=WGS84"
   ```

### Czech republic

ČÚZK publishes the orthophoto as JPEG + JGW per SM5 map sheet (S-JTSK, 0.125 m) in its [ATOM feed](https://atom.cuzk.gov.cz/ORTOFOTO/ORTOFOTO.xml). Half of the country is flown each year: west (_západ_) in odd years, east (_východ_) in even ones. The feed holds only the latest year of each sheet.

[`scripts/cz/download.sh <year>`](scripts/cz/download.sh) does the download and [`scripts/cz/build.sh <name>`](scripts/cz/build.sh) everything from the VRT to the checksum; both run in the working directory and can be re-run. The mask and the bounding polygon are made by hand first, as `mask.gpkg` and `bounds.geojson`.

The steps, for _západ_ 2025:

1. download the sheets of the year; some ZIPs keep the files in a subfolder, hence `-j` and the wildcards
   ```sh
   curl -sSf https://atom.cuzk.gov.cz/ORTOFOTO/ORTOFOTO.xml | grep -oE 'WRTO24\.2025\.[A-Z0-9]+' | sort -u > sheets.txt
   mkdir -p raw
   xargs -P 6 -I{} sh -c 'curl -sSf --retry 3 -o {}.zip https://openzu.cuzk.gov.cz/opendata/ORTOFOTO/{}.zip && unzip -qjo {}.zip "*{}.jpg" "*{}.jgw" -d raw && rm {}.zip' < sheets.txt
   ```
1. build VRT
   ```sh
   ls raw/*.jpg > jpgs.txt
   gdalbuildvrt -a_srs EPSG:5514 -input_file_list jpgs.txt zapad.vrt
   ```
1. create a vector mask: the sheets' footprint intersected with the state boundary buffered by 99.5 m (`administrative_units` from ČÚZK's administrative boundaries, in `admin.gpkg`)
   ```sh
   gdaltindex -lyr_name index index.gpkg raw/*.jpg
   ogr2ogr -f GPKG combined.gpkg index.gpkg -nln dissolved -nlt POLYGON -a_srs EPSG:5514 \
     -dialect sqlite -sql "SELECT ST_Union(geom) AS geometry FROM 'index'"
   ogr2ogr -f GPKG -update -append combined.gpkg admin.gpkg -nln tiles -nlt POLYGON -a_srs EPSG:5514 \
     -dialect sqlite -sql "SELECT ST_Buffer(geom, 99.5, 16) AS geometry FROM administrative_units"
   ogr2ogr -f GPKG zapad-mask.gpkg combined.gpkg -nln mask -nlt POLYGON -a_srs EPSG:5514 \
     -dialect sqlite -sql "
       SELECT ST_Intersection(a.geometry, b.geometry) AS geometry
       FROM tiles a, dissolved b
       WHERE ST_Intersects(a.geometry, b.geometry)
     "
   ```
   The mask of the previous run of the same half can be reused when the sheet set is the same; then the seam with the other half stays where it was.
1. create the bounding polygon
   ```sh
   ogr2ogr -f GeoJSON -t_srs EPSG:4326 -simplify 10 bounds.geojson zapad-mask.gpkg
   ```
1. rasterize the mask
   ```sh
   gdal_rasterize \
     -burn 0 \
     -at -i \
     -init 255 \
     $(gdalinfo -json zapad.vrt | jq -r '"-te \(.cornerCoordinates.upperLeft[0]) \(.cornerCoordinates.lowerRight[1]) \(.cornerCoordinates.lowerRight[0]) \(.cornerCoordinates.upperLeft[1]) -tr \(.geoTransform[1]) \(-.geoTransform[5])"') \
     -ot Byte \
     -of GTiff \
     -co TILED=YES \
     -co COMPRESS=ZSTD \
     -co BIGTIFF=YES \
     zapad-mask.gpkg zapad-alpha-mask.tif
   ```
1. add the mask to `zapad.vrt` as the alpha band and save it as `zapad-with-mask.vrt`, the same way as for Slovakia
1. create MBTiles; the transformation is the 7-parameter Helmert of EPSG "S-JTSK to WGS 84 (1)" (no grid); list the candidates with `projinfo -s EPSG:5514 -t EPSG:3857 --spatial-test intersects -o proj`
   ```sh
   nice freemap-tiler \
     --source-file zapad-with-mask.vrt \
     --target-file cz-zapad-2025.mbtiles \
     --max-zoom 20 \
     --source-srs EPSG:5514 \
     --jpeg-quality 85 \
     --warp-zoom-offset 4 \
     --bounding-polygon bounds.geojson \
     --transform-pipeline '+proj=pipeline +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +proj=push +v_3 +step +proj=cart +ellps=bessel +step +proj=helmert +x=570.8 +y=85.7 +z=462.8 +rx=4.998 +ry=1.587 +rz=5.261 +s=3.56 +convention=position_vector +step +inv +proj=cart +ellps=WGS84 +step +proj=pop +v_3 +step +proj=webmerc +lat_0=0 +lon_0=0 +x_0=0 +y_0=0 +ellps=WGS84'
   ```
   Add `--continue-file cz-zapad-2025.mbtiles` to resume an interrupted run.
1. name the tileset and leave WAL mode, so readers need no `-shm`; quote the values with single quotes, as `"name"` is the column, not the string
   ```sh
   sqlite3 cz-zapad-2025.mbtiles "UPDATE metadata SET value = 'cz-ortofoto-zapad-2025' WHERE name = 'name'; PRAGMA journal_mode = DELETE"
   ```
