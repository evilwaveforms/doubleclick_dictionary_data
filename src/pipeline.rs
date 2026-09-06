use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use serde::Serialize;

use crate::cli::Config;
use crate::model::{Definition, DictionaryEntry, EntryFragment, Meaning, RawEntry};
use crate::normalize::{lookup_key, shard_index};

const SCHEMA_VERSION: u32 = 2;
const PARTITION_LIMIT: usize = 256;
const MAX_MEANINGS: usize = 6;
const MAX_DEFINITIONS_PER_MEANING: usize = 4;
const OUTPUT_MARKER: &str = ".dictionary-data-output";
const ROOT_MARKER: &str = ".dictionary-data-root";
const RETAINED_SCHEMA_VERSIONS: usize = 2;

pub struct BuildStats {
    pub entries: usize,
    pub shards: usize,
    pub accepted_records: usize,
    pub skipped_records: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ShardDocument<'a> {
    schema_version: u32,
    entries: &'a BTreeMap<String, Vec<DictionaryEntry>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Metadata<'a> {
    schema_version: u32,
    source: &'static str,
    source_date: &'a str,
    license: &'static str,
    license_url: &'static str,
    attribution_url: &'static str,
    modified: bool,
    modification_notice: &'static str,
    languages: Vec<&'a str>,
    shard_count: usize,
    entry_count: usize,
}

pub fn build(config: &Config) -> Result<BuildStats, String> {
    prepare_output_root(&config.output)?;
    let version_output = config.output.join(format!("v{SCHEMA_VERSION}"));
    let staging = staging_path(&version_output)?;
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|error| path_error("clear staging directory", &staging, error))?;
    }
    fs::create_dir_all(&staging).map_err(|error| path_error("create staging directory", &staging, error))?;

    let result = build_staged(config, &staging);
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
        return result;
    }

    publish(&staging, &version_output)?;
    prune_schema_versions(&config.output)?;
    write_deployment_files(&config.output)?;
    result
}

fn build_staged(config: &Config, staging: &Path) -> Result<BuildStats, String> {
    let partition_count = config.shard_count.min(PARTITION_LIMIT);
    let work = staging.join(".work");
    fs::create_dir_all(&work).map_err(|error| path_error("create work directory", &work, error))?;

    let mut partitions = create_partitions(&work, partition_count)?;
    let mut accepted_records = 0;
    let mut skipped_records = 0;
    let mut line = String::new();
    let mut line_number = 0_usize;
    let mut input = open_input(&config.input)?;

    loop {
        line.clear();
        if input.read_line(&mut line).map_err(|error| path_error("read input", &config.input, error))? == 0 {
            break;
        }
        line_number += 1;
        if line.trim().is_empty() {
            continue;
        }

        let raw: RawEntry = serde_json::from_str(&line)
            .map_err(|error| format!("invalid JSON on input line {line_number}: {error}"))?;
        let Some(fragment) = to_fragment(raw, config) else {
            skipped_records += 1;
            continue;
        };

        let partition = fragment.shard % partition_count;
        serde_json::to_writer(&mut partitions[partition], &fragment)
            .map_err(|error| format!("serialize intermediate entry: {error}"))?;
        partitions[partition]
            .write_all(b"\n")
            .map_err(|error| path_error("write intermediate entry", &work, error))?;
        accepted_records += 1;
    }
    for writer in &mut partitions {
        writer.flush().map_err(|error| path_error("flush intermediate entries", &work, error))?;
    }
    drop(partitions);

    let shards_directory = staging.join("shards");
    fs::create_dir_all(&shards_directory)
        .map_err(|error| path_error("create shards directory", &shards_directory, error))?;
    let mut entry_count = 0;
    let mut written_shards = 0;

    for partition in 0..partition_count {
        let grouped = read_partition(&work.join(format!("{partition:03}.jsonl")))?;
        for (shard, entries) in grouped {
            entry_count += entries.values().map(Vec::len).sum::<usize>();
            write_json(
                &shards_directory.join(format!("{shard:04x}.json")),
                &ShardDocument { schema_version: SCHEMA_VERSION, entries: &entries },
            )?;
            written_shards += 1;
        }
    }

    fs::remove_dir_all(&work).map_err(|error| path_error("remove work directory", &work, error))?;
    let metadata = Metadata {
        schema_version: SCHEMA_VERSION,
        source: "Wiktionary via Wiktextract",
        source_date: &config.source_date,
        license: "CC BY-SA 4.0",
        license_url: "https://creativecommons.org/licenses/by-sa/4.0/",
        attribution_url: "https://en.wiktionary.org/",
        modified: true,
        modification_notice: "Wiktextract parsed the source; this generator selected, combined, normalized, and truncated fields for dictionary lookup.",
        languages: config.languages.iter().map(String::as_str).collect(),
        shard_count: config.shard_count,
        entry_count,
    };
    write_json(&staging.join("metadata.json"), &metadata)?;
    let licenses = staging.join("licenses");
    fs::create_dir(&licenses).map_err(|error| path_error("create licenses directory", &licenses, error))?;
    fs::write(licenses.join("wiktionary.txt"), wiktionary_notice())
        .map_err(|error| path_error("write Wiktionary notice", &licenses, error))?;
    fs::write(staging.join(OUTPUT_MARKER), "generated by doubleclick-dictionary-data\n")
        .map_err(|error| path_error("write output marker", staging, error))?;

    Ok(BuildStats {
        entries: entry_count,
        shards: written_shards,
        accepted_records,
        skipped_records,
    })
}

