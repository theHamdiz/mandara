//! Pinned, separately licensed model artifacts. Downloads never run implicitly.

use super::Engine;
use crate::models::{Model, ModelFile};
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    sync::atomic::{AtomicBool, Ordering},
};

/// Fixed-size transfer buffer; one model file is streamed at a time.
const TRANSFER_BYTES: usize = 64 * 1024;
/// A stalled transfer must remain cancellable within a bounded interval.
const NETWORK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub const ROUTER: Model = Model {
    id: "agent-router",
    title: "Arctic Embed XS Int8",
    purpose: "Semantic command retrieval",
    license: "Apache-2.0",
    source: "https://huggingface.co/Snowflake/snowflake-arctic-embed-xs",
    icon: "search",
    on_demand: true,
    files: &[
        ModelFile {
            name: "model.onnx",
            url: "https://huggingface.co/Snowflake/snowflake-arctic-embed-xs/resolve/d8c86521100d3556476a063fc2342036d45c106f/onnx/model_quantized.onnx",
            size: 22_972_992,
            sha256: "e6aa5e656466a73d7c3111e9a3378bd13e5b93af30eaac2b3f13fd56692589a1",
        },
        ModelFile {
            name: "tokenizer.json",
            url: "https://huggingface.co/Snowflake/snowflake-arctic-embed-xs/resolve/d8c86521100d3556476a063fc2342036d45c106f/tokenizer.json",
            size: 711_649,
            sha256: "91f1def9b9391fdabe028cd3f3fcc4efd34e5d1f08c3bf2de513ebb5911a1854",
        },
    ],
};
pub const FUNCTION_GEMMA: Model = Model {
    id: "agent-functiongemma",
    title: "FunctionGemma 270M Q4_K_M",
    purpose: "Local function compiler",
    license: "Gemma",
    source: "https://huggingface.co/unsloth/functiongemma-270m-it-GGUF",
    icon: "zap",
    on_demand: true,
    files: &[
        ModelFile {
            name: "model.gguf",
            url: "https://huggingface.co/unsloth/functiongemma-270m-it-GGUF/resolve/1de0f7fc7cae0062012c2e91ef02e642c161aa57/functiongemma-270m-it-Q4_K_M.gguf",
            size: 253_127_904,
            sha256: "7474135cf63b5de86bd29d2feb92c644d48e9df5e1ac31550662af373c67d0fc",
        },
        ModelFile {
            name: "tokenizer.json",
            url: "https://huggingface.co/unsloth/functiongemma-270m-it/resolve/79eaaa84125b897f5a0ad6638362b864fec813ad/tokenizer.json",
            size: 33_384_899,
            sha256: "b6b09a0b4a803ad453063ca4bb49a784540e8120004e2450e025df2b27d41fb2",
        },
    ],
};
pub const QWEN: Model = Model {
    id: "agent-qwen",
    title: "Qwen3 0.6B Q4_K_M",
    purpose: "Multilingual and compound commands",
    license: "Apache-2.0",
    source: "https://huggingface.co/unsloth/Qwen3-0.6B-GGUF",
    icon: "globe",
    on_demand: true,
    files: &[
        ModelFile {
            name: "model.gguf",
            url: "https://huggingface.co/unsloth/Qwen3-0.6B-GGUF/resolve/50968a4468ef4233ed78cd7c3de230dd1d61a56b/Qwen3-0.6B-Q4_K_M.gguf",
            size: 396_705_472,
            sha256: "ac2d97712095a558e31573f62f466a3f9d93990898b0ec79d7c974c1780d524a",
        },
        ModelFile {
            name: "tokenizer.json",
            url: "https://huggingface.co/Qwen/Qwen3-0.6B/resolve/c1899de289a04d12100db370d81485cdf75e47ca/tokenizer.json",
            size: 11_422_654,
            sha256: "aeb13307a71acd8fe81861d94ad54ab689df773318809eed3cbe794b4492dae4",
        },
    ],
};
pub const ALL: [&Model; 3] = [&ROUTER, &FUNCTION_GEMMA, &QWEN];

pub fn compiler(engine: Engine) -> &'static Model {
    match engine {
        Engine::FunctionGemma => &FUNCTION_GEMMA,
        Engine::Qwen => &QWEN,
    }
}

