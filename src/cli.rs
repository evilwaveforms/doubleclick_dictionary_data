use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;

pub struct Config {
    pub input: PathBuf,
    pub output: PathBuf,
    pub languages: BTreeSet<String>,
    pub shard_count: usize,
    pub source_date: String,
}

impl Config {
    pub fn parse(args: impl Iterator<Item = OsString>) -> Result<Option<Self>, String> {
        let mut args = args;
        let mut input = None;
        let mut output = None;
        let mut languages = BTreeSet::new();
        let mut shard_count = 8192_usize;
        let mut source_date = None;

        while let Some(argument) = args.next() {
            let flag = argument
                .to_str()
                .ok_or_else(|| "arguments must be valid UTF-8".to_owned())?;
            match flag {
                "--help" | "-h" => {
                    println!("{}", Self::usage());
                    return Ok(None);
                }
                "--input" => input = Some(PathBuf::from(next_value(&mut args, flag)?)),
                "--output" => output = Some(PathBuf::from(next_value(&mut args, flag)?)),
                "--language" => {
                    languages.insert(next_string(&mut args, flag)?);
                }
                "--shards" => {
                    shard_count = next_string(&mut args, flag)?
                        .parse()
                        .map_err(|_| "--shards must be an integer".to_owned())?;
                }
                "--source-date" => source_date = Some(next_string(&mut args, flag)?),
                _ => return Err(format!("unknown argument: {flag}")),
            }
        }

        if shard_count == 0 || shard_count > 16384 || !shard_count.is_power_of_two() {
            return Err("--shards must be a power of two between 1 and 16384".to_owned());
        }

        Ok(Some(Self {
            input: input.ok_or_else(|| "--input is required".to_owned())?,
            output: output.ok_or_else(|| "--output is required".to_owned())?,
            languages: if languages.is_empty() {
                return Err("at least one --language is required".to_owned());
            } else {
                languages
            },
            shard_count,
            source_date: source_date.ok_or_else(|| "--source-date is required".to_owned())?,
        }))
    }

    pub fn usage() -> &'static str {
        "Usage: doubleclick-dictionary-data \\
  --input <wiktextract.jsonl[.gz]> \\
  --output <directory> \\
  --language <code> [--language <code> ...] \\
  --source-date <YYYY-MM-DD> \\
  [--shards <power-of-two>]"
    }
}

fn next_value(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString, String> {
    args.next().ok_or_else(|| format!("{flag} requires a value"))
}

fn next_string(args: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<String, String> {
    next_value(args, flag)?
        .into_string()
        .map_err(|_| format!("{flag} must be valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::ffi::OsString;

    #[test]
    fn parses_repeated_languages() {
        let args = [
            "--input", "input.jsonl.gz", "--output", "dist", "--language", "en",
            "--language", "fi", "--source-date", "2026-09-01", "--shards", "1024",
        ]
        .into_iter()
        .map(OsString::from);

        let config = Config::parse(args).unwrap().unwrap();
        assert_eq!(config.shard_count, 1024);
        assert_eq!(config.languages.into_iter().collect::<Vec<_>>(), ["en", "fi"]);
    }

    #[test]
    fn rejects_non_power_of_two_shard_count() {
        let args = [
            "--input", "input.jsonl", "--output", "dist", "--language", "en",
            "--source-date", "2026-09-01", "--shards", "1000",
        ]
        .into_iter()
        .map(OsString::from);

        assert!(Config::parse(args).is_err());
    }
}