fn to_fragment(raw: RawEntry, config: &Config) -> Option<EntryFragment> {
    if !config.languages.contains(&raw.lang_code) {
        return None;
    }
    let key = lookup_key(&raw.lang_code, &raw.word)?;
    let definitions = raw
        .senses
        .into_iter()
        .filter_map(|sense| {
            let text = sense.glosses.into_iter().find(|gloss| !gloss.trim().is_empty())?;
            let example = sense.examples.into_iter().find_map(|example| {
                if example.kind.as_deref() == Some("quotation") || example.reference.is_some() {
                    None
                } else {
                    example.text
                }
            });
            Some(Definition { text, example })
        })
        .take(MAX_DEFINITIONS_PER_MEANING)
        .collect::<Vec<_>>();
    if definitions.is_empty() {
        return None;
    }

    let phonetic = raw.sounds.iter().find_map(|sound| sound.ipa.clone());
    let source_url = wiktionary_page_url(&raw.word);

    Some(EntryFragment {
        shard: shard_index(&key, config.shard_count),
        key,
        word: raw.word,
        phonetic,
        source_url,
        meaning: Meaning { part_of_speech: raw.pos, definitions },
    })
}

fn create_partitions(directory: &Path, count: usize) -> Result<Vec<BufWriter<File>>, String> {
    (0..count)
        .map(|partition| {
            let path = directory.join(format!("{partition:03}.jsonl"));
            File::create(&path)
                .map(BufWriter::new)
                .map_err(|error| path_error("create partition", &path, error))
        })
        .collect()
}

fn read_partition(path: &Path) -> Result<BTreeMap<usize, BTreeMap<String, Vec<DictionaryEntry>>>, String> {
    let file = File::open(path).map_err(|error| path_error("open partition", path, error))?;
    let mut grouped = BTreeMap::<usize, BTreeMap<String, Vec<DictionaryEntry>>>::new();

    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| path_error("read partition", path, error))?;
        let fragment: EntryFragment = serde_json::from_str(&line)
            .map_err(|error| format!("invalid intermediate record in {}: {error}", path.display()))?;
        let entries = grouped.entry(fragment.shard).or_default();
        merge_fragment(entries, fragment);
    }

    Ok(grouped)
}

