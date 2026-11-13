//! Catalog discovery and immutable, lazy installation of selected templates.

mod files;

use std::collections::BTreeMap;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use mandara_config::app_themes::{CatalogKind, CatalogSource};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::manifest::{Manifest, valid_id};

/// Bound discovery and downloads independently of remote Content-Length.
const MAX_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_PACKAGE_BYTES: usize = 32 * 1024 * 1024;
const MAX_FILES: usize = 256;
const MAX_APPS: usize = 512;
const REQUEST_TIMEOUT_SECONDS: u64 = 30;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(REQUEST_TIMEOUT_SECONDS);
const MANIFEST_FILE: &str = "template.toml";
const INSTALLED_FILE: &str = "installed.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct File {
    pub name: String,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub mode: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub source: String,
    pub name: String,
    pub category: String,
    pub documentation_url: String,
    pub files: Vec<File>,
    /// Manifest catalogs supply a selected slice of their shared manifest.
    pub manifest: Option<Manifest>,
    pub folder: String,
}

impl Entry {
    pub fn qualified_id(&self) -> String {
        format!("{}:{}", self.source, self.id)
    }
}

#[derive(Deserialize)]
struct Cached {
    source: CatalogSource,
    entries: Vec<Entry>,
}
#[derive(Serialize)]
struct CacheRef<'a> {
    source: &'a CatalogSource,
    entries: &'a [Entry],
}

fn same_source(a: &CatalogSource, b: &CatalogSource) -> bool {
    a.id == b.id
        && a.kind == b.kind
        && a.index_url == b.index_url
        && a.files_url == b.files_url
        && a.directory_url == b.directory_url
}

pub trait Transport {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>>;
}

