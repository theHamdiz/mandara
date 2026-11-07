# Copyright

Mandara Shell
Copyright (C) 2026 Ahmad Hamdi (theHamdiz) and contributors

This program is free software: you can redistribute it and/or modify it
under the terms of the GNU General Public License as published by the Free
Software Foundation, either version 3 of the License, or (at your option)
any later version.

This program is distributed in the hope that it will be useful, but WITHOUT
ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License
(LICENSE.md) for more details.

SPDX-License-Identifier: `GPL-3.0-or-later`

Third-party material keeps its own license: vendored code under `vendor/`
keeps the license stated in its directory, and fonts and models that
Mandara downloads on demand keep theirs (SIL Open Font License for Google
Fonts; Apache-2.0 for the PaddleOCR models and for Depth Anything V2 Small,
the wallpaper depth model, in its `onnx-community/depth-anything-v2-small`
ONNX export). The Kufic words on the main menu's tiles are glyph outlines
from Reem Kufi (SIL Open Font License 1.1, its text in
`crates/mandara-ui/assets/calligraphy/OFL.txt`).

The full license text is in [LICENSE.md](LICENSE.md).

## Optional command models

The command agent downloads no models until requested. Arctic Embed XS and
Qwen3 0.6B retain their Apache-2.0 licenses. FunctionGemma retains Google's
[Gemma terms](https://ai.google.dev/gemma/terms). Quantized artifacts come from
their linked Hugging Face repositories; model weights are not included in
Mandara's binary or source distribution.

The bounded Gemma GGUF adapter in
`crates/mandara-services/src/command_agent/gemma.rs` derives from Hugging Face
Candle 0.11.1 under Apache-2.0; the adjacent `gemma.LICENSE` preserves its license.
It shares quantized embeddings and rotary tables, bounds context, and corrects
chunked attention masks and the activation used by Gemma 3.
