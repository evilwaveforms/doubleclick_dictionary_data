#!/usr/bin/env bash
set -euo pipefail

if (( $# != 1 )); then
  echo "Usage: $0 <wiktionary-dump-date: YYYY-MM-DD>" >&2
  exit 2
fi

source_date=$1
if [[ ! $source_date =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]]; then
  echo "Source date must use YYYY-MM-DD format" >&2
  exit 2
fi

repository_directory=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
download_url=https://kaikki.org/dictionary/raw-wiktextract-data.jsonl.gz
input_path="$repository_directory/raw-wiktextract-data.jsonl.gz"
partial_path="$input_path.part"

curl \
  --fail \
  --location \
  --retry 3 \
  --retry-all-errors \
  --output "$partial_path" \
  "$download_url"
mv -- "$partial_path" "$input_path"

cd -- "$repository_directory"
cargo run --release -- \
  --input "$input_path" \
  --output "$repository_directory/dictionary-dist" \
  --language en \
  --source-date "$source_date"
