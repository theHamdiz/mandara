//! Follow package subdirectories without allowing unbounded catalog traversal.

use super::{CatalogSource, File, MAX_CATALOG_BYTES, MAX_FILES, Transport, validate_files};
use anyhow::{Result, ensure};
use std::collections::VecDeque;

/// Theme packages use a few asset folders; deeper trees are not app templates.
const MAX_DEPTH: usize = 8;

pub(super) fn list(source: &CatalogSource, app: &str, transport: &impl Transport) -> Result<Vec<File>> {
    let root = url::Url::parse(&source.directory_url.replace("{app}", app))?;
    let mut pending = VecDeque::from([(String::new(), root, 0)]);
    let mut result = Vec::new();
    let mut entries = 0;
    while let Some((prefix, url, depth)) = pending.pop_front() {
        let files: Vec<File> = serde_json::from_slice(&transport.get(url.as_str(), MAX_CATALOG_BYTES)?)?;
        validate_files(&files)?;
        entries += files.len();
        ensure!(entries <= MAX_FILES, "too many package files or directories");
        for mut file in files {
            if file.name.to_ascii_lowercase().contains("screenshot") { continue; }
            let name = if prefix.is_empty() { file.name.clone() } else { format!("{prefix}/{}",file.name) };
            if file.kind == "dir" {
                ensure!(depth < MAX_DEPTH, "theme package has too many nested directories");
                let mut child = url.clone();
                {
                    let mut segments = child.path_segments_mut().map_err(|_| anyhow::anyhow!("catalog directory URL has no path"))?;
                    segments.pop_if_empty();
                    for segment in file.name.split('/') { segments.push(segment); }
                }
                pending.push_back((name, child, depth+1));
            } else {
                ensure!(file.kind.is_empty() || file.kind == "file", "unsupported package entry type: {}",file.kind);
                file.name = name;
                result.push(file);
            }
        }
    }
    validate_files(&result)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture;
    impl Transport for Fixture {
        fn get(&self, url: &str, _:u64) -> Result<Vec<u8>> {
            Ok(match url {
                "https://example.invalid/app?ref=stable" => br#"[{"name":"nested","type":"dir"},{"name":"theme.css","type":"file"}]"#.to_vec(),
                "https://example.invalid/app/nested?ref=stable" => br#"[{"name":"reload.sh","type":"file","sha":"hash"}]"#.to_vec(),
                _ => anyhow::bail!("unexpected directory URL: {url}"),
            })
        }
    }
    #[test]
    fn nested_assets_keep_relative_paths_and_revision_query() {
        let source = CatalogSource { directory_url:"https://example.invalid/{app}?ref=stable".into(),..super::super::tests::source() };
        let files = list(&source,"app",&Fixture).unwrap();
        assert_eq!(files.iter().map(|file|file.name.as_str()).collect::<Vec<_>>(),["theme.css","nested/reload.sh"]);
        assert_eq!(files[1].sha,"hash");
    }
    struct Deep;
    impl Transport for Deep {
        fn get(&self,_:&str,_:u64)->Result<Vec<u8>> { Ok(br#"[{"name":"deeper","type":"dir"}]"#.to_vec()) }
    }
    #[test]
    fn recursive_directory_lists_stop_at_the_depth_limit() {
        let source = CatalogSource { directory_url:"https://example.invalid/{app}".into(),..super::super::tests::source() };
        assert!(list(&source,"app",&Deep).unwrap_err().to_string().contains("nested"));
    }
}
