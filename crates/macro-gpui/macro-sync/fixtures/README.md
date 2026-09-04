# Macro collaboration fixtures

These fixtures contain synthetic data only. They pin Macro's JavaScript/WASM
`loro-crdt` 1.13.7 wire output and the matching Rust `loro` 1.13.7 behavior.

To verify the recorded cross-runtime convergence:

```sh
cd crates/macro-gpui/macro-sync/fixtures
npm ci --ignore-scripts --no-audit --no-fund
node verify_markdown_two_client.mjs
```

To verify the currently compiled Rust implementation rather than only the
recorded bytes, generate an update and pass it directly to the same Macro
JavaScript/WASM runtime:

```sh
native_update_hex="$(cargo run -q -p macro_sync --example generate_native_markdown_update)"
node crates/macro-gpui/macro-sync/fixtures/verify_markdown_two_client.mjs \
  "--native-update-hex=${native_update_hex}"
```

To regenerate the two-client fixture, first run
`node generate_markdown_two_client.mjs` and copy its seed/browser fields into
`markdown_two_client_typescript.json`. Then generate the Rust branch:

```sh
cargo run -p macro_sync --example generate_native_markdown_update
```

Copy that hex into `native_update_hex`, then run
`node verify_markdown_two_client.mjs --emit-converged` and copy its output into
`expected_converged`. Finally run the verifier without the flag to prove the
checked-in expectation matches JavaScript/WASM exactly.
The bytes include recorded change timestamps, so their headers may differ after
regeneration; semantic convergence and stable Lexical IDs are the contract.
