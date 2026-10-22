#!/bin/bash
# Builds <name>.mbtiles from ./raw, mask.gpkg and bounds.geojson (see the README);
# each step is skipped if its output exists, and an interrupted tiling continues.
# Usage: build.sh <name>   e.g. build.sh cz-zapad-2025
set -euo pipefail

name=${1:?usage: build.sh <name>}

if [ ! -e source.vrt ]; then
  find raw -name '*.jpg' | sort > jpgs.txt
  gdalbuildvrt -a_srs EPSG:5514 -input_file_list jpgs.txt source.vrt
fi

read -r xmin ymin xmax ymax res width height < <(gdalinfo -json source.vrt | jq -r '
  "\(.cornerCoordinates.upperLeft[0]) \(.cornerCoordinates.lowerRight[1]) \(.cornerCoordinates.lowerRight[0]) \(.cornerCoordinates.upperLeft[1]) \(.geoTransform[1]) \(.size[0]) \(.size[1])"')

if [ ! -e alpha-mask.tif ]; then
  nice gdal_rasterize -burn 0 -at -i -init 255 \
    -te "$xmin" "$ymin" "$xmax" "$ymax" -tr "$res" "$res" \
    -ot Byte -of GTiff -co TILED=YES -co COMPRESS=ZSTD -co BIGTIFF=YES \
    mask.gpkg alpha-mask.tif.tmp
  mv alpha-mask.tif.tmp alpha-mask.tif
fi

if [ ! -e with-mask.vrt ]; then
  {
    head -n -1 source.vrt
    cat <<XML
  <VRTRasterBand dataType="Byte" band="4">
    <ColorInterp>Alpha</ColorInterp>
    <SimpleSource>
      <SourceFilename relativeToVRT="1">alpha-mask.tif</SourceFilename>
      <SourceBand>1</SourceBand>
      <SrcRect xOff="0" yOff="0" xSize="$width" ySize="$height" />
      <DstRect xOff="0" yOff="0" xSize="$width" ySize="$height" />
    </SimpleSource>
  </VRTRasterBand>
</VRTDataset>
XML
  } > with-mask.vrt.tmp
  mv with-mask.vrt.tmp with-mask.vrt
fi

if [ ! -e "$name.mbtiles.done" ]; then
  cont=()
  [ -e "$name.mbtiles" ] && cont=(--continue-file "$name.mbtiles")

  nice freemap-tiler \
    --source-file with-mask.vrt \
    --target-file "$name.mbtiles" "${cont[@]}" \
    --max-zoom 20 \
    --source-srs EPSG:5514 \
    --jpeg-quality 85 \
    --warp-zoom-offset 4 \
    --bounding-polygon bounds.geojson \
    --transform-pipeline '+proj=pipeline +step +inv +proj=krovak +lat_0=49.5 +lon_0=24.8333333333333 +alpha=30.2881397527778 +k=0.9999 +x_0=0 +y_0=0 +ellps=bessel +step +proj=push +v_3 +step +proj=cart +ellps=bessel +step +proj=helmert +x=570.8 +y=85.7 +z=462.8 +rx=4.998 +ry=1.587 +rz=5.261 +s=3.56 +convention=position_vector +step +inv +proj=cart +ellps=WGS84 +step +proj=pop +v_3 +step +proj=webmerc +lat_0=0 +lon_0=0 +x_0=0 +y_0=0 +ellps=WGS84'

  touch "$name.mbtiles.done"
fi

# leaving WAL mode spares readers a -shm; values in single quotes, "name" is the column
sqlite3 "$name.mbtiles" "UPDATE metadata SET value = 'cz-ortofoto-${name#cz-}' WHERE name = 'name'; PRAGMA journal_mode = DELETE"

sha256sum "$name.mbtiles" > "$name.mbtiles.sha256sum"
