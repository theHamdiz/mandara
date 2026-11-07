# Local shell commands

Open **Studio → AI Shell Agent** under **Extend**, enable natural language
shell control and choose a compiler. Download the required models from that
page. Downloads show their actual byte counts in the card background, verify
SHA-256 before installation and build a command index. Pause keeps partial
files for resume; Cancel removes partial files. Models have their own licenses,
shown on their cards, and are never bundled or downloaded by the installer.

Type `!dark mode` or `! set volume to 40` in the launcher. A bare `!` shows
examples. Ordinary launcher searches do not start inference. The same rows
show retrieval candidates and the final action preview in Classic, Grid and
Beacon. Enter runs the previewed actions through Mandara's command dispatcher;
Esc cancels. The preview is a proposal, so check its command and target values.

| Component | Download including tokenizer | License |
|---|---:|---|
| Arctic Embed XS, int8 ONNX | about 24 MB | Apache-2.0 |
| FunctionGemma 270M, Q4_K_M, default compiler | about 287 MB | Gemma |
| Qwen3 0.6B, Q4_K_M, optional compiler | about 409 MB | Apache-2.0 |

The default setup downloads about 311 MB. These are verified artifact sizes,
not RAM requirements. Inference needs additional working memory. The models
are stored in `$XDG_DATA_HOME/mandara/models/agent-*`, normally
`~/.local/share/mandara/models/`. Delete them from their cards to reclaim disk
space. `mandara msg models list`, `models download <id>` and
`models remove <id>` expose the same model inventory.

FunctionGemma compiles one action. Qwen permits up to four actions and supports
multilingual requests. Qwen translates non-English queries into English for
Arctic retrieval, then compiles the original request against the retrieved
commands, reusing its weights and clearing the translation KV cache. The test bench
shows the candidates, generated JSON and measured retrieval/compilation time
without executing anything. It also has an immediate purge button.

Each request runs in a short-lived child invocation of the same Rust binary.
Loading, tokenization, retrieval and sampling stay off the UI thread. Only ten
retrieved schemas enter the compiler context. A grammar masks invalid tokens;
the output is checked again against those schemas and the live dispatcher.
Closing the launcher, removing `!`, changing the query or disabling the agent
cancels its worker. A request also ends after 15 seconds without a new query.
Downloads and explicit verification check SHA-256; interactive requests check
installed file sizes to keep disk work out of the query path.
After successful compilation the worker exits before execution, releasing all
model state and dependency thread pools. Cold loading and inference speed
depend on the CPU and storage; no fixed sub-millisecond latency is promised.

The index is cached at
`$XDG_CACHE_HOME/mandara/agent/mandara-commands-embeddings.bin` and rebuilt when
command descriptions, arguments, examples or the router model change. Typed
Studio settings are indexed as `config set` actions, including their real
paths, choices and numeric bounds. This catalog is built inside the worker. A stale
or corrupt cache is rejected. Setup can spend longer building this cache;
interactive inference keeps the 15-second limit.

Settings live under `[launcher.agent]`: `enabled` defaults to `false`,
`engine` is `function-gemma` or `qwen`, and `debounce_ms` defaults to `180`
within `100..=1000`. Existing configurations stay disabled until you opt in.

The same non-executing paths are available over IPC:

```sh
mandara msg agent status
mandara msg agent prepare
mandara msg agent preview 'set volume to 40'
mandara msg agent purge
```

`agent prepare` rebuilds the current command index after models are installed.
`agent preview` never executes its output. `agent purge` cancels the shell's
active inference requests; Studio's purge button cancels both processes.
