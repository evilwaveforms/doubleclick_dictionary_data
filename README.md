# Double-click Dictionary Data

Builds compact, deterministic dictionary shards from Wiktextract JSONL for the [Double-click Dictionary Firefox extension](https://github.com/evilwaveforms/firefox_doubleclick_dictionary). Output is designed for Cloudflare Workers Static Assets.

## Build

Download the current raw Wiktextract dataset and build English, Finnish, Swedish, German, French, and Spanish entries. Pass the enwiktionary dump date shown on the [Kaikki raw data page](https://kaikki.org/dictionary/rawdata.html), not the download date:

```sh
./scripts/update-dictionary.sh 2026-08-05
```

The script downloads to a temporary file before replacing the previous input. It then builds the release binary and generates `dictionary-dist`.

To process an existing download manually:

```sh
cargo build --release
target/release/doubleclick-dictionary-data \
  --input raw-wiktextract-data.jsonl.gz \
  --output dictionary-dist \
  --language en \
  --language fi \
  --language sv \
  --language de \
  --language fr \
  --language es \
  --source-date 2026-08-05
```

The input is streamed and partitioned before final shards are assembled. Peak memory is bounded by one partition rather than the complete dictionary. `--language` can be repeated and `--shards` defaults to 8192.

The output directory contains:

- `v2/shards/*.json`: hashed lookup shards using schema version 2.
- `v2/metadata.json`: source, language, shard and entry metadata.
- `v2/licenses/wiktionary.txt`: dictionary attribution, license, and modification notice.
- `_headers`: cache and CORS headers for Workers Static Assets.

Each incompatible schema gets a stable versioned URL. The generator retains the current schema and the newest older schema until another schema replaces it. It refuses to manage unmarked schema directories or downgrade over a newer schema.

The generator replaces a schema directory only when it contains its output marker. Raw downloads and generated output are ignored by Git.

## Deploy

Test the processor, deploy the generated assets, and verify the schema URL:

```sh
cargo test
npx wrangler login
npx wrangler deploy
curl --fail https://doubleclick-dictionary-data.evilwaveforms.workers.dev/v2/metadata.json
```

Wrangler login is only needed when the local session is not already authenticated. Add the production custom domain in Cloudflare after the first deployment. Configure the extension with the schema URL, such as `https://example.workers.dev/v2`, rather than the deployment root.

For an ordinary dictionary refresh, keep `SCHEMA_VERSION` unchanged, run `scripts/update-dictionary.sh` with the new source date, and repeat the commands above.

For an incompatible format change:

1. Increment `SCHEMA_VERSION` in `src/pipeline.rs`.
2. Generate and test the dictionary.
3. Deploy the data and verify the new versioned metadata URL.
4. Update the extension's schema version and dictionary URL.
5. Test and release the extension.

The previous schema remains deployed for extension installations that have not updated yet. When another schema is generated later, the generator keeps the two newest schemas and removes older generated schema directories before deployment.

## Lookup contract

The extension normalizes a lookup as `<language>:<word>`, applies 32-bit FNV-1a and masks the result by `shardCount - 1`. For 8192 shards, `en:hello` is in `v2/shards/0268.json`.

Each shard has this shape:

```json
{
  "schemaVersion": 2,
  "entries": {
    "en:hello": [
      {
        "word": "hello",
        "phonetic": "/həˈləʊ/",
        "sourceUrl": "https://en.wiktionary.org/wiki/hello",
        "meanings": [
          {
            "partOfSpeech": "interjection",
            "definitions": [{ "text": "A greeting." }]
          }
        ]
      }
    ]
  }
}
```

Entries sharing a case-insensitive lookup key remain separate variants in the same shard. Clients should prefer an exact spelling match, then the lowercase variant, then the first available variant.

## License

The processor source code is MIT licensed. Generated Wiktionary data is modified and distributed separately under CC BY-SA 4.0; see [NOTICE.md](NOTICE.md). Deploy the complete generated output so `metadata.json`, `licenses/wiktionary.txt`, and each entry's `sourceUrl` remain available.

Pronunciation audio is intentionally not copied into generated output. Commons media files have individual licenses and attribution requirements that need a separate license-aware ingestion step.
