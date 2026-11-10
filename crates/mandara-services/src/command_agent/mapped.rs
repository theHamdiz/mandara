//! Zero-copy GGUF weights borrowed from one read-only file mapping.
//!
//! Candle's GGUF reader copies every tensor into a fresh buffer and then into
//! owned blocks. Here each quantized tensor borrows its blocks from a shared
//! read-only mapping instead, so weights stay clean file-backed pages that the
//! kernel can share with the page cache and reclaim, rather than private
//! anonymous memory. The disposable worker unmaps everything when it exits.

use candle_core::quantized::{GgmlDType, GgmlType, QStorage, QTensor, QuantizedType, gguf_file, k_quants};
use candle_core::{CpuStorage, Result, bail};
use half::{bf16, f16};
use memmap2::Mmap;
use std::{fs::File, marker::PhantomData, path::Path, sync::Arc};

/// A GGUF file's metadata, read from its mapping, and the mapping itself.
pub(super) struct Gguf {
    map: Arc<Mmap>,
    pub content: gguf_file::Content,
}

impl Gguf {
    /// Map the pinned model read-only and parse its header without copying tensors.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the mapping is read-only and private to the worker process.
        // Installs and removals replace or unlink model files rather than
        // rewriting them in place, so an open mapping keeps its inode intact.
        let map = unsafe { memmap2::MmapOptions::new().map(&file)? };
        let content = gguf_file::Content::read(&mut std::io::Cursor::new(&map[..]))?;
        Ok(Self { map: Arc::new(map), content })
    }

    /// Borrow one tensor's blocks from the mapping after validating bounds and alignment.
    pub fn tensor(&self, name: &str) -> Result<QTensor> {
        let Some(info) = self.content.tensor_infos.get(name) else { bail!("cannot find tensor info for {name}") };
        let elems = info.shape.elem_count();
        let dtype = info.ggml_dtype;
        if !elems.is_multiple_of(dtype.block_size()) {
            bail!("tensor {name} has {elems} elements, not a multiple of its block size")
        }
        let blocks = elems / dtype.block_size();
        let start = usize::try_from(self.content.tensor_data_offset.saturating_add(info.offset)).map_err(|_| candle_core::Error::Msg(format!("tensor {name} offset overflows")))?;
        let end = blocks.checked_mul(dtype.type_size()).and_then(|bytes| start.checked_add(bytes));
        if end.is_none_or(|end| end > self.map.len()) {
            bail!("tensor {name} extends past the end of the model file")
        }
        let storage: Box<dyn QuantizedType> = match dtype {
            GgmlDType::F32 => Mapped::<f32>::boxed(&self.map, start, blocks)?,
            GgmlDType::F16 => Mapped::<f16>::boxed(&self.map, start, blocks)?,
            GgmlDType::BF16 => Mapped::<bf16>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q4_0 => Mapped::<k_quants::BlockQ4_0>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q4_1 => Mapped::<k_quants::BlockQ4_1>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q5_0 => Mapped::<k_quants::BlockQ5_0>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q5_1 => Mapped::<k_quants::BlockQ5_1>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q8_0 => Mapped::<k_quants::BlockQ8_0>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q8_1 => Mapped::<k_quants::BlockQ8_1>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q2K => Mapped::<k_quants::BlockQ2K>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q3K => Mapped::<k_quants::BlockQ3K>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q4K => Mapped::<k_quants::BlockQ4K>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q5K => Mapped::<k_quants::BlockQ5K>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q6K => Mapped::<k_quants::BlockQ6K>::boxed(&self.map, start, blocks)?,
            GgmlDType::Q8K => Mapped::<k_quants::BlockQ8K>::boxed(&self.map, start, blocks)?,
        };
        QTensor::new(QStorage::Cpu(storage), info.shape.clone())
    }

    /// Borrow a tensor the model may leave out, such as an untied output projection.
    /// Only absence gives `None`: a recorded tensor that fails validation is an error,
    /// so a damaged weight never silently falls back to a substitute.
    pub fn optional_tensor(&self, name: &str) -> Result<Option<QTensor>> {
        if self.content.tensor_infos.contains_key(name) { self.tensor(name).map(Some) } else { Ok(None) }
    }
}

