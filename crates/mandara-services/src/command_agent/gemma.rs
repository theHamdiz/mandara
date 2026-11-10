//! Gemma 3 GGUF inference with bounded context and quantized token lookup.
//!
//! Adapted from Hugging Face Candle 0.11.1, quantized_gemma3.rs, under
//! Apache-2.0. See gemma.LICENSE. Mandara changes: zero-copy mapped weights, quantized tied embeddings,
//! shared bounded rotary tables, GELU activation and absolute-position masks.

use candle::quantized::QTensor;
use candle::{DType, Device, IndexOp, Result, Tensor};
use candle_core as candle;
use candle_nn::Module;
use candle_transformers::quantized_nn::RmsNorm;
use std::sync::Arc;

/// Rotary tables need only the bounded compiler context, not the training context.
const MAX_SEQ_LEN: usize = super::compiler::MAX_CONTEXT_TOKENS;
/// Gemma 3 alternates local attention with one global layer in each group of six.
const DEFAULT_SLIDING_WINDOW_TYPE: usize = 6;
/// Published Gemma 3 global-attention rotary base when GGUF metadata omits it.
const DEFAULT_ROPE_FREQUENCY: f32 = 1_000_000.;
/// Published rotary base for Gemma 3 local-attention layers.
const DEFAULT_ROPE_FREQUENCY_SLIDING: f32 = 10_000.;
/// An absent rotary scaling factor leaves model frequencies unchanged.
const DEFAULT_ROPE_FREQUENCY_SCALE_FACTOR: f32 = 1.;

#[derive(Debug, Clone)]
struct QMatMul {
    inner: candle::quantized::QMatMul,
    span: tracing::Span,
}

impl QMatMul {
    /// Keep a quantized matrix and its tracing span together.
    fn from_qtensor(qtensor: QTensor) -> Result<Self> {
        let inner = candle::quantized::QMatMul::from_qtensor(qtensor)?;
        let span = tracing::span!(tracing::Level::TRACE, "qmatmul");
        Ok(Self { inner, span })
    }

    /// Multiply without dequantizing the complete weight matrix.
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let _enter = self.span.enter();
        self.inner.forward(xs)
    }
}

#[derive(Debug, Clone)]
struct Mlp {
    feed_forward_gate: QMatMul, // ffn_gate in GGUF
    feed_forward_up: QMatMul,   // ffn_up in GGUF
    feed_forward_down: QMatMul, // ffn_down in GGUF
}

impl Module for Mlp {
    /// Apply Gemma 3’s GELU gate before the quantized down projection.
    fn forward(&self, xs: &Tensor) -> Result<Tensor> {
        let gate = self.feed_forward_gate.forward(xs)?;
        let up = self.feed_forward_up.forward(xs)?;
        let activated = candle_nn::Activation::GeluPytorchTanh.forward(&gate)?;
        let gated = (activated * up)?;
        self.feed_forward_down.forward(&gated)
    }
}

#[derive(Debug, Clone)]
struct RotaryEmbedding {
    sin: Tensor,
    cos: Tensor,
}

impl RotaryEmbedding {
    /// Build context-bounded rotary tables shared by attention layers.
    fn new(head_dim: usize, rope_frequency: f32, device: &Device) -> Result<Self> {
        let theta: Vec<_> = (0..head_dim).step_by(2).map(|i| 1f32 / rope_frequency.powf(i as f32 / head_dim as f32)).collect();
        let theta = Tensor::new(theta.as_slice(), device)?;
        let idx_theta =
            Tensor::arange(0, MAX_SEQ_LEN as u32, device)?.to_dtype(DType::F32)?.reshape((MAX_SEQ_LEN, 1))?.matmul(&theta.reshape((1, theta.elem_count()))?)?;
        let cos = idx_theta.cos()?;
        let sin = idx_theta.sin()?;
        Ok(Self { sin, cos })
    }

    /// Rotate query and key chunks at their absolute cache positions.
    fn apply_rotary_emb_qkv(&self, q: &Tensor, k: &Tensor, index_pos: usize) -> Result<(Tensor, Tensor)> {
        let (_b_sz, _h, seq_len, _n_embd) = q.dims4()?;
        let cos = self.cos.narrow(0, index_pos, seq_len)?;
        let sin = self.sin.narrow(0, index_pos, seq_len)?;
        let q_embed = candle_nn::rotary_emb::rope(&q.contiguous()?, &cos, &sin)?;
        let k_embed = candle_nn::rotary_emb::rope(&k.contiguous()?, &cos, &sin)?;
        Ok((q_embed, k_embed))
    }
}

#[derive(Debug, Clone)]
struct LayerWeights {
    // Attention components
    attention_wq: QMatMul,
    attention_wk: QMatMul,
    attention_wv: QMatMul,
    attention_wo: QMatMul,