fn merge_fragment(entries: &mut BTreeMap<String, Vec<DictionaryEntry>>, fragment: EntryFragment) {
    let variants = entries.entry(fragment.key).or_default();
    let Some(entry) = variants.iter_mut().find(|entry| entry.word == fragment.word) else {
        variants.push(DictionaryEntry {
            word: fragment.word,
            phonetic: fragment.phonetic,
            source_url: fragment.source_url,
            meanings: vec![fragment.meaning],
        });
        return;
    };
    entry.phonetic = entry.phonetic.take().or(fragment.phonetic);

    if let Some(meaning) = entry
        .meanings
        .iter_mut()
        .find(|meaning| meaning.part_of_speech == fragment.meaning.part_of_speech)
    {
        for definition in fragment.meaning.definitions {
            if meaning.definitions.len() == MAX_DEFINITIONS_PER_MEANING {
                break;
            }
            if !meaning.definitions.iter().any(|existing| existing.text == definition.text) {
                meaning.definitions.push(definition);
            }
        }
    } else if entry.meanings.len() < MAX_MEANINGS {
        entry.meanings.push(fragment.meaning);
    }
}

fn open_input(path: &Path) -> Result<Box<dyn BufRead>, String> {
    let file = File::open(path).map_err(|error| path_error("open input", path, error))?;
    if path.extension().is_some_and(|extension| extension == "gz") {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let file = File::create(path).map_err(|error| path_error("create JSON output", path, error))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, value)
        .map_err(|error| format!("serialize {}: {error}", path.display()))?;
    writer.write_all(b"\n").map_err(|error| path_error("write JSON output", path, error))
}

fn staging_path(output: &Path) -> Result<PathBuf, String> {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "--output must name a directory".to_owned())?;
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    Ok(parent.join(format!(".{name}-{}.tmp", std::process::id())))
}

fn prepare_output_root(output: &Path) -> Result<(), String> {
    if output.exists() {
        if !output.is_dir() {
            return Err(format!("--output must name a directory: {}", output.display()));
        }
        if !output.join(ROOT_MARKER).is_file() {
            if fs::read_dir(output)
                .map_err(|error| path_error("read output directory", output, error))?
                .next()
                .is_some()
            {
                return Err(format!(
                    "refusing to use {} because it was not created as a versioned output root",
                    output.display()
                ));
            }
        }
    } else {
        fs::create_dir_all(output).map_err(|error| path_error("create output directory", output, error))?;
    }

    fs::write(output.join(ROOT_MARKER), "generated by doubleclick-dictionary-data\n")
        .map_err(|error| path_error("write output root marker", output, error))?;
    validate_schema_directories(output)
}

fn validate_schema_directories(output: &Path) -> Result<(), String> {
    for entry in fs::read_dir(output).map_err(|error| path_error("read output directory", output, error))? {
        let entry = entry.map_err(|error| path_error("read output directory entry", output, error))?;
        if !entry.file_type().map_err(|error| path_error("read output entry type", &entry.path(), error))?.is_dir() {
            continue;
        }
        let Some(version) = schema_version(&entry.file_name()) else {
            continue;
        };
        if !entry.path().join(OUTPUT_MARKER).is_file() {
            return Err(format!(
                "refusing to manage {} because it was not generated by this tool",
                entry.path().display()
            ));
        }
        if version > SCHEMA_VERSION {
            return Err(format!(
                "refusing to replace newer schema v{version} with v{SCHEMA_VERSION}"
            ));
        }
    }
    Ok(())
}

