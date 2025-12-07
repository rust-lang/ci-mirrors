use anyhow::{Context, Error};
use reqwest::Url;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(PartialEq, Eq, Clone)]
struct Span(std::ops::Range<usize>);

impl PartialOrd for Span {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Span {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .start
            .partial_cmp(&other.0.start)
            .unwrap()
            .then(self.0.end.partial_cmp(&other.0.end).unwrap())
    }
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone)]
struct Location {
    file: PathBuf,
    span: Span,
}

#[derive(Default)]
struct LocationCache {
    seen_urls: HashMap<Url, BTreeSet<Location>>,
    seen_hashes: HashMap<String, BTreeSet<Location>>,
    seen_paths: HashMap<String, BTreeSet<Location>>,
}

/// Returns (files, found errors).
/// Errors are returned explicitly so that they can be merged with follow-up errors, rather than
/// exiting immediately.
pub(crate) fn load_manifests(load_from: &Path) -> Result<(Vec<MirrorFile>, Vec<String>), Error> {
    let mut result = Vec::new();
    let mut cache = LocationCache::default();
    let mut errors = Vec::new();

    fn emit_error(
        error: String,
        mirror_file: &MirrorFile,
        file_source: &str,
        cache: &LocationCache,
        errors: &mut Vec<String>,
    ) {
        let location = cache
            .seen_paths
            .get(&mirror_file.name)
            .unwrap()
            .first()
            .unwrap();
        let (src_line, snippet) = span_info(&file_source, location);
        errors.push(format!(
            "{error}:\n\
             # {} (line {src_line})\n{snippet}\n",
            location.file.display()
        ));
    }

    fn load_inner(
        load_from: &Path,
        result: &mut Vec<MirrorFile>,
        cache: &mut LocationCache,
        errors: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        for entry in load_from.read_dir()? {
            let path = entry?.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("toml") {
                let file_source = std::fs::read_to_string(&path)
                    .map_err(Error::from)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                let manifest = toml::from_str::<Manifest>(&file_source)
                    .map_err(Error::from)
                    .with_context(|| format!("failed to read {}", path.display()))?;
                record_locations(&path, &manifest, cache);

                for file in manifest.files {
                    let mirror_file = match file.into_inner() {
                        ManifestFile::Legacy(legacy) => MirrorFile {
                            name: legacy.name,
                            sha256: legacy.sha256,
                            source: Source::Legacy,
                            rename_from: None,
                        },
                        ManifestFile::Managed(managed) => MirrorFile {
                            name: managed.name,
                            sha256: managed.sha256,
                            source: Source::Url(managed.source),
                            rename_from: managed.rename_from,
                        },
                    };
                    if mirror_file.name.starts_with('/') {
                        emit_error(
                            "Mirrored path cannot start with a slash (/)".to_string(),
                            &mirror_file,
                            &file_source,
                            cache,
                            errors,
                        );
                    }

                    if let Source::Url(ref source) = mirror_file.source {
                        if let Some(file_name) = source.path().split('/').last()
                            && let Some(path_name) = mirror_file.name.split('/').last()
                        {
                            match mirror_file.rename_from {
                                Some(ref rename_from) => {
                                    if path_name == file_name {
                                        emit_error(
                                            format!(
                                                "`rename-from` field isn't needed since `source` and `name` field have the same file name (`{file_name}`)"
                                            ),
                                            &mirror_file,
                                            &file_source,
                                            cache,
                                            errors,
                                        );
                                    } else if rename_from != file_name {
                                        emit_error(
                                            format!(
                                                "`rename-from` field value doesn't match name from the URL `{source}` (`{file_name}` != `{rename_from}`)"
                                            ),
                                            &mirror_file,
                                            &file_source,
                                            cache,
                                            errors,
                                        );
                                    }
                                }
                                None => {
                                    if path_name != file_name {
                                        emit_error(
                                            format!(
                                                "The name from the URL `{source}` doesn't match the `name` field (`{file_name}` != `{path_name}`). \
                                             Add `rename-from = {file_name:?}` to fix this error"
                                            ),
                                            &mirror_file,
                                            &file_source,
                                            cache,
                                            errors,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    result.push(mirror_file);
                }
            } else if path.is_dir() {
                load_inner(&path, result, cache, errors)?;
            }
        }
        Ok(())
    }

    load_inner(load_from, &mut result, &mut cache, &mut errors)?;
    find_errors(cache, &mut errors);
    Ok((result, errors))
}

fn record_locations(toml_path: &Path, manifest: &Manifest, cache: &mut LocationCache) {
    for file in &manifest.files {
        let span = Span(file.span());
        let file = file.get_ref();

        let location = Location {
            file: toml_path.to_owned(),
            span,
        };
        let (hash, name, url) = match file {
            ManifestFile::Legacy(f) => {
                if f.skip_validation {
                    return;
                }

                (f.sha256.clone(), f.name.clone(), None)
            }
            ManifestFile::Managed(f) => (f.sha256.clone(), f.name.clone(), Some(f.source.clone())),
        };
        cache
            .seen_hashes
            .entry(hash)
            .or_default()
            .insert(location.clone());
        cache
            .seen_paths
            .entry(name)
            .or_default()
            .insert(location.clone());
        if let Some(url) = url {
            cache
                .seen_urls
                .entry(url)
                .or_default()
                .insert(location.clone());
        }
    }
}

fn span_info<'a>(content: &'a str, location: &Location) -> (usize, &'a str) {
    // Find the corresponding line number
    let mut accumulated_chars = 0;
    let mut src_line = 0;
    for (index, line) in content.lines().enumerate() {
        accumulated_chars += line.len() + 1; // +1 for newline
        if accumulated_chars > location.span.0.start {
            src_line = index + 1;
            break;
        }
    }

    let snippet = &content[location.span.0.start..location.span.0.end];
    (src_line, snippet)
}

fn find_errors(cache: LocationCache, errors: &mut Vec<String>) {
    let mut file_cache: HashMap<PathBuf, String> = HashMap::new();

    fn format_locations(
        cache: &mut HashMap<PathBuf, String>,
        locations: &BTreeSet<Location>,
    ) -> String {
        use std::fmt::Write;

        let mut output = String::new();
        for location in locations {
            let content = cache.entry(location.file.clone()).or_insert_with(|| {
                std::fs::read_to_string(&location.file).unwrap_or_else(|e| {
                    panic!("cannot load file from {}: {e:?}", location.file.display())
                })
            });

            let (src_line, snippet) = span_info(&content, location);
            writeln!(
                output,
                "# {} (line {src_line})\n{snippet}\n",
                location.file.display()
            )
            .unwrap();
        }

        output
    }

    for (path, locations) in cache.seen_paths {
        if locations.len() > 1 {
            errors.push(format!(
                "The following entries share the same CDN path `{path}`:\n{}",
                format_locations(&mut file_cache, &locations)
            ));
        }
    }
    for (url, locations) in cache.seen_urls {
        if locations.len() > 1 {
            errors.push(format!(
                "The following entries share the same URL `{url}`:\n{}",
                format_locations(&mut file_cache, &locations)
            ));
        }
    }
    for (hash, locations) in cache.seen_hashes {
        if locations.len() > 1 {
            errors.push(format!(
                "The following entries share the same hash `{hash}`:\n{}",
                format_locations(&mut file_cache, &locations)
            ));
        }
    }
}

pub(crate) struct MirrorFile {
    pub(crate) name: String,
    pub(crate) sha256: String,
    pub(crate) source: Source,
    pub(crate) rename_from: Option<String>,
}

pub(crate) enum Source {
    Url(Url),
    Legacy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    files: Vec<toml::Spanned<ManifestFile>>,
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
    #[serde(default, rename = "skip-validation")]
    skip_validation: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFileManaged {
    name: String,
    sha256: String,
    #[serde(deserialize_with = "deserialize_url", serialize_with = "serialize_url")]
    source: Url,
    // This field is not considered at all by the automation, we just enforce its presence so that
    // people adding new entries think about the licensing implications.
    license: String,
    #[serde(default, rename = "rename-from")]
    rename_from: Option<String>,
}

impl ManifestFileManaged {
    pub fn new(
        name: String,
        sha256: String,
        source: Url,
        license: String,
        rename_from: Option<String>,
    ) -> Self {
        Self {
            name,
            sha256,
            source,
            license,
            rename_from,
        }
    }
}

fn deserialize_url<'de, D: Deserializer<'de>>(de: D) -> Result<Url, D::Error> {
    let raw = String::deserialize(de)?;
    Url::parse(&raw).map_err(|e| D::Error::custom(format!("{e:?}")))
}

fn serialize_url<S: Serializer>(url: &Url, s: S) -> Result<S::Ok, S::Error> {
    url.as_str().serialize(s)
}

fn deserialize_true<'de, D: Deserializer<'de>>(de: D) -> Result<(), D::Error> {
    let raw = bool::deserialize(de)?;
    if raw {
        Ok(())
    } else {
        Err(D::Error::custom("must be true"))
    }
}