/// Blocks borrowed from the mapping. A requantization, which inference never
/// performs, moves the tensor into owned blocks instead of writing the file.
enum Mapped<T> {
    Borrowed { map: Arc<Mmap>, start: usize, len: usize, kind: PhantomData<T> },
    Owned(Vec<T>),
}

impl<T: GgmlType + 'static> Mapped<T> {
    /// Box a validated view of `len` blocks of `T` starting at byte `start`.
    fn boxed(map: &Arc<Mmap>, start: usize, len: usize) -> Result<Box<dyn QuantizedType>> {
        if !(map.as_ptr() as usize + start).is_multiple_of(align_of::<T>()) {
            bail!("misaligned {:?} tensor at byte {start}", T::DTYPE)
        }
        Ok(Box::new(Self::Borrowed { map: map.clone(), start, len, kind: PhantomData }))
    }

    fn blocks(&self) -> &[T] {
        match self {
            // SAFETY: `boxed` checked the range against the mapping and the
            // pointer's alignment. GGML blocks are plain integer and float
            // fields, valid for every bit pattern, and the read-only mapping
            // lives as long as this view through its `Arc`.
            Self::Borrowed { map, start, len, .. } => unsafe { std::slice::from_raw_parts(map.as_ptr().add(*start).cast::<T>(), *len) },
            Self::Owned(blocks) => blocks,
        }
    }

    /// Replace a borrowed view with owned zeroed blocks before writing.
    fn owned(&mut self) -> &mut Vec<T> {
        if let Self::Borrowed { len, .. } = self {
            *self = Self::Owned(vec![T::zeros(); *len]);
        }
        match self {
            Self::Owned(blocks) => blocks,
            Self::Borrowed { .. } => unreachable!("converted to owned blocks above"),
        }
    }
}