fn hash_file(file: &mut File, cancel: &AtomicBool) -> Result<(u64, Sha256)> {
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buf = [0; TRANSFER_BYTES];
    loop {
        ensure!(!cancel.load(Ordering::Acquire), "download paused");
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok((bytes, hash))
}

/// Verify cached files before inference too; size alone cannot detect damage.
pub fn verified(model: &Model, cancel: &AtomicBool) -> Result<()> {
    for expected in model.files {
        let (bytes, hash) = hash_file(&mut File::open(model.file(expected.name))?, cancel)?;
        ensure!(bytes == expected.size && crate::models::hex(&hash.finalize()) == expected.sha256, "{} failed verification", expected.name);
    }
    Ok(())
}

/// Resume a partial file only when the pinned server confirms its exact byte
/// range. Pausing retains partial bytes; only verified files become installed.
pub fn download(model: &Model, cancel: &AtomicBool, progress: impl Fn(u64, u64)) -> Result<()> {
    crate::offline::check().map_err(anyhow::Error::msg)?;
    download_into(model, &model.dir(), cancel, progress)
}

fn download_into(model: &Model, dir: &std::path::Path, cancel: &AtomicBool, progress: impl Fn(u64, u64)) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let lock = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join("download.lock"))?;
    lock.try_lock()?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_resolve(Some(NETWORK_TIMEOUT))
        .timeout_connect(Some(NETWORK_TIMEOUT))
        .timeout_recv_body(Some(NETWORK_TIMEOUT))
        .build()
        .into();
    let mut done = 0;
    for f in model.files {
        ensure!(!cancel.load(Ordering::Acquire), "download paused");
        let path = dir.join(f.name);
        if let Ok(mut existing) = File::open(&path) {
            let (bytes, hash) = hash_file(&mut existing, cancel)?;
            if bytes == f.size && crate::models::hex(&hash.finalize()) == f.sha256 {
                done += bytes;
                progress(done, model.download_size());
                continue;
            }
        }
        let part = dir.join(format!("{}.part", f.name));
        let mut out = OpenOptions::new().create(true).truncate(false).read(true).append(true).open(&part)?;
        let (mut got, mut hash) = hash_file(&mut out, cancel)?;
        if got >= f.size {
            out.set_len(0)?;
            got = 0;
            hash = Sha256::new();
        }
        let mut response = agent.get(f.url).header("Range", &format!("bytes={got}-")).call()?;
        if got > 0 && response.status().as_u16() != 206 {
            out.set_len(0)?;
            got = 0;
            hash = Sha256::new();
        }
        if response.status().as_u16() == 206 {
            ensure!(
                response.headers().get("content-range").and_then(|h| h.to_str().ok()) == Some(format!("bytes {got}-{}/{}", f.size - 1, f.size).as_str()),
                "invalid download range"
            );
        }
        let mut reader = response.body_mut().with_config().limit(f.size + 1).reader();
        let mut buf = [0; TRANSFER_BYTES];
        loop {
            ensure!(!cancel.load(Ordering::Acquire), "download paused");
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            ensure!(got + n as u64 <= f.size, "oversized model download");
            out.write_all(&buf[..n])?;
            hash.update(&buf[..n]);
            got += n as u64;
            progress(done + got, model.download_size());
        }
        if got != f.size || crate::models::hex(&hash.finalize()) != f.sha256 {
            drop(out);
            std::fs::remove_file(&part)?;
            anyhow::bail!("{} failed verification", f.name);
        }
        out.sync_all()?;
        drop(out);
        std::fs::rename(part, path)?;
        done += f.size;
    }
    progress(done, model.download_size());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artifacts_are_pinned_and_sizes_include_tokenizers() {
        for model in ALL {
            assert!(model.on_demand);
            assert!(model.files.iter().all(|f| f.size > 0 && f.sha256.len() == 64 && f.url.contains("/resolve/") && !f.url.contains("/main/")));
            assert!(model.download_size() > model.files[0].size);
        }
        assert_eq!(compiler(Engine::Qwen).id, QWEN.id);
    }
    #[test]
    fn verification_reads_all_bytes_and_honors_cancellation() {
        let mut file = tempfile::tempfile().unwrap();
        let bytes = vec![7; TRANSFER_BYTES * 2 + 1];
        file.write_all(&bytes).unwrap();
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).unwrap();
        let cancel = AtomicBool::new(false);
        let (n, hash) = hash_file(&mut file, &cancel).unwrap();
        assert_eq!(n, bytes.len() as u64);
        assert_eq!(hash.finalize()[..], Sha256::digest(&bytes)[..]);
        cancel.store(true, Ordering::Release);
        assert!(hash_file(&mut file, &cancel).is_err());
    }
}

#[cfg(test)]
mod transfer_tests {
    use super::*;
    use std::net::TcpListener;
    #[test]
    fn resumed_downloads_validate_the_range_and_replace_corrupt_installs() {
        for (status, valid_hash) in [(206, true), (200, true), (206, false)] {
            let dir = tempfile::tempdir().unwrap();
            let content = b"verified model bytes";
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = Box::leak(format!("http://{}/model", listener.local_addr().unwrap()).into_boxed_str());
            let hash = Box::leak(crate::models::hex(&Sha256::digest(content)).into_boxed_str());
            let files = Box::leak(Box::new([ModelFile {
                name: "model",
                url,
                size: content.len() as u64,
                sha256: if valid_hash { hash } else { "0000000000000000000000000000000000000000000000000000000000000000" },
            }]));
            let model =
                Model { id: "fixture", title: "fixture", purpose: "fixture", license: "fixture", source: "fixture", icon: "search", on_demand: true, files };
            std::fs::write(dir.path().join("model.part"), &content[..3]).unwrap();
            std::fs::write(dir.path().join("model"), vec![0; content.len()]).unwrap();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    assert!(request.len() < 4096);
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                assert!(String::from_utf8(request).unwrap().to_ascii_lowercase().contains("range: bytes=3-"));
                let body = if status == 206 { &content[3..] } else { content.as_slice() };
                let range = if status == 206 { format!("Content-Range: bytes 3-{}/{}\r\n", content.len() - 1, content.len()) } else { String::new() };
                write!(stream, "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\n{range}Connection: close\r\n\r\n", body.len()).unwrap();
                stream.write_all(body).unwrap();
            });
            let result = download_into(&model, dir.path(), &AtomicBool::new(false), |_, _| {});
            server.join().unwrap();
            if valid_hash {
                result.unwrap();
                assert_eq!(std::fs::read(dir.path().join("model")).unwrap(), content);
            } else {
                assert!(result.is_err());
                assert!(!dir.path().join("model.part").exists());
                assert_eq!(std::fs::read(dir.path().join("model")).unwrap(), vec![0; content.len()]);
            }
        }
    }
    #[test]
    fn a_paused_transfer_keeps_its_existing_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx.part"), b"partial").unwrap();
        assert!(download_into(&ROUTER, dir.path(), &AtomicBool::new(true), |_, _| {}).is_err());
        assert_eq!(std::fs::read(dir.path().join("model.onnx.part")).unwrap(), b"partial");
    }
}
