//! Downloadable model files the shell uses (OCR, the wallpaper depth
//! model), with install status and a resumable-safe downloader. Files land
//! in `$XDG_DATA_HOME/mandara/models/<id>/` and are written to `*.part`
//! first, then checked against their size and, where known, their SHA-256,
//! so an interrupted or tampered download never leaves a bad model in place.
//!
//! Every model keeps its own licence (all Apache-2.0 today; see
//! COPYRIGHT.md). None ships inside the binary.

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
    /// Expected SHA-256 (lowercase hex) checked after download; empty for
    /// files without a pinned upstream revision.
    pub sha256: &'static str,
}

pub struct Model {
    pub id: &'static str,
    pub title: &'static str,
    pub purpose: &'static str,
    pub license: &'static str,
    /// Where it comes from, for display (the project page).
    pub source: &'static str,
    /// Icon name in Mandara's icon set.
    pub icon: &'static str,
    /// Downloaded only when its feature asks for it (the depth model):
    /// left out of "download all" and of Maintenance's missing-model count.
    pub on_demand: bool,
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
        source: HF,
        icon: "text-scan",
        on_demand: false,
        files: &[
            ModelFile { name: "inference.onnx", url: hf!("PP-OCRv5_mobile_det_onnx", "inference.onnx"), size: 4_826_518, sha256: "" },
            ModelFile { name: "inference.yml", url: hf!("PP-OCRv5_mobile_det_onnx", "inference.yml"), size: 0, sha256: "" },
        ],
    },
    Model {
        id: "ocr-latin",
        title: tk("Latin text recognition"),
        purpose: tk("Reads English, Spanish, French and other Latin-script text"),
        license: "Apache-2.0",
        source: HF,
        icon: "text-scan",
        on_demand: false,
        files: &[
            ModelFile { name: "inference.onnx", url: hf!("latin_PP-OCRv5_mobile_rec_onnx", "inference.onnx"), size: 8_042_023, sha256: "" },
            ModelFile { name: "inference.yml", url: hf!("latin_PP-OCRv5_mobile_rec_onnx", "inference.yml"), size: 0, sha256: "" },
        ],
    },
    Model {
        id: "ocr-arabic",
        title: tk("Arabic text recognition"),
        purpose: tk("Reads Arabic (and Persian / Urdu) text"),
        license: "Apache-2.0",
        source: HF,
        icon: "text-scan",
        on_demand: false,
        files: &[
            ModelFile { name: "inference.onnx", url: hf!("arabic_PP-OCRv5_mobile_rec_onnx", "inference.onnx"), size: 7_998_947, sha256: "" },
            ModelFile { name: "inference.yml", url: hf!("arabic_PP-OCRv5_mobile_rec_onnx", "inference.yml"), size: 0, sha256: "" },
        ],
    },
    Model {
        id: crate::depth_model::MODEL_ID,
        title: tk("Wallpaper depth"),
        purpose: tk("Finds what is near and far in a wallpaper for the depth clock (Depth Anything V2 Small)"),
        license: "Apache-2.0",
        source: crate::depth_model::SOURCE,
        icon: "image",
        on_demand: true,
        files: &[ModelFile { name: crate::depth_model::MODEL_FILE, url: crate::depth_model::URL, size: crate::depth_model::SIZE, sha256: crate::depth_model::SHA256 }],
    },
];

/// Source of the OCR models, for display.
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

    /// Where file `name` of this model lives.
    pub fn file(&self, name: &str) -> PathBuf {
        self.dir().join(name)
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
            let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
            loop {
                let n = reader.read(&mut buf).map_err(|e| tf("Downloading {file}: {error}", &[("file", &f.name), ("error", &e)]))?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
                sha2::Digest::update(&mut hasher, &buf[..n]);
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
            let digest = hex(&sha2::Digest::finalize(hasher));
            if !f.sha256.is_empty() && !digest.eq_ignore_ascii_case(f.sha256) {
                let _ = std::fs::remove_file(&part);
                return Err(tf(
                    "{model} ({file}) failed its checksum: got {got}, expected {expected}",
                    &[("model", &tr(self.title)), ("file", &f.name), ("got", &digest), ("expected", &f.sha256)],
                ));
            }
            std::fs::rename(&part, &path).map_err(|e| format!("{}: {e}", path.display()))?;
            done += f.size;
        }
        progress(total, total);
        Ok(())
    }
}

/// Lowercase hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelStatus {
    pub id: &'static str,
    pub title: &'static str,
    pub purpose: &'static str,
    pub installed: bool,
    pub size: u64,
    pub path: PathBuf,
    pub license: &'static str,
    pub on_demand: bool,
}

pub fn status() -> Vec<ModelStatus> {
    MODELS
        .iter()
        .map(|m| ModelStatus { id: m.id, title: m.title, purpose: m.purpose, installed: m.installed(), size: m.download_size(), path: m.dir(), license: m.license, on_demand: m.on_demand })
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
        // On-demand models are pinned: a size and a checksum per file.
        for m in MODELS.iter().filter(|m| m.on_demand) {
            assert!(m.files.iter().all(|f| f.sha256.len() == 64 && f.size > 0), "{}", m.id);
        }
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xab]), "000fab");
    }
}
