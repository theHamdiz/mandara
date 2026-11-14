# Local AI

Open Studio → Plugins, enable **Local AI**, then open its **Settings**.
Choose a provider and a model or workspace. The panel opens on Chat;
**Models** keeps Ollama's load, unload and GPU-memory controls.

This plugin connects to a server you run. It does not install models or start
an inference engine. The [AI Shell Agent](command-agent.md) is the separate
feature that turns launcher requests into desktop commands.

| Provider | Default server address | Before connecting |
|---|---|---|
| Ollama | `http://127.0.0.1:11434` | Run Ollama and install a model |
| llama.cpp | `http://127.0.0.1:8080/v1` | Start `llama-server` with your model |
| Jan | `http://127.0.0.1:1337/v1` | Start Jan's Local API Server with a local model |
| LM Studio | `http://127.0.0.1:1234/v1` | Start its local API server |
| AnythingLLM | `http://127.0.0.1:3001/api` | Enable API access and create a workspace |
| OpenAI-compatible | `http://127.0.0.1:8080/v1` | Use a server with `/models` and streaming `/chat/completions` |

Leave **Server address** empty to use the preset, or enter your server's API
base, including any reverse-proxy prefix. Existing Ollama addresses are kept.
The panel's searchable picker lists the server's models or workspace slugs;
you can also enter an exact ID in **Model or workspace** in Studio. The picker
opens over the conversation, scrolls independently and closes with Esc.
**New chat**, **Models** and **Settings** sit in the top toolbar.

Turn on **Use an API key** if your server requires one, then save it in the
keyring editor below. AnythingLLM always needs a key. Keys are scoped to the
server address and stay out of Mandara's configuration file. Authenticated
requests use HTTPS or an explicitly local HTTP address and never follow
redirects. Jan's key must match its Local API Server settings.

Write a message and press **Send** or **Ctrl+Enter**. Enter adds a line. **Stop**
interrupts the current request; **Copy** copies an answer. **New chat** clears
the transcript and gives AnythingLLM a fresh session identity. Closing the
last panel cancels the stream and clears the transcript and drafts. Mandara
does not delete your server's history or change its workspace settings.

Only one reply runs at a time. Prompts and replies are each limited to 8 KiB;
the transcript keeps at most sixteen messages and 16–32 KiB, dropping the
oldest complete turns. **Reply tokens** defaults to 1024. AnythingLLM owns
its token and inference-context limits in its workspace settings; Mandara
still bounds what it receives. Other servers receive the configured token
limit. Model RAM, GPU memory and worker threads belong to the server. Only
Ollama exposes the memory statistics and model controls this panel uses.

No chat runs just because you type. There is no background chat polling.

The same controls are available through IPC:

```bash
mandara msg ai models
mandara msg ai chat send Explain this idea
mandara msg ai chat stop
mandara msg ai chat new
mandara msg ai loaded
mandara msg ai load llama3.2
mandara msg ai unload all
```

Load, unload and loaded-memory commands apply to Ollama. A chat command and
the panel share the same request limit.

Provider documentation: [llama.cpp server](https://github.com/ggml-org/llama.cpp/tree/master/tools/server),
[Jan Local API Server](https://docs.jan.ai/desktop/api-server),
[AnythingLLM API](https://docs.anythingllm.com/features/api).