impl<T: GgmlType + 'static> QuantizedType for Mapped<T> {
    fn dtype(&self) -> GgmlDType {
        T::DTYPE
    }
    fn matmul_t(&self, mkn: (usize, usize, usize), lhs: &[f32], dst: &mut [f32]) -> Result<()> {
        k_quants::matmul(mkn, lhs, self.blocks(), dst)
    }
    fn matmul_t_f16(&self, mkn: (usize, usize, usize), lhs: &[f16], dst: &mut [f16]) -> Result<()> {
        k_quants::matmul_f16(mkn, lhs, self.blocks(), dst)
    }
    /// Dequantize only the requested rows, as Candle's owned storage does.
    fn embedding(&self, ids: &[u32], rows: usize, hidden: usize) -> Result<CpuStorage> {
        if !hidden.is_multiple_of(T::BLCK_SIZE) {
            bail!("quantized embedding hidden size {hidden} is not divisible by block size {}", T::BLCK_SIZE)
        }
        let row_blocks = hidden / T::BLCK_SIZE;
        let blocks = self.blocks();
        if blocks.len() != rows * row_blocks {
            bail!("quantized tensor has {} blocks, expected {}", blocks.len(), rows * row_blocks)
        }
        let mut out = vec![0f32; ids.len() * hidden];
        for (dst, &row) in out.chunks_exact_mut(hidden).zip(ids) {
            let row = row as usize;
            if row >= rows {
                bail!("embedding id {row} is out of range for {rows} rows")
            }
            T::to_float(&blocks[row * row_blocks..(row + 1) * row_blocks], dst);
        }
        Ok(CpuStorage::F32(out))
    }
    fn dequantize(&self, elem_count: usize) -> Result<CpuStorage> {
        let mut ys = vec![0f32; elem_count];
        T::to_float(self.blocks(), &mut ys);
        Ok(CpuStorage::F32(ys))
    }
    fn storage_size_in_bytes(&self) -> usize {
        size_of_val(self.blocks())
    }
    fn as_ptr(&self) -> *const u8 {
        self.blocks().as_ptr().cast()
    }
    fn block_size(&self) -> usize {
        T::BLCK_SIZE
    }
    fn from_float(&mut self, xs: &[f32]) {
        T::from_float(xs, self.owned());
    }
    fn from_float_imatrix(&mut self, xs: &[f32], imatrix_weights: &[f32], n_per_row: usize) {
        T::from_float_imatrix(xs, self.owned(), imatrix_weights, n_per_row);
    }
    fn size(&self) -> usize {
        size_of_val(self.blocks())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::{Device, Module, Tensor};

    /// Write a small GGUF file through Candle and read it back through the mapping.
    #[test]
    fn mapped_tensors_match_candles_owned_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        let values: Vec<f32> = (0..2 * 256).map(|i| (i as f32 * 0.37).sin()).collect();
        let source = Tensor::from_vec(values, (2, 256), &Device::Cpu).unwrap();
        let q8 = QTensor::quantize(&source, GgmlDType::Q8_0).unwrap();
        let q4k = QTensor::quantize(&source, GgmlDType::Q4K).unwrap();
        let f32s = QTensor::quantize(&source, GgmlDType::F32).unwrap();
        let mut file = File::create(&path).unwrap();
        gguf_file::write(&mut file, &[], &[("q8", &q8), ("q4k", &q4k), ("f32", &f32s)]).unwrap();
        drop(file);

        let mapped = Gguf::open(&path).unwrap();
        let mut reader = File::open(&path).unwrap();
        let owned = gguf_file::Content::read(&mut reader).unwrap();
        let ids = Tensor::new(&[1u32, 0], &Device::Cpu).unwrap();
        let lhs = Tensor::from_vec((0..256).map(|i| i as f32 / 256.0).collect::<Vec<_>>(), (1, 256), &Device::Cpu).unwrap();
        for name in ["q8", "q4k", "f32"] {
            let a = mapped.tensor(name).unwrap();
            let b = owned.tensor(&mut reader, name, &Device::Cpu).unwrap();
            assert_eq!((a.dtype(), a.shape()), (b.dtype(), b.shape()));
            assert_eq!(a.data().unwrap(), b.data().unwrap());
            let rows = |t: &QTensor| t.embedding(&ids).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
            assert_eq!(rows(&a), rows(&b));
            let full = |t: &QTensor| t.dequantize(&Device::Cpu).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
            assert_eq!(full(&a), full(&b));
            let product = |t: QTensor| candle_core::quantized::QMatMul::from_qtensor(t).unwrap().forward(&lhs).unwrap().flatten_all().unwrap().to_vec1::<f32>().unwrap();
            assert_eq!(product(a), product(b));
        }
        assert!(mapped.tensor("missing").is_err());
    }

    /// A tensor whose recorded extent passes the end of the file is rejected, not read.
    #[test]
    fn truncated_files_are_rejected_before_borrowing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        let source = Tensor::zeros((4, 256), candle_core::DType::F32, &Device::Cpu).unwrap();
        let q = QTensor::quantize(&source, GgmlDType::Q8_0).unwrap();
        let mut file = File::create(&path).unwrap();
        gguf_file::write(&mut file, &[], &[("w", &q)]).unwrap();
        let len = file.metadata().unwrap().len();
        file.set_len(len - 1).unwrap();
        drop(file);
        let map = Gguf::open(&path).unwrap();
        assert!(map.tensor("w").is_err());
        // An optional tensor that is present but damaged is an error, not absent.
        assert!(map.optional_tensor("w").is_err());
        assert!(map.optional_tensor("output.weight").unwrap().is_none());
    }

    /// Requantizing a borrowed tensor writes owned blocks and leaves the file untouched.
    #[test]
    fn requantization_moves_to_owned_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        let source = Tensor::ones((1, 32), candle_core::DType::F32, &Device::Cpu).unwrap();
        let q = QTensor::quantize(&source, GgmlDType::Q8_0).unwrap();
        let mut file = File::create(&path).unwrap();
        gguf_file::write(&mut file, &[], &[("w", &q)]).unwrap();
        drop(file);
        let before = std::fs::read(&path).unwrap();
        let map = Gguf::open(&path).unwrap();
        let start = usize::try_from(map.content.tensor_data_offset).unwrap();
        let mut view = Mapped::<k_quants::BlockQ8_0>::Borrowed { map: map.map.clone(), start, len: 1, kind: PhantomData };
        view.from_float(&[2.0; 32]);
        assert!(matches!(view, Mapped::Owned(_)));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}
