use anyhow::Error;
use reqwest::Url;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) files: Vec<ManifestFile>,
}

impl Manifest {
    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        let raw = std::fs::read_to_string(path)?;
        let manifest = toml::from_str(&raw)?;
        validate_manifest(&manifest)?;
        Ok(manifest)
    }
}

fn validate_manifest(manifest: &Manifest) -> anyhow::Result<()> {
    use std::fmt::Write;

    let mut name_map: HashMap<&str, Vec<&ManifestFile>> =
        HashMap::with_capacity(manifest.files.len());
    let mut url_map: HashMap<&Url, Vec<&ManifestFile>> =
        HashMap::with_capacity(manifest.files.len());
    let mut hash_map: HashMap<&str, Vec<&ManifestFile>> =
        HashMap::with_capacity(manifest.files.len());

    for file in &manifest.files {
        name_map.entry(&file.name).or_default().push(file);
        url_map.entry(&file.source).or_default().push(file);
        hash_map.entry(&file.sha256).or_default().push(file);
    }

    fn format_conflicts(conflicts: Vec<&ManifestFile>) -> String {
        conflicts
            .into_iter()
            .map(|file| {
                let ManifestFile {
                    name,
                    source,
                    sha256,
                } = file;
                format!(
                    r#"[[files]]
name = "{name}"
source = "{source}"
sha256 = "{sha256}"
"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    let mut error = String::new();
    for (name, conflicts) in name_map.into_iter().filter(|(_, files)| files.len() > 1) {
        writeln!(
            error,
            "The following files share the name `{name}`:\n{}\n",
            format_conflicts(conflicts)
        )?;
    }
    for (url, conflicts) in url_map.into_iter().filter(|(_, files)| files.len() > 1) {
        writeln!(
            error,
            "The following files share the URL `{url}`:\n{}\n",
            format_conflicts(conflicts)
        )?;
    }
    for (hash, conflicts) in hash_map.into_iter().filter(|(_, files)| files.len() > 1) {
        writeln!(
            error,
            "The following files share the hash `{hash}`:\n{}\n",
            format_conflicts(conflicts)
        )?;
    }

    if error.is_empty() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Found error(s) in the file manifest:\n{error}"
        ))
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
