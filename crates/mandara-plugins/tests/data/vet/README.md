# Compliance-check fixture

`fixture.wasm` is one small plugin component used by the `vet` tests in
`crates/mandara-plugins/tests/vet.rs`. Its behaviour comes from its `mode`
setting, whose default each test writes into a `plugin.toml` next to a copy
of the component:

| `mode`  | What it does                          | Expected verdict |
|---------|---------------------------------------|------------------|
| `ok`    | a bar item with a label               | passes           |
| `empty` | a bar item whose only label is blank  | Placeholder      |
| `trap`  | `init` panics                         | Broken           |
| `spin`  | `bar` never returns                   | Broken (budget)  |
| `eager` | `init` queues a command               | Malicious        |

It was built from `fixture/` (a crate outside the workspace, against the
real `wit/plugin.wit`):

```bash
cd crates/mandara-plugins/tests/data/vet/fixture
cargo build --release --locked --target wasm32-wasip2
cp target/wasm32-wasip2/release/vet_fixture.wasm ../fixture.wasm
```

Rebuild it whenever `wit/plugin.wit` changes.