fn prune_schema_versions(output: &Path) -> Result<(), String> {
    let mut versions = Vec::new();
    for entry in fs::read_dir(output).map_err(|error| path_error("read output directory", output, error))? {
        let entry = entry.map_err(|error| path_error("read output directory entry", output, error))?;
        if !entry.file_type().map_err(|error| path_error("read output entry type", &entry.path(), error))?.is_dir() {
            continue;
        }
        if let Some(version) = schema_version(&entry.file_name()) {
            versions.push((version, entry.path()));
        }
    }
    versions.sort_unstable_by(|left, right| right.0.cmp(&left.0));
    for (_, path) in versions.into_iter().skip(RETAINED_SCHEMA_VERSIONS) {
        fs::remove_dir_all(&path).map_err(|error| path_error("remove old schema version", &path, error))?;
    }
    Ok(())
}

fn schema_version(name: &std::ffi::OsStr) -> Option<u32> {
    let digits = name.to_str()?.strip_prefix('v')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn write_deployment_files(output: &Path) -> Result<(), String> {
    fs::write(output.join("_headers"), headers())
        .map_err(|error| path_error("write _headers", output, error))?;
    fs::write(
        output.join(".assetsignore"),
        format!("{ROOT_MARKER}\n**/{OUTPUT_MARKER}\n.assetsignore\n"),
    )
    .map_err(|error| path_error("write .assetsignore", output, error))
}

fn publish(staging: &Path, output: &Path) -> Result<(), String> {
    if output.exists() {
        if !output.join(OUTPUT_MARKER).is_file() {
            return Err(format!(
                "refusing to replace {} because it was not generated by this tool",
                output.display()
            ));
        }
        let name = output.file_name().and_then(|name| name.to_str()).unwrap();
        let parent = output.parent().unwrap_or_else(|| Path::new("."));
        let backup = parent.join(format!(".{name}-{}.old", std::process::id()));
        if backup.exists() {
            return Err(format!("stale backup exists at {}", backup.display()));
        }
        fs::rename(output, &backup).map_err(|error| path_error("back up existing output", output, error))?;
        if let Err(error) = fs::rename(staging, output) {
            let _ = fs::rename(&backup, output);
            return Err(path_error("publish generated output", output, error));
        }
        fs::remove_dir_all(&backup).map_err(|error| path_error("remove previous output", &backup, error))?;
        return Ok(());
    }
    fs::rename(staging, output).map_err(|error| path_error("publish generated output", output, error))
}

fn headers() -> &'static str {
    "/:version/shards/*\n  Access-Control-Allow-Origin: *\n  Cache-Control: public, max-age=3600, must-revalidate\n\n/:version/metadata.json\n  Access-Control-Allow-Origin: *\n  Cache-Control: public, max-age=300, must-revalidate\n"
}

fn wiktionary_notice() -> &'static str {
    "Dictionary data license and attribution\n\nThe definitions, ordinary usage examples, parts of speech, and phonetic transcriptions in this distribution are derived from Wiktionary contributors. Each generated entry has a sourceUrl linking to the corresponding Wiktionary page and its contributor history.\n\nSource: https://en.wiktionary.org/\nCopyright information: https://en.wiktionary.org/wiki/Wiktionary:Copyrights\nLicense: Creative Commons Attribution-ShareAlike 4.0 International\nLicense URL: https://creativecommons.org/licenses/by-sa/4.0/\n\nChanges were made. Wiktextract converted Wiktionary markup to structured data. This generator selected supported languages and fields, normalized lookup keys, combined entries, limited meanings and definitions, omitted sourced quotations and other unused fields, and emitted hashed JSON shards. The generated dictionary data is distributed under CC BY-SA 4.0.\n\nWiktextract: https://github.com/tatuylonen/wiktextract\nNo endorsement by Wiktionary, its contributors, or the Wikimedia Foundation is implied.\n"
}

