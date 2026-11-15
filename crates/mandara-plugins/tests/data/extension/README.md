# Extension-platform fixture

`fixture.wasm` is one small `extension-plugin` component used by
`crates/mandara-plugins/tests/extension.rs`. Its `mode` setting picks what
it does:

| `mode`   | What it does                                                  |
|----------|---------------------------------------------------------------|
| `ok`     | negotiates real features; a view with a field, switch and list; events that submit broker requests and log the answers |
| `future` | negotiates a feature no host has (`ui.webview`)               |
| `huge`   | returns a view of 5000 nodes                                  |
| `spin`   | `launcher-query` never returns                                |
| `sneaky` | asks for the clipboard while rendering (no user gesture)      |

Every event and broker reply is appended to a log that the view's first
text node shows, so the tests read what the plugin saw.

It was built from `fixture/` (a crate outside the workspace, against the
real `wit/` folder):

```bash
cd crates/mandara-plugins/tests/data/extension/fixture
cargo build --release --locked --target wasm32-wasip2
cp target/wasm32-wasip2/release/extension_fixture.wasm ../fixture.wasm
```

Rebuild it whenever `wit/extensions.wit` changes.
