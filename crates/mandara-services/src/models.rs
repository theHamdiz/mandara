//! Downloadable model files the shell uses (OCR today), with install status
//! and a resumable-safe downloader. Files land in
//! `$XDG_DATA_HOME/mandara/models/<id>/` and are written to `*.part` first,
//! so an interrupted download never leaves a truncated model in place.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use mandara_i18n::{tf, tk, tr};
use serde::Serialize;

/// Connect timeout for model downloads (network safety bound).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Body timeout for model downloads (network safety bound).
const BODY_TIMEOUT: Duration = Duration::from_secs(120);

/// One file of a model.
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    /// Expected size in bytes (0 = unknown; then any non-empty file counts).
    pub size: u64,
}

pub struct Model {
    pub id: &'static str,
    pub title: &'static str,
    pub purpose: &'static str,
    pub license: &'static str,
    pub files: &'static [ModelFile],
}

const HF: &str = "https://huggingface.co/PaddlePaddle";

macro_rules! hf {
    ($repo:literal, $file:literal) => {
        concat!("https://huggingface.co/PaddlePaddle/", $repo, "/resolve/main/", $file)
    };
}

/// Every model Mandara can download.
pub const MODELS: &[Model] = &[
    Model {
        id: "ocr-det",
        title: tk("Text detection"),
        purpose: tk("Finds lines of text in screenshots (PP-OCRv5 mobile)"),
        license: "Apache-2.0",
        files: &[
            ModelFile { name: "inference.onnx", url: hf!("PP-OCRv5_mobile_det_onnx", "inference.onnx"), size: 4_826_518 },
            ModelFile { name: "inference.yml", url: hf!("PP-OCRv5_mobile_det_onnx", "inference.yml"), size: 0 },
        ],
    },
    Model {
        id: "ocr-latin",
        title: tk("Latin text recognition"),
        purpose: tk("Reads English, Spanish, French and other Latin-script text"),
        license: "Apache-2.0",
        files: &[
            ModelFile { name: "inference.onnx", url: hf!("latin_PP-OCRv5_mobile_rec_onnx", "inference.onnx"), size: 8_042_023 },
            ModelFile { name: "inference.yml", url: hf!("latin_PP-OCRv5_mobile_rec_onnx", "inference.yml"), size: 0 },
        ],
    },
    Model {
        id: "ocr-arabic",
        title: tk("Arabic text recognition"),
        purpose: tk("Reads Arabic (and Persian / Urdu) text"),
        license: "Apache-2.0",
        files: &[
            ModelFile { name: "inference.onnx", url: hf!("arabic_PP-OCRv5_mobile_rec_onnx", "inference.onnx"), size: 7_998_947 },
            ModelFile { name: "inference.yml", url: hf!("arabic_PP-OCRv5_mobile_rec_onnx", "inference.yml"), size: 0 },
        ],
    },
];

/// Source of every model, for display.
pub const SOURCE: &str = HF;

pub fn find(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn dir(id: &str) -> PathBuf {
    mandara_config::paths::data_dir().join("models").join(id)
}

impl Model {
    pub fn dir(&self) -> PathBuf {
        dir(self.id)
    }

    pub fn download_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// All files present with their expected sizes.
    pub fn installed(&self) -> bool {
        self.files.iter().all(|f| {
            std::fs::metadata(self.dir().join(f.name)).is_ok_and(|m| if f.size > 0 { m.len() == f.size } else { m.len() > 0 })
        })
    }

    pub fn remove(&self) -> Result<(), String> {
        match std::fs::remove_dir_all(self.dir()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", self.dir().display())),
        }
    }

    /// Downloads missing / damaged files. `progress(done, total)` is called
    /// as bytes arrive (total is the model's known size).
    pub fn download(&self, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
        crate::offline::check()?;
        let dir = self.dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(CONNECT_TIMEOUT)).timeout_recv_body(Some(BODY_TIMEOUT)).build().into();
        let total = self.download_size();
        let mut done = 0u64;
        for f in self.files {
            let path = dir.join(f.name);
            if std::fs::metadata(&path).is_ok_and(|m| if f.size > 0 { m.len() == f.size } else { m.len() > 0 }) {
                done += f.size;
                progress(done, total);
                continue;
            }
            let part = dir.join(format!("{}.part", f.name));
            let mut response = agent
                .get(f.url)
                .header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION")))
                .call()
                .map_err(|e| tf("Downloading {model} ({file}): {error}", &[("model", &tr(self.title)), ("file", &f.name), ("error", &e)]))?;
            let mut reader = response.body_mut().with_config().limit(256 * 1024 * 1024).reader();
            let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
            let mut buf = vec![0u8; 64 * 1024];
            let mut got = 0u64;
            loop {
                let n = reader.read(&mut buf).map_err(|e| tf("Downloading {file}: {error}", &[("file", &f.name), ("error", &e)]))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
                got += n as u64;
                progress(done + got.min(f.size.max(got)), total);
            }
            out.sync_all().ok();
            if f.size > 0 && got != f.size {
                let _ = std::fs::remove_file(&part);
                return Err(tf(
                    "{model} ({file}) is {got} bytes, expected {expected}",
                    &[("model", &tr(self.title)), ("file", &f.name), ("got", &got), ("expected", &f.size)],
                ));
            }
            std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
            done += f.size;
        }
        progress(total, total);
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelStatus {
    pub id: &'static str,
    pub title: &'static str,
    pub purpose: &'static str,
    pub installed: bool,
    pub size: u64,
    pub path: PathBuf,
}

pub fn status() -> Vec<ModelStatus> {
    MODELS
        .iter()
        .map(|m| ModelStatus { id: m.id, title: m.title, purpose: m.purpose, installed: m.installed(), size: m.download_size(), path: m.dir() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_ids_are_unique_and_urls_https() {
        let mut ids: Vec<_> = MODELS.iter().map(|m| m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), MODELS.len());
        assert!(MODELS.iter().flat_map(|m| m.files).all(|f| f.url.starts_with("https://")));
    }
}
