//! Normalized CLS embeddings and a versioned, catalog-keyed vector cache.

use super::{
    Match,
    catalog::{Command, TOP_K},
    models,
};
use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
};
use tokenizers::Tokenizer;
use tract_onnx::prelude::*;

/// Arctic Embed XS's published embedding width.
const DIM: usize = 384;
/// Bound tokenization and quadratic attention for one command description.
const MAX_TOKENS: usize = 128;
/// Cache format and embedding recipe version.
const MAGIC: &[u8; 8] = b"MDRAG002";

pub struct Embedder {
    tokenizer: Tokenizer,
    state: TypedSimpleState,
    inputs: usize,
}

impl Embedder {
    pub fn load() -> Result<Self> {
        let mut tokenizer = Tokenizer::from_file(models::ROUTER.file("tokenizer.json")).map_err(anyhow::Error::msg)?;
        tokenizer.with_truncation(Some(tokenizers::TruncationParams { max_length: MAX_TOKENS, ..Default::default() })).map_err(anyhow::Error::msg)?;
        let model = tract_onnx::onnx().model_for_path(models::ROUTER.file("model.onnx"))?;
        let plan = model.into_optimized()?.into_runnable()?;
        let inputs = plan.model().input_outlets()?.len();
        Ok(Self { tokenizer, state: plan.spawn()?, inputs })
    }

    pub fn embed(&mut self, text: &str, query: bool) -> Result<Vec<f32>> {
        let input = if query { format!("Represent this sentence for searching relevant passages: {text}") } else { text.to_owned() };
        let encoded = self.tokenizer.encode(input, true).map_err(anyhow::Error::msg)?;
        let len = encoded.len().min(MAX_TOKENS);
        let ids = tract_ndarray::Array2::from_shape_vec((1, len), encoded.get_ids()[..len].iter().map(|&v| i64::from(v)).collect())?.into_tensor();
        let mask = tract_ndarray::Array2::<i64>::ones((1, len)).into_tensor();
        let types = tract_ndarray::Array2::<i64>::zeros((1, len)).into_tensor();
        let inputs = if self.inputs == 3 { tvec![ids.into(), mask.into(), types.into()] } else { tvec![ids.into(), mask.into()] };
        let output = self.state.run(inputs)?;
        let values = output[0].to_plain_array_view::<f32>()?;
        ensure!(values.len() >= DIM, "invalid embedding dimensions");
        let mut cls: Vec<f32> = values.iter().take(DIM).copied().collect();
        normalize(&mut cls)?;
        Ok(cls)
    }
}

fn normalize(vector: &mut [f32]) -> Result<()> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    ensure!(norm.is_finite() && norm > 0.0, "invalid embedding");
    for value in vector {
        *value /= norm;
    }
    Ok(())
}

fn identity(commands: &[Command]) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(MAGIC);
    hash.update(models::ROUTER.files[0].sha256);
    hash.update(serde_json::to_vec(commands)?);
    Ok(hash.finalize().into())
}

fn read_cache(path: &Path, key: &[u8; 32], count: usize) -> Result<Vec<Vec<f32>>> {
    let mut file = std::fs::File::open(path)?;
    let expected = MAGIC.len() + key.len() + count * DIM * size_of::<f32>();
    ensure!(file.metadata()?.len() == expected as u64, "stale vector cache");
    let mut magic = [0; 8];
    let mut found = [0; 32];
    file.read_exact(&mut magic)?;
    file.read_exact(&mut found)?;
    ensure!(&magic == MAGIC && &found == key, "stale vector cache");
    let mut vectors = Vec::with_capacity(count);
    for _ in 0..count {
        let mut bytes = [0; DIM * size_of::<f32>()];
        file.read_exact(&mut bytes)?;
        let mut row: Vec<f32> = bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
        normalize(&mut row)?;
        vectors.push(row);
    }
    Ok(vectors)
}

