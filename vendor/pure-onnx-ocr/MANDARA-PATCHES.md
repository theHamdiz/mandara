# Mandara patches to `pure-onnx-ocr`

Vendored from crates.io, `pure-onnx-ocr` 0.3.2
([siska-tech/pure-onnx-ocr](https://github.com/siska-tech/pure-onnx-ocr),
Apache-2.0, see `LICENSE-APACHE`), the pure-Rust PaddleOCR engine behind
Mandara's text recognition (`crates/mandara-services/src/ocr.rs`), and wired
in through the workspace's `[patch.crates-io]`. The dependency and API
changes are listed below.

## 1. No default features of `image` and `imageproc` (`Cargo.toml`)

Upstream depends on `image = "0.25"` and `imageproc = "0.25"` with their
default features. Cargo unifies features across the build, so that turned on
every codec of the `image` crate in the whole shell, the AVIF *encoder*
(`ravif`) included, although nothing in Mandara writes AVIF and the engine
itself never decodes a file: Mandara hands it pixels in memory
(`OcrEngine::run_from_image`). Both now have `default-features = false` and
keep only `rayon` (imageproc's default, and the parallel pixel iterators the
default gave `image`). The codecs the shell decodes are chosen by the crates
that decode them (`image` in the workspace manifest, GPUI's patch 3). This
removes `ravif`, `avif-serialize`, `imgref` and `loop9` from the build, and
with GPUI's patch 3 the EXR, QOI, DDS, HDR, TGA and farbfeld decoders.

The engine's own `run_from_path` / `run_from_bytes` decode with whichever
codecs the build has, which in Mandara are PNG, JPEG, WebP, GIF, BMP, ICO,
TIFF and PNM.

## 2. No `time` dependency (`Cargo.toml`)

Upstream lists `time = "0.3.44"`, but no source file uses the crate: the
engine's clock is `std::time::Instant` (`web_time::Instant` in browsers, see
`src/time.rs`). Dropping it takes `time`, `time-core`, `deranged`,
`powerfmt` and `num-conv` out of the build; nothing else in Mandara links
them.

## 3. Shared geometry dependencies (`Cargo.toml`, `src/crop.rs`)

`imageproc` 0.27 uses the RNG and iterator versions already linked by the
shell, replacing the separate versions pulled in by 0.25. Its contour
tracing and bicubic projection remain the OCR engine's geometry operations.
The crop adapter passes `Projection` by value and names
`Border::Constant(Rgb([0, 0, 0]))` explicitly, preserving the previous black
padding outside the source image. Parallel row processing remains enabled.

`crates/mandara-services/tests/ocr_geometry.rs` checks interior pixels,
outside-image padding, vertical-line orientation and detection contours
without loading inference models. The tract dependencies stay on the same
patch version as Mandara's depth engine.

## Packaging changes

Vendored: `src/` (without `src/bin/`, the `ocr_smoke` binary), `Cargo.toml`
and `LICENSE-APACHE` (the Apache-2.0 text, as in `vendor/gpui`). Not vendored:
the smoke binary, examples, integration tests, docs and scripts. The manifest
keeps upstream's package fields, library target, features and dependency
versions; it drops the binary, example and test targets and the
dev-dependencies, sets `publish = false` and declares its own empty
`[workspace]` so it stays outside Mandara's.

To move to a new upstream release, copy its `src/` (minus `src/bin/`), carry
the dependency versions over into this `Cargo.toml` and keep these patches:
patch 1 (no default features of `image` and `imageproc`) and patch 2 (no
`time`: upstream's manifest still lists it, so leave it out again unless
the new sources really use it), plus patch 3's geometry dependency and
explicit border adapter while upstream uses the older API.