pub struct Http(ureq::Agent);
impl Default for Http {
    fn default() -> Self {
        Self(
            ureq::Agent::config_builder()
                .https_only(true)
                .timeout_global(Some(REQUEST_TIMEOUT))
                .build()
                .into(),
        )
    }
}
impl Transport for Http {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        crate::offline::check().map_err(anyhow::Error::msg)?;
        crate::theme_pkg::require_https(url).map_err(anyhow::Error::msg)?;
        let mut response = self
            .0
            .get(url)
            .header(
                "User-Agent",
                concat!("mandara-shell/", env!("CARGO_PKG_VERSION")),
            )
            .call()?;
        let mut bytes = Vec::new();
        response
            .body_mut()
            .with_config()
            .limit(limit)
            .reader()
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

fn source_dir(root: &Path, source: &CatalogSource) -> Result<PathBuf> {
    ensure!(valid_id(&source.id), "invalid catalog source id");
    Ok(root.join(&source.id))
}

pub fn list(source: &CatalogSource, root: &Path, refresh: bool) -> Result<Vec<Entry>> {
    list_with(source, root, refresh, &Http::default())
}

pub fn list_with(
    source: &CatalogSource,
    root: &Path,
    refresh: bool,
    transport: &impl Transport,
) -> Result<Vec<Entry>> {
    let directory = source_dir(root, source)?;
    let path = directory.join("catalog.json");
    if !refresh && let Ok(file) = std::fs::File::open(&path) {
        let mut bytes = Vec::new();
        file.take(MAX_CATALOG_BYTES + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_CATALOG_BYTES,
            "cached catalog is too large"
        );
        if let Ok(cached) = serde_json::from_slice::<Cached>(&bytes)
            && same_source(&cached.source, source)
        {
            validate_entries(source, &cached.entries)?;
            return Ok(cached.entries);
        }
    }
    let bytes = transport.get(&source.index_url, MAX_CATALOG_BYTES)?;
    let entries = decode(source, &bytes)?;
    std::fs::create_dir_all(&directory)?;
    mandara_config::store::write_atomic(
        &path,
        &serde_json::to_vec(&CacheRef {
            source,
            entries: &entries,
        })?,
    )?;
    Ok(entries)
}

fn decode(source: &CatalogSource, bytes: &[u8]) -> Result<Vec<Entry>> {
    let entries = match source.kind {
        CatalogKind::Index => {
            #[derive(Deserialize)]
            struct Listing {
                name: String,
                #[serde(default)]
                category: String,
                files: Vec<File>,
            }
            let list: Vec<Listing> = serde_json::from_slice(bytes)?;
            list.into_iter()
                .map(|app| Entry {
                    id: app.name.clone(),
                    source: source.id.clone(),
                    name: app.name.clone(),
                    folder: app.name,
                    category: app.category,
                    documentation_url: String::new(),
                    files: app.files,
                    manifest: None,
                })
                .collect::<Vec<_>>()
        }
        CatalogKind::Manifest => {
            let all = Manifest::parse(std::str::from_utf8(bytes)?)?;
            all.catalog
                .iter()
                .map(|(id, meta)| {
                    let templates = all
                        .templates
                        .iter()
                        .filter(|(key, _)| *key == id || key.starts_with(&format!("{id}_")))
                        .map(|(id, entry)| (id.clone(), entry.clone()))
                        .collect();
                    let selected = Manifest {
                        config: all.config.clone(),
                        catalog: BTreeMap::from([(id.clone(), meta.clone())]),
                        templates,
                    };
                    let folder = selected
                        .templates
                        .values()
                        .find_map(|t| {
                            t.input_path.components().find_map(|c| match c {
                                std::path::Component::Normal(s) => s.to_str().map(str::to_string),
                                _ => None,
                            })
                        })
                        .unwrap_or_else(|| id.clone());
                    Entry {
                        id: id.clone(),
                        source: source.id.clone(),
                        name: meta.name.clone(),
                        category: meta.category.clone(),
                        documentation_url: meta.documentation_url.clone(),
                        files: Vec::new(),
                        manifest: Some(selected),
                        folder,
                    }
                })
                .collect::<Vec<_>>()
        }
    };
    validate_entries(source, &entries)?;
    Ok(entries)
}

fn validate_entries(source: &CatalogSource, entries: &[Entry]) -> Result<()> {
    ensure!(
        entries.len() <= MAX_APPS,
        "catalog has more than {MAX_APPS} apps"
    );
    let mut ids = std::collections::BTreeSet::new();
    for entry in entries {
        ensure!(
            entry.source == source.id && ids.insert(&entry.id),
            "duplicate app or incorrect source in catalog"
        );
        ensure!(
            valid_id(&entry.id) && valid_id(&entry.folder),
            "invalid app directory in catalog"
        );
        validate_files(&entry.files)?;
    }
    Ok(())
}

fn safe_file(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}
fn validate_files(files: &[File]) -> Result<()> {
    ensure!(files.len() <= MAX_FILES, "theme package has too many files");
    let names: std::collections::BTreeSet<_> =
        files.iter().map(|file| file.name.as_str()).collect();
    ensure!(
        names.len() == files.len(),
        "duplicate file in theme package"
    );
    ensure!(
        files.iter().all(|f| safe_file(&f.name)),
        "unsafe path in theme package"
    );
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installed {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub category: String,
    #[serde(skip)]
    pub(super) lease: Option<std::sync::Arc<std::fs::File>>,
}

/// Install on an explicit selection. Theme changes never fetch the network.
pub fn install(source: &CatalogSource, entry: &Entry, root: &Path) -> Result<Installed> {
    install_with(source, entry, root, &Http::default())
}

pub fn installed(root: &Path, qualified: &str) -> Result<Installed> {
    let (source, app) = qualified
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("theme id needs a source"))?;
    ensure!(valid_id(source) && valid_id(app), "invalid theme id");
    let directory = root.join(source).join(app);
    let _guard = lock(&directory)?;
    let file = std::fs::File::open(directory.join(INSTALLED_FILE))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_FILE_BYTES,
        "installed theme metadata is too large"
    );
    let mut package: Installed = serde_json::from_slice(&bytes)?;
    let revision = package
        .root
        .strip_prefix(&directory)
        .ok()
        .and_then(|path| path.to_str())
        .unwrap_or_default();
    ensure!(
        revision.len() == 64
            && revision.bytes().all(|c| c.is_ascii_hexdigit())
            && package.root.is_dir(),
        "installed theme has an invalid revision directory"
    );
    Manifest::parse(&toml::to_string(&package.manifest)?)?;
    package.lease = Some(lease(&package.root)?);
    Ok(package)
}