    // Specialized normalization for Q and K
    attention_q_norm: RmsNorm,
    attention_k_norm: RmsNorm,

    // Layer normalization
    attention_norm: RmsNorm,      // Applied before attention
    post_attention_norm: RmsNorm, // Applied after attention
    ffn_norm: RmsNorm,            // Applied before feedforward
    post_ffn_norm: RmsNorm,       // Applied after feedforward

    // Feed-forward network
    mlp: Mlp,

    // Attention parameters
    n_head: usize,    // Number of query heads
    n_kv_head: usize, // Number of key-value heads
    head_dim: usize,  // Dimension of each head
    q_dim: usize,     // Total dimension for queries

    sliding_window_size: Option<usize>,

    rotary_embedding: Arc<RotaryEmbedding>,
    neg_inf: Tensor,

    // Cache
    kv_cache: Option<(Tensor, Tensor)>,

    // Tracing
    span_attn: tracing::Span,
    span_mlp: tracing::Span,
}

impl LayerWeights {
    /// Mask future and expired local keys using absolute token positions.
    fn mask(&self, b_sz: usize, seq_len: usize, index_pos: usize, dtype: DType, device: &Device) -> Result<Tensor> {
        let _ = dtype;
        let mask = attention_mask(seq_len, index_pos, self.sliding_window_size);
        Tensor::from_slice(&mask, (seq_len, seq_len + index_pos), device)?.expand((b_sz, 1, seq_len, seq_len + index_pos))
    }

    /// Append keys and values, then apply the layer’s local or global attention.
    fn forward_attn(&mut self, x: &Tensor, mask: Option<&Tensor>, index_pos: usize) -> Result<Tensor> {
        let _enter = self.span_attn.enter();
        let (b_sz, seq_len, _) = x.dims3()?;

        let q = self.attention_wq.forward(x)?;
        let k = self.attention_wk.forward(x)?;
        let v = self.attention_wv.forward(x)?;

        let q = q.reshape((b_sz, seq_len, self.n_head, self.head_dim))?.transpose(1, 2)?;
        let k = k.reshape((b_sz, seq_len, self.n_kv_head, self.head_dim))?.transpose(1, 2)?;
        let v = v.reshape((b_sz, seq_len, self.n_kv_head, self.head_dim))?.transpose(1, 2)?;

        let q = self.attention_q_norm.forward(&q.contiguous()?)?;
        let k = self.attention_k_norm.forward(&k.contiguous()?)?;

        let (q, k) = self.rotary_embedding.apply_rotary_emb_qkv(&q, &k, index_pos)?;

        let (k, v) = match &self.kv_cache {
            None => (k, v),
            Some((k_cache, v_cache)) => {
                if index_pos == 0 {
                    (k, v)
                } else {
                    let k = Tensor::cat(&[k_cache, &k], 2)?; // concat on seq dim
                    let v = Tensor::cat(&[v_cache, &v], 2)?;
                    (k, v)
                }
            }
        };
        self.kv_cache = Some((k.clone(), v.clone())); // update cache

        // Repeat KV for GQA
        let k = candle_transformers::utils::repeat_kv(k, self.n_head / self.n_kv_head)?;
        let v = candle_transformers::utils::repeat_kv(v, self.n_head / self.n_kv_head)?;

        // Scaled Dot-Product Attention
        let scale = 1.0 / (self.head_dim as f64).sqrt();
        let mut attn_weights = (q.matmul(&k.transpose(2, 3)?)? * scale)?;

        if let Some(mask) = mask {
            let mask = mask.broadcast_as(attn_weights.shape())?;
            let neg_inf = self.neg_inf.broadcast_as(attn_weights.dims())?;
            attn_weights = mask.eq(0u32)?.where_cond(&neg_inf, &attn_weights)?;
        }

        let attn_weights = candle_nn::ops::softmax_last_dim(&attn_weights)?;
        let attn_output = attn_weights.matmul(&v)?;

        let attn_output = attn_output.transpose(1, 2)?.reshape((b_sz, seq_len, self.q_dim))?;

        self.attention_wo.forward(&attn_output)
    }
}

#[derive(Debug, Clone)]
pub struct ModelWeights {
    tok_embeddings: Arc<QTensor>,
    embedding_length: usize,
    layers: Vec<LayerWeights>,
    norm: RmsNorm,
    output: QMatMul,
    span: tracing::Span,
    span_output: tracing::Span,
}

