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
const MAGIC: &[u8; 8] = b"MDRAG003";

pub struct Embedder {
    tokenizer: Tokenizer,
    state: TypedSimpleState,
    inputs: usize,
}

impl Embedder {
    /// Load Arctic once and reuse its execution state while building the index.
    pub fn load() -> Result<Self> {
        let mut tokenizer = Tokenizer::from_file(models::ROUTER.file("tokenizer.json")).map_err(anyhow::Error::msg)?;
        tokenizer.with_truncation(Some(tokenizers::TruncationParams { max_length: MAX_TOKENS, ..Default::default() })).map_err(anyhow::Error::msg)?;
        let model = tract_onnx::onnx().model_for_path(models::ROUTER.file("model.onnx"))?;
        let plan = model.into_optimized()?.into_runnable()?;
        let inputs = plan.model().input_outlets()?.len();
        Ok(Self { tokenizer, state: plan.spawn()?, inputs })
    }

    /// Return a normalized CLS vector, with Arctic’s search prefix for queries.
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

/// Reject empty or non-finite vectors before cosine scoring.
fn normalize(vector: &mut [f32]) -> Result<()> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    ensure!(norm.is_finite() && norm > 0.0, "invalid embedding");
    for value in vector {
        *value /= norm;
    }
    Ok(())
}

/// Key vectors by the embedding recipe, pinned weights and complete command metadata.
fn identity(commands: &[Command]) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    hash.update(MAGIC);
    hash.update(models::ROUTER.files[0].sha256);
    hash.update(serde_json::to_vec(commands)?);
    Ok(hash.finalize().into())
}

/// Accept only the expected cache identity, shape and finite vector values.
/// Rows land in one contiguous matrix through a fixed stack buffer.
fn read_cache(path: &Path, key: &[u8; 32], count: usize) -> Result<Vec<f32>> {
    let mut file = std::fs::File::open(path)?;
    let expected = MAGIC.len() + key.len() + count * DIM * size_of::<f32>();
    ensure!(file.metadata()?.len() == expected as u64, "stale vector cache");
    let mut magic = [0; 8];
    let mut found = [0; 32];
    file.read_exact(&mut magic)?;
    file.read_exact(&mut found)?;
    ensure!(&magic == MAGIC && &found == key, "stale vector cache");
    let mut vectors = Vec::with_capacity(count * DIM);
    let mut bytes = [0; DIM * size_of::<f32>()];
    for _ in 0..count {
        file.read_exact(&mut bytes)?;
        let row = vectors.len();
        vectors.extend(bytes.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)));
        normalize(&mut vectors[row..])?;
    }
    Ok(vectors)
}

/// Reuse a matching index or build and atomically replace it under a file lock.
/// The result is row-major: one `DIM`-wide unit vector per command.
pub fn index(embedder: &mut Embedder, commands: &[Command]) -> Result<Vec<f32>> {
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
    let mut vectors = Vec::with_capacity(commands.len() * DIM);
    for command in commands {
        let mut text = format!("{}: {}", command.path, command.summary);
        for arg in &command.args {
            text.push_str(&format!(" {} {} {} {}", arg.name, arg.kind, arg.description, arg.choices.join(" ")));
            for value in &arg.values {
                text.push(' ');
                text.push_str(&value.to_string());
            }
        }
        for example in &command.examples {
            text.push(' ');
            text.push_str(example);
        }
        vectors.extend(embedder.embed(&text, false)?);
    }
    std::fs::create_dir_all(&dir)?;
    let part = dir.join("mandara-commands-embeddings.bin.part");
    let mut tmp = std::fs::File::create(&part)?;
    tmp.write_all(MAGIC)?;
    tmp.write_all(&key)?;
    for row in vectors.as_chunks::<DIM>().0 {
        let mut bytes = [0; DIM * size_of::<f32>()];
        for (out, v) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(row) {
            *out = v.to_le_bytes();
        }
        tmp.write_all(&bytes)?;
    }
    tmp.sync_all()?;
    drop(tmp);
    std::fs::rename(part, path)?;
    Ok(vectors)
}