pub fn install_with(
    source: &CatalogSource,
    entry: &Entry,
    root: &Path,
    transport: &impl Transport,
) -> Result<Installed> {
    ensure!(
        entry.source == source.id && valid_id(&entry.id) && valid_id(&entry.folder),
        "catalog entry belongs to another source"
    );
    let directory = source_dir(root, source)?.join(&entry.id);
    std::fs::create_dir_all(&directory)?;
    let scratch = tempfile::Builder::new()
        .prefix("download-")
        .tempdir_in(&directory)?;
    let mut files = entry.files.clone();
    if source.kind == CatalogKind::Manifest {
        files = files::list(source, &entry.folder, transport)?;
    }
    validate_files(&files)?;
    let mut total = 0;
    let mut hash = Sha256::new();
    for file in files
        .iter()
        .filter(|f| f.kind != "dir" && !f.name.to_ascii_lowercase().contains("screenshot"))
    {
        let bytes = transport.get(
            &source
                .files_url
                .replace("{app}", &entry.folder)
                .replace("{file}", &file.name),
            MAX_FILE_BYTES,
        )?;
        verify(file, &bytes)?;
        total += bytes.len();
        ensure!(
            total <= MAX_PACKAGE_BYTES,
            "theme package exceeds {MAX_PACKAGE_BYTES} bytes"
        );
        hash.update(file.name.as_bytes());
        hash.update(&bytes);
        let path = if source.kind == CatalogKind::Manifest {
            scratch.path().join(&entry.folder).join(&file.name)
        } else {
            scratch.path().join(&file.name)
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
        if let Ok(mode) = u32::from_str_radix(&file.mode, 8) {
            std::fs::set_permissions(
                &path,
                std::fs::Permissions::from_mode(if mode & 0o111 != 0 { 0o755 } else { 0o644 }),
            )?;
        }
    }
    let mut manifest = match &entry.manifest {
        Some(manifest) => manifest.clone(),
        None => Manifest::parse(
            &std::fs::read_to_string(scratch.path().join(MANIFEST_FILE))
                .context("theme package has no manifest")?,
        )?,
    };
    super::compat::adapt(&entry.id, &mut manifest);
    ensure!(
        !manifest.templates.is_empty(),
        "theme package has no entries"
    );
    if manifest
        .templates
        .values()
        .any(|template| template.post_action == "firefox-theme")
    {
        // Browser CSS is imported with the selected package, never fetched
        // by a palette change or a connected extension.
        let directory = scratch.path().join(".mandara/firefox");
        std::fs::create_dir_all(&directory)?;
        for (name, url) in [
            (
                "userChrome.css",
                "https://raw.githubusercontent.com/noctalia-dev/noctalia/main/assets/firefox_theme/css/userChrome.css",
            ),
            (
                "userContent.css",
                "https://raw.githubusercontent.com/noctalia-dev/noctalia/main/assets/firefox_theme/css/userContent.css",
            ),
            (
                "LICENSE",
                "https://raw.githubusercontent.com/noctalia-dev/noctalia/main/LICENSE",
            ),
        ] {
            let bytes = transport.get(url, MAX_FILE_BYTES)?;
            total += bytes.len();
            ensure!(
                total <= MAX_PACKAGE_BYTES,
                "theme package exceeds {MAX_PACKAGE_BYTES} bytes"
            );
            hash.update(name.as_bytes());
            hash.update(&bytes);
            std::fs::write(directory.join(name), bytes)?;
        }
    }
    hash.update(serde_json::to_vec(&manifest)?);
    let revision = format!("{:x}", hash.finalize());
    let destination = directory.join(&revision);
    let _guard = lock(&directory)?;
    if !destination.exists() {
        std::fs::rename(scratch.path(), &destination)?;
    }
    let installed = Installed {
        lease: Some(lease(&destination)?),
        root: destination,
        manifest,
        category: entry.category.clone(),
    };
    mandara_config::store::write_atomic(
        &directory.join(INSTALLED_FILE),
        &serde_json::to_vec(&installed)?,
    )?;
    Ok(installed)
}

fn lock(directory: &Path) -> Result<std::fs::File> {
    std::fs::create_dir_all(directory)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".catalog.lock"))?;
    file.lock()?;
    Ok(file)
}

fn lease(directory: &Path) -> Result<std::sync::Arc<std::fs::File>> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(".lease"))?;
    file.lock_shared()?;
    Ok(std::sync::Arc::new(file))
}