impl ModelWeights {
    /// Load quantized weights while sharing tied embeddings and rotary tables.
    pub fn from_gguf(gg: &super::mapped::Gguf, device: &Device) -> Result<Self> {
        let ct = &gg.content;
        // Detect architecture prefix by probing which keys exist in metadata.
        // This supports gemma3, gemma2, gemma, gemma-embedding, and future variants.
        let prefix = ["gemma3", "gemma2", "gemma", "gemma-embedding"]
            .iter()
            .find(|p| ct.metadata.contains_key(&format!("{}.attention.head_count", p)))
            .copied()
            .unwrap_or("gemma3");

        let md_get = |s: &str| {
            let key = format!("{prefix}.{s}");
            match ct.metadata.get(&key) {
                None => candle::bail!("cannot find {key} in metadata"),
                Some(v) => Ok(v),
            }
        };

        let head_count = md_get("attention.head_count")?.to_u32()? as usize;
        let head_count_kv = md_get("attention.head_count_kv")?.to_u32()? as usize;
        let block_count = md_get("block_count")?.to_u32()? as usize;
        let embedding_length = md_get("embedding_length")?.to_u32()? as usize;
        let key_length = md_get("attention.key_length")?.to_u32()? as usize;
        let _value_length = md_get("attention.value_length")?.to_u32()? as usize;
        let rms_norm_eps = md_get("attention.layer_norm_rms_epsilon")?.to_f32()? as f64;
        let sliding_window_size = md_get("attention.sliding_window")?.to_u32()? as usize;

        let sliding_window_type = md_get("attention.sliding_window_type").and_then(|m| Ok(m.to_u32()? as usize)).unwrap_or(DEFAULT_SLIDING_WINDOW_TYPE);

        let rope_freq_base = md_get("rope.freq_base").and_then(|m| m.to_f32()).unwrap_or(DEFAULT_ROPE_FREQUENCY);

        let rope_freq_base_sliding = md_get("rope.local_freq_base").and_then(|m| m.to_f32()).unwrap_or(DEFAULT_ROPE_FREQUENCY_SLIDING);

        // Unused in Llama.cpp so we aren't using it here.
        let _rope_freq_scaling_factor = md_get("rope.scaling.factor").and_then(|m| m.to_f32()).unwrap_or(DEFAULT_ROPE_FREQUENCY_SCALE_FACTOR);

        // Compute the dimensions for queries, keys, and values
        // These are the total dimensions when projected across all heads
        let q_dim = head_count * key_length;

        let neg_inf = Tensor::new(f32::NEG_INFINITY, device)?;

        // Load token embeddings and output projection
        let tok_embeddings = gg.tensor("token_embd.weight")?;
        let tok_embeddings = Arc::new(tok_embeddings);
        let norm = RmsNorm::from_qtensor(gg.tensor("output_norm.weight")?, rms_norm_eps)?;
        let output = match gg.optional_tensor("output.weight")? {
            Some(tensor) => Arc::new(tensor),
            None => tok_embeddings.clone(),
        };

        let global_rope = Arc::new(RotaryEmbedding::new(key_length, rope_freq_base, device)?);
        let local_rope = Arc::new(RotaryEmbedding::new(key_length, rope_freq_base_sliding, device)?);
        let mut layers = Vec::with_capacity(block_count);
        for layer_idx in 0..block_count {
            let prefix = format!("blk.{layer_idx}");

            let attention_wq = gg.tensor(&format!("{prefix}.attn_q.weight"))?;
            let attention_wk = gg.tensor(&format!("{prefix}.attn_k.weight"))?;
            let attention_wv = gg.tensor(&format!("{prefix}.attn_v.weight"))?;
            let attention_wo = gg.tensor(&format!("{prefix}.attn_output.weight"))?;

            let attention_q_norm = RmsNorm::from_qtensor(gg.tensor(&format!("{prefix}.attn_q_norm.weight"))?, rms_norm_eps)?;

            let attention_k_norm = RmsNorm::from_qtensor(gg.tensor(&format!("{prefix}.attn_k_norm.weight"))?, rms_norm_eps)?;

            let attention_norm = RmsNorm::from_qtensor(gg.tensor(&format!("{prefix}.attn_norm.weight"))?, rms_norm_eps)?;

            let post_attention_norm = RmsNorm::from_qtensor(gg.tensor(&format!("{prefix}.post_attention_norm.weight"))?, rms_norm_eps)?;

            let ffn_norm = RmsNorm::from_qtensor(gg.tensor(&format!("{prefix}.ffn_norm.weight"))?, rms_norm_eps)?;

            let post_ffn_norm = RmsNorm::from_qtensor(gg.tensor(&format!("{prefix}.post_ffw_norm.weight"))?, rms_norm_eps)?;

            let feed_forward_gate = gg.tensor(&format!("{prefix}.ffn_gate.weight"))?;
            let feed_forward_up = gg.tensor(&format!("{prefix}.ffn_up.weight"))?;
            let feed_forward_down = gg.tensor(&format!("{prefix}.ffn_down.weight"))?;

            let mlp = Mlp {
                feed_forward_gate: QMatMul::from_qtensor(feed_forward_gate)?,
                feed_forward_up: QMatMul::from_qtensor(feed_forward_up)?,
                feed_forward_down: QMatMul::from_qtensor(feed_forward_down)?,
            };

            // Sliding window pattern hardcoded to 6 because it's not explicitly defined
            let is_sliding = (layer_idx + 1) % sliding_window_type > 0;
            let sliding_window_size = is_sliding.then_some(sliding_window_size);
            let rotary_embedding = if is_sliding { local_rope.clone() } else { global_rope.clone() };

            // Tracing spans
            let span_attn = tracing::span!(tracing::Level::TRACE, "attn");
            let span_mlp = tracing::span!(tracing::Level::TRACE, "attn-mlp");

            layers.push(LayerWeights {
                attention_wq: QMatMul::from_qtensor(attention_wq)?,
                attention_wk: QMatMul::from_qtensor(attention_wk)?,
                attention_wv: QMatMul::from_qtensor(attention_wv)?,
                attention_wo: QMatMul::from_qtensor(attention_wo)?,
                attention_q_norm,
                attention_k_norm,
                attention_norm,
                post_attention_norm,
                ffn_norm,
                post_ffn_norm,
                mlp,
                n_head: head_count,
                n_kv_head: head_count_kv,
                head_dim: key_length,
                q_dim,
                sliding_window_size,
                rotary_embedding,
                neg_inf: neg_inf.clone(),
                kv_cache: None,
                span_attn,
                span_mlp,
            })
        }

        let span = tracing::span!(tracing::Level::TRACE, "model");
        let span_output = tracing::span!(tracing::Level::TRACE, "output");

        Ok(Self {
            tok_embeddings,
            embedding_length,
            layers,
            norm,
            output: QMatMul { inner: candle::quantized::QMatMul::from_arc(output)?, span: tracing::span!(tracing::Level::TRACE, "output") },
            span,
            span_output,
        })
    }

