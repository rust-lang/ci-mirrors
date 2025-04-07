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
                result.push(match file {
                    ManifestFile::Legacy(legacy) => MirrorFile {
                        name: legacy.name,
                        sha256: legacy.sha256,
                        source: Source::Legacy,
                    },
                    ManifestFile::Managed(managed) => MirrorFile {
                        name: managed.name,
                        sha256: managed.sha256,
                        source: Source::Url(managed.source),
                    },
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
    pub(crate) source: Source,
}

pub(crate) enum Source {
    Url(Url),
    Legacy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    files: Vec<ManifestFile>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ManifestFile {
    Legacy(ManifestFileLegacy),
    Managed(ManifestFileManaged),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFileLegacy {
    name: String,
    sha256: String,
    #[serde(deserialize_with = "deserialize_true")]
    #[expect(unused)]
    legacy: (),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFileManaged {
    name: String,
    sha256: String,
    #[serde(deserialize_with = "deserialize_url")]
    source: Url,
    // This field is not considered at all by the automation, we just enforce its presence so that
    // people adding new entries think about the licensing implications.
    #[expect(unused)]
    license: String,
}

fn deserialize_url<'de, D: Deserializer<'de>>(de: D) -> Result<Url, D::Error> {
    let raw = String::deserialize(de)?;
    Url::parse(&raw).map_err(|e| D::Error::custom(format!("{e:?}")))
}

fn deserialize_true<'de, D: Deserializer<'de>>(de: D) -> Result<(), D::Error> {
    let raw = bool::deserialize(de)?;
    if raw {
        Ok(())
    } else {
        Err(D::Error::custom("must be true"))
    }
}