/// Independent lane sums let the compiler vectorize the dot product; a single
/// running sum would serialize every addition.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    const LANES: usize = 16;
    let (a_chunks, a_rest) = a.as_chunks::<LANES>();
    let (b_chunks, b_rest) = b.as_chunks::<LANES>();
    let mut sums = [0f32; LANES];
    for (x, y) in a_chunks.iter().zip(b_chunks) {
        for lane in 0..LANES {
            sums[lane] += x[lane] * y[lane];
        }
    }
    sums.iter().sum::<f32>() + a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum::<f32>()
}

/// Keep the ten highest cosine scores without copying command metadata.
/// `vectors` is row-major with rows as wide as the query.
pub(super) fn ranking(query: &[f32], vectors: &[f32]) -> Vec<(usize, f32)> {
    let mut best: Vec<(usize, f32)> = Vec::with_capacity(TOP_K + 1);
    for (i, row) in vectors.chunks_exact(query.len()).enumerate() {
        let score = dot(query, row);
        let at = best.partition_point(|(_, old)| *old >= score);
        if at < TOP_K {
            best.insert(at, (i, score));
            best.truncate(TOP_K);
        }
    }
    best
}

/// Interleave clause rankings so every requested action gets candidate slots.
/// Deduplication and the shared top-K limit keep the compiler context bounded.
/// Only the command descriptions that will cross the worker boundary are copied.
pub fn retrieve_clauses(rankings: &[Vec<(usize, f32)>], commands: &[Command]) -> Vec<Match> {
    if let [ranked] = rankings {
        return ranked.iter().map(|&(i, score)| Match { command: commands[i].clone(), score }).collect();
    }
    let mut merged: Vec<Match> = Vec::with_capacity(TOP_K);
    for rank in 0..TOP_K {
        for group in rankings.iter().take(super::catalog::MAX_ACTIONS) {
            if let Some(&(index, score)) = group.get(rank)
                && !merged.iter().any(|m| m.command.path == commands[index].path)
            {
                merged.push(Match { command: commands[index].clone(), score });
                if merged.len() == TOP_K {
                    return merged;
                }
            }
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commands(count: usize) -> Vec<Command> {
        (0..count).map(|i| Command { path: i.to_string(), summary: String::new(), args: vec![], examples: vec![] }).collect()
    }

    /// A strong first intent must not displace every candidate for the second intent.
    #[test]
    fn compound_retrieval_keeps_both_intents_within_the_same_tool_budget() {
        let commands = commands(20);
        let vectors: Vec<f32> = (0..20).flat_map(|i| if i < 10 { [1.0, 0.0] } else { [0.0, 1.0] }).collect();
        let rankings = [ranking(&[1.0, 0.0], &vectors), ranking(&[0.0, 1.0], &vectors)];
        let merged = retrieve_clauses(&rankings, &commands);
        assert_eq!(merged.len(), TOP_K);
        assert!(merged.iter().any(|m| m.command.path == "0"));
        assert!(merged.iter().any(|m| m.command.path == "10"));
        let repeated = retrieve_clauses(&[rankings[0].clone(), rankings[0].clone()], &commands);
        assert_eq!(repeated.len(), TOP_K);
        assert_eq!(repeated.iter().map(|m| &m.command.path).collect::<std::collections::BTreeSet<_>>().len(), TOP_K);
    }

    #[test]
    /// Exercise cache recovery without loading an inference runtime.
    fn vector_cache_rejects_stale_corrupt_and_truncated_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index");
        let key = [1; 32];
        let mut data = MAGIC.to_vec();
        data.extend(key);
        for row in 0..2 {
            data.extend((0..DIM).flat_map(|i| (if i == row { 2.0_f32 } else { 0.0_f32 }).to_le_bytes()));
        }
        std::fs::write(&path, &data).unwrap();
        let vectors = read_cache(&path, &key, 2).unwrap();
        assert_eq!(vectors.len(), 2 * DIM);
        assert_eq!((vectors[0], vectors[DIM + 1]), (1.0, 1.0));
        assert!(read_cache(&path, &[2; 32], 2).is_err());
        assert!(read_cache(&path, &key, 1).is_err());
        std::fs::write(&path, &data[..data.len() - 1]).unwrap();
        assert!(read_cache(&path, &key, 2).is_err());
        assert!(normalize(&mut [f32::NAN]).is_err());
    }
    #[test]
    /// Score more entries than fit in the result set and retain the best ten.
    fn cosine_search_is_bounded_and_orders_best_first() {
        let vectors: Vec<_> = (0..20).map(|i| i as f32).collect();
        let matches = retrieve_clauses(&[ranking(&[1.0], &vectors)], &commands(20));
        assert_eq!(matches.len(), TOP_K);
        assert_eq!(matches[0].command.path, "19");
        assert!(matches.windows(2).all(|pair| pair[0].score >= pair[1].score));
    }

    /// The lane-split product agrees with a sequential sum at the real width and odd tails.
    #[test]
    fn vectorized_dot_product_matches_a_sequential_sum() {
        for width in [1, 15, 16, 17, DIM] {
            let a: Vec<f32> = (0..width).map(|i| ((i * 7 % 13) as f32 - 6.0) / 13.0).collect();
            let b: Vec<f32> = (0..width).map(|i| ((i * 5 % 11) as f32 - 5.0) / 11.0).collect();
            let sequential = a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>();
            assert!((dot(&a, &b) - sequential).abs() <= 1e-5, "width {width}");
        }
    }

    #[test]
    #[ignore = "requires the pinned router files in a private XDG data directory"]
    /// Check Arctic tokenization and semantic ordering with the pinned local fixtures.
    fn pinned_router_embeds_and_retrieves_real_command_descriptions() {
        let mut embedder = Embedder::load().unwrap();
        let commands = vec![
            Command { path: "theme mode".into(), summary: "Set light or dark appearance".into(), args: vec![], examples: vec![] },
            Command { path: "audio volume set".into(), summary: "Set output sound volume".into(), args: vec![], examples: vec![] },
        ];
        let vectors = index(&mut embedder, &commands).unwrap();
        let start = std::time::Instant::now();
        let query = embedder.embed("switch to dark mode", true).unwrap();
        let matches = retrieve_clauses(&[ranking(&query, &vectors)], &commands);
        eprintln!("Arctic retrieval {:.1} ms", start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(matches[0].command.path, "theme mode");
        assert_eq!(query.len(), DIM);
    }

    /// Time cold loading apart from warm query embedding and ranking. Run with
    /// `cargo test --release -p mandara-services pinned_router_times -- --ignored --nocapture`
    /// and a private `XDG_DATA_HOME` holding the pinned router.
    #[test]
    #[ignore = "benchmark: requires the pinned router files in a private XDG data directory"]
    fn pinned_router_times_cold_loading_apart_from_warm_retrieval() {
        let ms = |start: std::time::Instant| start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        let mut embedder = Embedder::load().unwrap();
        let load = ms(start);
        let rows = 1500;
        let vectors: Vec<f32> = (0..rows * DIM).map(|i| ((i * 7919 % 1009) as f32 / 1009.0) - 0.5).collect();
        let median = |values: &mut Vec<f64>| {
            values.sort_by(f64::total_cmp);
            values[values.len() / 2]
        };
        let queries = ["set volume to 40", "switch to dark mode", "open the calendar", "turn off notifications"];
        use tract_onnx::prelude::multithread::{Executor, multithread_tract_scope, set_threading_panel_threshold};
        for (label, executor, panels) in [("one thread", Executor::SingleThread, 64), ("rayon, 64 panels", Executor::RayonGlobal, 64), ("rayon, 8 panels", Executor::RayonGlobal, 8), ("rayon, 0 panels", Executor::RayonGlobal, 0)] {
            set_threading_panel_threshold(panels);
            let mut first = None;
            let (mut embed, mut rank) = (Vec::new(), Vec::new());
            for query in queries.iter().cycle().take(41) {
                let start = std::time::Instant::now();
                let vector = multithread_tract_scope(executor.clone(), || embedder.embed(query, true)).unwrap();
                let embedded = ms(start);
                let start = std::time::Instant::now();
                std::hint::black_box(ranking(&vector, &vectors));
                let ranked = ms(start);
                if first.is_none() {
                    first = Some(embedded);
                } else {
                    embed.push(embedded);
                    rank.push(ranked);
                }
            }
            eprintln!("{label}: first query embed {:.2} ms; warm median embed {:.2} ms + ranking {:.3} ms over {rows} rows", first.unwrap(), median(&mut embed), median(&mut rank));
        }
        eprintln!("load {load:.1} ms");
    }
}