pub fn index(embedder: &mut Embedder, commands: &[Command]) -> Result<Vec<Vec<f32>>> {
    let key = identity(commands)?;
    let dir = mandara_config::paths::cache_dir().join("agent");
    let path = dir.join("mandara-commands-embeddings.bin");
    if let Ok(vectors) = read_cache(&path, &key, commands.len()) {
        return Ok(vectors);
    }
    std::fs::create_dir_all(&dir)?;
    let lock = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join("index.lock"))?;
    lock.try_lock()?;
    if let Ok(vectors) = read_cache(&path, &key, commands.len()) {
        return Ok(vectors);
    }
    let mut vectors = Vec::with_capacity(commands.len());
    for command in commands {
        let mut text = format!("{}: {}", command.path, command.summary);
        for arg in &command.args {
            text.push_str(&format!(" {} {} {}", arg.name, arg.description, arg.choices.join(" ")));
            for value in &arg.values {
                text.push(' ');
                text.push_str(&value.to_string());
            }
        }
        for example in &command.examples {
            text.push(' ');
            text.push_str(example);
        }
        vectors.push(embedder.embed(&text, false)?);
    }
    std::fs::create_dir_all(&dir)?;
    let part = dir.join("mandara-commands-embeddings.bin.part");
    let mut tmp = std::fs::File::create(&part)?;
    tmp.write_all(MAGIC)?;
    tmp.write_all(&key)?;
    for row in &vectors {
        for v in row {
            tmp.write_all(&v.to_le_bytes())?;
        }
    }
    tmp.sync_all()?;
    drop(tmp);
    std::fs::rename(part, path)?;
    Ok(vectors)
}

pub fn retrieve(query: &[f32], commands: &[Command], vectors: &[Vec<f32>]) -> Vec<Match> {
    let mut best: Vec<(usize, f32)> = Vec::with_capacity(TOP_K + 1);
    for (i, row) in vectors.iter().enumerate() {
        let score = query.iter().zip(row).map(|(a, b)| a * b).sum::<f32>();
        let at = best.partition_point(|(_, old)| *old >= score);
        if at < TOP_K {
            best.insert(at, (i, score));
            best.truncate(TOP_K);
        }
    }
    best.into_iter().map(|(i, score)| Match { command: commands[i].clone(), score }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vector_cache_rejects_stale_corrupt_and_truncated_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index");
        let key = [1; 32];
        let mut data = MAGIC.to_vec();
        data.extend(key);
        data.extend((0..DIM).flat_map(|i| (if i == 0 { 1.0_f32 } else { 0.0_f32 }).to_le_bytes()));
        std::fs::write(&path, &data).unwrap();
        assert_eq!(read_cache(&path, &key, 1).unwrap()[0][0], 1.0);
        assert!(read_cache(&path, &[2; 32], 1).is_err());
        std::fs::write(&path, &data[..data.len() - 1]).unwrap();
        assert!(read_cache(&path, &key, 1).is_err());
        assert!(normalize(&mut [f32::NAN]).is_err());
    }
    #[test]
    fn cosine_search_is_bounded_and_orders_best_first() {
        let commands: Vec<Command> = (0..20).map(|i| Command { path: i.to_string(), summary: String::new(), args: vec![], examples: vec![] }).collect();
        let vectors: Vec<_> = (0..20).map(|i| vec![i as f32]).collect();
        let matches = retrieve(&[1.0], &commands, &vectors);
        assert_eq!(matches.len(), TOP_K);
        assert_eq!(matches[0].command.path, "19");
    }

    #[test]
    #[ignore = "requires the pinned router files in a private XDG data directory"]
    fn pinned_router_embeds_and_retrieves_real_command_descriptions() {
        let mut embedder = Embedder::load().unwrap();
        let commands = vec![
            Command { path: "theme mode".into(), summary: "Set light or dark appearance".into(), args: vec![], examples: vec![] },
            Command { path: "audio volume set".into(), summary: "Set output sound volume".into(), args: vec![], examples: vec![] },
        ];
        let vectors = index(&mut embedder, &commands).unwrap();
        let start = std::time::Instant::now();
        let query = embedder.embed("switch to dark mode", true).unwrap();
        let matches = retrieve(&query, &commands, &vectors);
        eprintln!("Arctic retrieval {:.1} ms", start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(matches[0].command.path, "theme mode");
        assert_eq!(query.len(), DIM);
    }
}
