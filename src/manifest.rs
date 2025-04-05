use anyhow::Error;
use reqwest::Url;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) files: Vec<ManifestFile>,
}

impl Manifest {
    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        let raw = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestFile {
    pub(crate) name: String,
    #[serde(deserialize_with = "deserialize_url")]
    pub(crate) source: Url,
    pub(crate) sha256: String,
}

fn deserialize_url<'de, D: Deserializer<'de>>(de: D) -> Result<Url, D::Error> {
    let raw = String::deserialize(de)?;
    Url::parse(&raw).map_err(|e| D::Error::custom(format!("{e:?}")))
}