/// Keep the current package, any running job and the revisions needed by undo.
pub fn prune(root: &Path, qualified: &str, protected: &[PathBuf]) -> Result<()> {
    let current = installed(root, qualified)?;
    let directory = current
        .root
        .parent()
        .ok_or_else(|| anyhow::anyhow!("package has no parent"))?;
    let _guard = lock(directory)?;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !entry.file_type()?.is_dir()
            || name.len() != 64
            || !name.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            continue;
        }
        let path = entry.path();
        if path == current.root || protected.iter().any(|base| base.starts_with(&path)) {
            continue;
        }
        let in_use = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.join(".lease"))?;
        match in_use.try_lock() {
            Ok(()) => std::fs::remove_dir_all(path)?,
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn verify(file: &File, bytes: &[u8]) -> Result<()> {
    if !file.md5.is_empty() {
        ensure!(
            format!("{:x}", md5::Md5::digest(bytes)).eq_ignore_ascii_case(&file.md5),
            "{} changed since the catalog was loaded; refresh it and try again",
            file.name
        );
    }
    if !file.sha.is_empty() {
        let mut hash = sha1::Sha1::new();
        hash.update(format!("blob {}\0", bytes.len()).as_bytes());
        hash.update(bytes);
        ensure!(
            format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&file.sha),
            "{} changed since the catalog was loaded; refresh it and try again",
            file.name
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fixture {
        calls: Cell<usize>,
    }
    impl Transport for Fixture {
        fn get(&self, url: &str, _: u64) -> Result<Vec<u8>> {
            self.calls.set(self.calls.get() + 1);
            Ok(match url {
                "https://example.invalid/index" => br#"[{"name":"editor","category":"editor","files":[{"name":"template.toml"},{"name":"theme.css"}]}]"#.to_vec(),
                "https://example.invalid/editor/template.toml" => {
                    b"[catalog.editor]\nname = 'Editor'\n[templates.editor]\ninput_path = 'theme.css'\noutput_path = '~/editor/theme.css'\n".to_vec()
                }
                "https://example.invalid/editor/theme.css" => b"color: {{ colors.primary.default.hex }};".to_vec(),
                _ => anyhow::bail!("unexpected request {url}"),
            })
        }
    }
    pub(super) fn source() -> CatalogSource {
        CatalogSource {
            id: "demo".into(),
            name: "Demo".into(),
            kind: CatalogKind::Index,
            index_url: "https://example.invalid/index".into(),
            files_url: "https://example.invalid/{app}/{file}".into(),
            directory_url: String::new(),
        }
    }

    #[test]
    fn discovery_is_cached_and_installation_is_explicit_and_immutable() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture {
            calls: Cell::new(0),
        };
        let apps = list_with(&source(), dir.path(), false, &fixture).unwrap();
        assert_eq!(fixture.calls.get(), 1);
        assert!(!dir.path().join("demo/editor").exists());
        list_with(&source(), dir.path(), false, &fixture).unwrap();
        assert_eq!(fixture.calls.get(), 1);
        let first = install_with(&source(), &apps[0], dir.path(), &fixture).unwrap();
        assert!(first.root.join("theme.css").is_file());
        assert_eq!(
            installed(dir.path(), "demo:editor").unwrap().root,
            first.root
        );
        let second = install_with(&source(), &apps[0], dir.path(), &fixture).unwrap();
        assert_eq!(second.root, first.root);
    }

    #[test]
    fn catalog_cache_tracks_fetch_settings_without_refetching_a_display_name_change() {
        let source = source();
        let mut renamed = source.clone();
        renamed.name = "Renamed".into();
        assert!(same_source(&source, &renamed));
        renamed.files_url.push_str("?changed");
        assert!(!same_source(&source, &renamed));
    }

    #[test]
    fn pruning_keeps_active_and_undo_revisions_until_their_owners_release_them() {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Fixture {
            calls: Cell::new(0),
        };
        let apps = list_with(&source(), dir.path(), false, &fixture).unwrap();
        let current = install_with(&source(), &apps[0], dir.path(), &fixture).unwrap();
        let old = current.root.parent().unwrap().join("a".repeat(64));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("undo.sh"), "saved undo").unwrap();
        let active = lease(&old).unwrap();
        prune(dir.path(), "demo:editor", &[]).unwrap();
        assert!(old.exists());
        drop(active);
        prune(dir.path(), "demo:editor", std::slice::from_ref(&old)).unwrap();
        assert!(old.exists());
        prune(dir.path(), "demo:editor", &[]).unwrap();
        assert!(!old.exists() && current.root.exists());
    }

    #[test]
    fn advertised_checksums_detect_changed_files_before_installation() {
        let mut file = File {
            name: "theme.css".into(),
            md5: "5d41402abc4b2a76b9719d911017c592".into(),
            sha: "b6fc4c620b67d95f953a5c1c1230aaab5db5a1b0".into(),
            ..Default::default()
        };
        verify(&file, b"hello").unwrap();
        assert!(verify(&file, b"different").is_err());
        file.md5.clear();
        assert!(verify(&file, b"different").is_err());
    }

    #[test]
    fn remote_paths_cannot_escape_the_package_directory() {
        for path in ["../outside", "/absolute", "a/../../outside", ""] {
            assert!(!safe_file(path));
        }
        assert!(safe_file("theme/colors.css"));
        let bad = br#"[{"name":"editor","files":[{"name":"../../outside"}]}]"#;
        assert!(decode(&source(), bad).is_err());
    }
}