fn wiktionary_page_url(word: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut url = String::from("https://en.wiktionary.org/wiki/");
    for byte in word.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => url.push(byte as char),
            b' ' => url.push('_'),
            _ => {
                url.push('%');
                url.push(HEX[(byte >> 4) as usize] as char);
                url.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    url
}

fn path_error(action: &str, path: &Path, error: io::Error) -> String {
    format!("could not {action} {}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::cli::Config;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::fs;

    #[test]
    fn builds_and_merges_dictionary_shards() {
        let root = std::env::temp_dir().join(format!("dictionary-data-test-{}", std::process::id()));
        let input = root.join("input.jsonl");
        let output = root.join("output");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &input,
            concat!(
                r#"{"word":"Hello","lang_code":"en","pos":"interjection","senses":[{"glosses":["A greeting."],"examples":[{"text":"Quoted hello.","type":"quotation","ref":"Example Author"},{"text":"Hello there.","type":"example"}]}],"sounds":[{"ipa":"/həˈləʊ/","mp3_url":"https://audio.example/hello.mp3"}]}"#,
                "\n",
                r#"{"word":"hello","lang_code":"en","pos":"noun","senses":[{"glosses":["An utterance of hello."]}]}"#,
                "\n",
                r#"{"word":"bonjour","lang_code":"fr","pos":"interjection","senses":[{"glosses":["Hello."]}]}"#,
                "\n",
                r#"{"title":"grain of salt","redirect":"with a grain of salt","pos":"hard-redirect"}"#,
                "\n",
            ),
        )
        .unwrap();
        let config = Config {
            input,
            output: output.clone(),
            languages: BTreeSet::from(["en".to_owned()]),
            shard_count: 4,
            source_date: "2026-09-01".to_owned(),
        };

        let stats = build(&config).unwrap();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.accepted_records, 2);
        assert_eq!(stats.skipped_records, 2);

        let shard = super::shard_index("en:hello", 4);
        let version_output = output.join("v2");
        let document: Value = serde_json::from_slice(
            &fs::read(version_output.join(format!("shards/{shard:04x}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(document["schemaVersion"], 2);
        let variants = document["entries"]["en:hello"].as_array().unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0]["word"], "Hello");
        assert_eq!(variants[0]["meanings"].as_array().unwrap().len(), 1);
        assert_eq!(variants[0]["phonetic"], "/həˈləʊ/");
        assert_eq!(variants[0]["meanings"][0]["definitions"][0]["example"], "Hello there.");
        assert_eq!(variants[0]["sourceUrl"], "https://en.wiktionary.org/wiki/Hello");
        assert_eq!(variants[1]["word"], "hello");
        assert_eq!(variants[1]["meanings"].as_array().unwrap().len(), 1);

        let metadata: Value = serde_json::from_slice(&fs::read(version_output.join("metadata.json")).unwrap()).unwrap();
        assert_eq!(metadata["license"], "CC BY-SA 4.0");
        assert_eq!(metadata["modified"], true);
        let notice = fs::read_to_string(version_output.join("licenses/wiktionary.txt")).unwrap();
        assert!(notice.contains("Changes were made"));
        assert!(notice.contains("CC BY-SA 4.0"));
        assert!(output.join("_headers").is_file());
        assert!(output.join(".assetsignore").is_file());

        for version in ["v0", "v1"] {
            let directory = output.join(version);
            fs::create_dir(&directory).unwrap();
            fs::write(directory.join(super::OUTPUT_MARKER), "test\n").unwrap();
        }
        build(&config).unwrap();
        assert!(!output.join("v0").exists());
        assert!(output.join("v1").exists());
        assert!(output.join("v2").exists());

        let newer = output.join("v3");
        fs::create_dir(&newer).unwrap();
        fs::write(newer.join(super::OUTPUT_MARKER), "test\n").unwrap();
        let error = build(&config).err().unwrap();
        assert!(error.contains("refusing to replace newer schema v3 with v2"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encodes_wiktionary_page_urls() {
        assert_eq!(
            super::wiktionary_page_url("Café au lait/#"),
            "https://en.wiktionary.org/wiki/Caf%C3%A9_au_lait%2F%23",
        );
    }
}
