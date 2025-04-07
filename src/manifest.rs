use anyhow::{Context, Error};
use reqwest::Url;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::path::Path;

pub(crate) fn load_manifests(load_from: &Path) -> Result<Vec<MirrorFile>, Error> {
    let mut result = Vec::new();

    for entry in load_from.read_dir()? {
        let path = entry?.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
            let manifest = std::fs::read_to_string(&path)
                .map_err(Error::from)
                .and_then(|raw| toml::from_str::<Manifest>(&raw).map_err(Error::from))
                .with_context(|| format!("failed to read {}", path.display()))?;

            for file in manifest.files {
                result.push(MirrorFile {
                    name: file.name,
                    sha256: file.sha256,
                    source: file.source,
                });
            }
        } else if path.is_dir() {
            result.extend(load_manifests(&path)?.into_iter());
        }
    }

    Ok(result)
}

pub(crate) struct MirrorFile {
    pub(crate) name: String,
    pub(crate) sha256: String,
    pub(crate) source: Url,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    name: String,
    #[serde(deserialize_with = "deserialize_url")]
    source: Url,
    sha256: String,
}

fn deserialize_url<'de, D: Deserializer<'de>>(de: D) -> Result<Url, D::Error> {
    let raw = String::deserialize(de)?;
    Url::parse(&raw).map_err(|e| D::Error::custom(format!("{e:?}")))
}
