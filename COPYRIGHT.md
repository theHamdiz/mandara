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
`crates/mandara-ui/assets/calligraphy/OFL.txt`). Ink Rush's comic display face is
Bangers by Vernon Adams (SIL Open Font License 1.1, its text in
`assets/fonts/bangers/OFL.txt`), downloaded on demand like the other Google
Fonts.

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

## Kaomoji catalog

The bundled catalog is from [Kaomojikan](https://github.com/kaomojikan/kaomoji-data)
under the MIT License.

```text
MIT License

Copyright (c) 2026 kaomojikan

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The category mapping and search aliases were adapted from
[Nandomo Clipboard](https://github.com/is-suzart/nandomo-clipboard) under the
BSD 3-Clause License.

```text
Copyright (c) 2026 is-suzart.

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
