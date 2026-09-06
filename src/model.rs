use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct RawEntry {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub lang_code: String,
    #[serde(default)]
    pub pos: String,
    #[serde(default)]
    pub senses: Vec<RawSense>,
    #[serde(default)]
    pub sounds: Vec<RawSound>,
}

#[derive(Deserialize)]
pub struct RawSense {
    #[serde(default)]
    pub glosses: Vec<String>,
    #[serde(default)]
    pub examples: Vec<RawExample>,
}

#[derive(Deserialize)]
pub struct RawExample {
    pub text: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
}

#[derive(Deserialize)]
pub struct RawSound {
    pub ipa: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meaning {
    pub part_of_speech: String,
    pub definitions: Vec<Definition>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DictionaryEntry {
    pub word: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phonetic: Option<String>,
    pub source_url: String,
    pub meanings: Vec<Meaning>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryFragment {
    pub shard: usize,
    pub key: String,
    pub word: String,
    pub phonetic: Option<String>,
    pub source_url: String,
    pub meaning: Meaning,
}
