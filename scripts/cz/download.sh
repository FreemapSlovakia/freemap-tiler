#!/bin/bash
# Downloads the ČÚZK orthophoto sheets of one year into ./raw; safe to re-run.
# Usage: download.sh <year>   (JOBS=<n> sets parallel downloads, default 6)
set -euo pipefail

year=${1:?usage: download.sh <year>}

mkdir -p raw zip

if [ ! -s sheets.txt ]; then
  curl -sSf https://atom.cuzk.gov.cz/ORTOFOTO/ORTOFOTO.xml \
    | grep -oE "WRTO24\.$year\.[A-Z0-9]+" | sort -u > sheets.txt
fi

fetch() {
  s=$1
  [ -e "raw/$s.done" ] && return 0
  for i in 1 2 3 4 5; do
    # some ZIPs keep the files in a subfolder
    if curl -sSf --retry 3 -o "zip/$s.zip" "https://openzu.cuzk.gov.cz/opendata/ORTOFOTO/$s.zip" \
      && unzip -qjo "zip/$s.zip" "*$s.jpg" "*$s.jgw" -d raw; then
      rm "zip/$s.zip"
      touch "raw/$s.done"
      return 0
    fi
    sleep $((i * 10))
  done
  echo "FAILED $s" >&2
}
export -f fetch

xargs -P "${JOBS:-6}" -I{} bash -c 'fetch {}' < sheets.txt

echo "done: $(find raw -name '*.done' | wc -l) / $(wc -l < sheets.txt)" | tee download.done
