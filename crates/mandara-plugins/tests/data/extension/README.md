# Plugin-platform fixture

`fixture.wasm` is one small plugin component used by
`crates/mandara-plugins/tests/extension.rs`. Its `mode` setting picks what
it does:

| `mode`   | What it does                                                  |
|----------|---------------------------------------------------------------|
| `ok`     | negotiates real features; a view with a field, switch and list; events that submit broker requests and log the answers |
| `future` | negotiates a feature no host has (`ui.webview`)               |
| `huge`   | returns a view of 5000 nodes                                  |
| `spin`   | `launcher-query` never returns                                |
| `sneaky` | asks for the clipboard while rendering (no user gesture)      |
| `async`  | returns no launcher results until its folder-list reply arrives |
| `live`   | shows status, list-projects, render and stop buttons for Studio relay tests |

Every event and broker reply is appended to a log that the view's first
text node shows, so the tests read what the plugin saw.
The `request` command takes a capability, operation and JSON payload,
so sandbox tests can exercise the actual broker without special host hooks.

It was built from `fixture/` (a crate outside the workspace, against the
real `wit/` folder):

```bash
cd crates/mandara-plugins/tests/data/extension/fixture
cargo build --release --locked --target wasm32-wasip2
cp target/wasm32-wasip2/release/extension_fixture.wasm ../fixture.wasm
```

Rebuild it whenever `wit/plugin.wit` changes.
