//! Read a released manifest without loading or unpacking its WebAssembly component.

use super::{MANIFEST_BYTES, Metadata, budget};
use crate::marketplace;
use std::io::Read;

/// Repositories need not keep plugin.toml at their root. Read its archive
/// entry as a stream, stopping as soon as the manifest is found.
pub(super) fn from_archive(repo: &str, url: &str) -> Result<Metadata, String> {
    budget::take()?;
    let mut response =
        marketplace::response_timeout(url, "application/octet-stream", budget::TIMEOUT).map_err(String::from)?;
    let reader = response
        .body_mut()
        .with_config()
        .limit(marketplace::MAX_BYTES)
        .reader();
    let bytes = extract(reader, marketplace::MAX_BYTES).map_err(|error| error.to_string())?;
    parse(repo, &bytes)
}

fn extract(reader: impl Read, expanded_limit: u64) -> std::io::Result<Vec<u8>> {
    use std::path::Component;
    let decoded = flate2::read::GzDecoder::new(reader).take(expanded_limit);
    let mut archive = tar::Archive::new(decoded);
    let mut root = None;
    let mut wrapped: Option<(std::path::PathBuf, Vec<u8>)> = None;
    let mut ambiguous = false;
    for entry in archive.entries()? {
        let entry = entry?;
        if !entry.header().entry_type().is_file() { continue; }
        let path = entry.path()?.into_owned();
        if path.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let mut parts = path.components().filter(|c| *c != Component::CurDir);
        let is_root = match (parts.next(), parts.next(), parts.next()) {
            (Some(Component::Normal(name)), None, None) if name == "plugin.toml" => true,
            (Some(Component::Normal(_)), Some(Component::Normal(name)), None) if name == "plugin.toml" => false,
            _ => continue,
        };
        let expected = entry.header().size()?;
        if expected > MANIFEST_BYTES {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "plugin manifest exceeds the store limit"));
        }
        let mut bytes = Vec::new();
        entry.take(MANIFEST_BYTES).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != expected { return Err(std::io::ErrorKind::UnexpectedEof.into()); }
        if is_root { root = Some(bytes); }
        else if !ambiguous {
            if wrapped.as_ref().is_some_and(|(known, _)| *known != path) {
                ambiguous = true;
                wrapped = None;
            } else { wrapped = Some((path, bytes)); }
        }
    }
    root.or_else(|| wrapped.map(|(_, bytes)| bytes)).ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::InvalidData, "archive has no unambiguous plugin manifest within the store limit"))
}

pub(super) fn fetch(api: &str, repo: &str, tag: &str) -> Result<Metadata, String> {
    let mut url = url::Url::parse(&format!("{api}/repos/{repo}/contents/plugin.toml"))
        .map_err(|error| error.to_string())?;
    url.query_pairs_mut().append_pair("ref", tag);
    budget::take()?;
    let bytes = marketplace::get_media_timeout(
        url.as_str(),
        MANIFEST_BYTES,
        "application/vnd.github.raw+json",
        budget::TIMEOUT,
    )
    .map_err(String::from)?;
    parse(repo, &bytes)
}

fn parse(repo: &str, bytes: &[u8]) -> Result<Metadata, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let metadata: Metadata = toml::from_str(text).map_err(|error| error.to_string())?;
    // Bound layout work too: a valid TOML file need not contain useful card text.
    const NAME_BYTES: usize = 512;
    const DESCRIPTION_BYTES: usize = 4096;
    let valid_text = |name: &str, description: &str| {
        name.len() <= NAME_BYTES && description.len() <= DESCRIPTION_BYTES
    };
    if !marketplace::valid_id(&metadata.id)
        || metadata.name.trim().is_empty()
        || !valid_text(&metadata.name, &metadata.description)
        || !metadata
            .i18n
            .values()
            .all(|t| valid_text(&t.name, &t.description))
    {
        return Err(format!("invalid store metadata for {repo}"));
    }
    Ok(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn archive_metadata_needs_no_root_source_file_or_extracted_component() {
        let bytes = archive(&[
            (
                "picker/plugin.toml",
                b"id='picker'\nname='Real Picker'\ndescription='Published description'\n",
            ),
            ("picker/plugin.wasm", b"unused"),
        ]);
        let metadata = parse(
            "author/picker",
            &extract(bytes.as_slice(), marketplace::MAX_BYTES).unwrap(),
        )
        .unwrap();
        assert_eq!(
            (
                metadata.id.as_str(),
                metadata.name.as_str(),
                metadata.description.as_str()
            ),
            ("picker", "Real Picker", "Published description")
        );
    }

    #[test]
    fn root_manifest_wins_over_examples_and_ambiguous_wrappers_are_rejected() {
        let example = b"id='example'\nname='Example'";
        let real = b"id='real'\nname='Real plugin'";
        let bytes = archive(&[("examples/plugin.toml", example), ("plugin.toml", real)]);
        assert_eq!(parse("author/real", &extract(bytes.as_slice(), marketplace::MAX_BYTES).unwrap()).unwrap().id, "real");
        let bytes = archive(&[("examples/plugin.toml", example), ("real/plugin.toml", real)]);
        assert!(extract(bytes.as_slice(), marketplace::MAX_BYTES).is_err());
        let bytes = archive(&[("plugin.toml", example), ("plugin.toml", real)]);
        assert_eq!(extract(bytes.as_slice(), marketplace::MAX_BYTES).unwrap(), real);
    }

    #[test]
    fn archive_expansion_and_manifest_size_are_bounded() {
        let large = vec![b'x'; MANIFEST_BYTES as usize + 1];
        let bytes = archive(&[("plugin.toml", &large)]);
        assert!(extract(bytes.as_slice(), marketplace::MAX_BYTES).is_err());
        let bytes = archive(&[
            ("plugin.wasm", &large),
            ("plugin.toml", b"id='picker'\nname='Picker'"),
        ]);
        assert!(
            extract(bytes.as_slice(), 1024).is_err(),
            "do not expand a large component while looking for metadata"
        );
        assert!(parse("author/picker", b"id='../picker'\nname='Picker'").is_err());
    }
}