    /// Release every layer’s attention cache before a new request.
    pub fn clear_kv_cache(&mut self) {
        for layer in &mut self.layers {
            layer.kv_cache = None;
        }
    }

    /// Look up only input-token embedding rows and advance all transformer layers.
    pub fn forward(&mut self, x: &Tensor, index_pos: usize) -> Result<Tensor> {
        let (b_sz, seq_len) = x.dims2()?;
        let _enter = self.span.enter();

        let mut layer_in = self.tok_embeddings.embedding(x)?;
        layer_in = (layer_in * (self.embedding_length as f64).sqrt())?;

        for layer in self.layers.iter_mut() {
            let attention_mask =
                if seq_len == 1 && layer.sliding_window_size.is_none() { None } else { Some(layer.mask(b_sz, seq_len, index_pos, x.dtype(), x.device())?) };

            // Attention block
            let residual = &layer_in;
            let x = layer.attention_norm.forward(&layer_in)?;
            let x = layer.forward_attn(&x, attention_mask.as_ref(), index_pos)?;
            let x = layer.post_attention_norm.forward(&x)?;
            let x = (x + residual)?;

            // Feed-forward block
            let _enter = layer.span_mlp.enter();
            let residual = &x;
            let x = layer.ffn_norm.forward(&x)?;
            let x = layer.mlp.forward(&x)?;
            let x = layer.post_ffn_norm.forward(&x)?;
            let x = (x + residual)?;
            drop(_enter);

            layer_in = x;
        }

        let _enter = self.span_output.enter();

        let x = layer_in.i((.., seq_len - 1, ..))?;
        let x = self.norm.forward(&x)?;
        let output = self.output.forward(&x)?;

        Ok(output)
    }
}

/// Mask both prefill chunks and generation using absolute key positions.
fn attention_mask(length: usize, offset: usize, window: Option<usize>) -> Vec<u32> {
    (0..length).flat_map(|i| (0..offset + length).map(move |j| u32::from(j <= offset + i && window.is_none_or(|width| j + width > offset + i)))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunked_prefill_preserves_past_keys_and_masks_future_keys() {
        assert_eq!(attention_mask(2, 2, None), [1, 1, 1, 0, 1, 1, 1, 1]);
        assert_eq!(attention_mask(1, 4, Some(2)), [0, 0, 0, 1, 1]);
        assert_eq!(attention_mask(2, 2, Some(2)), [0, 1, 1, 0, 0, 0, 1, 1]);
    }
    #[test]
    fn rotary_tables_are_bounded_by_the_actual_compiler_context() {
        let rope = RotaryEmbedding::new(8, DEFAULT_ROPE_FREQUENCY, &Device::Cpu).unwrap();
        assert_eq!(rope.sin.dims(), [MAX_SEQ_LEN, 4]);
        assert_eq!(rope.cos.dims(), [MAX_SEQ_LEN, 4]);
    }
}
