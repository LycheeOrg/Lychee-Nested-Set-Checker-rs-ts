<!--
Thank you for contributing to nested-set-checker-wasm! We only accept PRs against the `main` branch.

Please describe the benefit to consumers of the package, and why it doesn't break
the published API/types.

If you're pushing a Feature:
- Title it: "This new feature"
- Describe what the new feature enables
- Add Rust tests (`cargo test`) covering the new behavior
- Ensure it doesn't break any existing features or the published `.d.ts` types

If you're pushing a Fix:
- Title it: "Fixes the bug name"
- If it fixes an existing issue, start the description with `fixes #xxxx`
- Describe how it fixes the bug in a few words
- Add a test that would have caught the bug before your fix

All Pull Requests run `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`,
a `wasm32-unknown-unknown` build, and a Node smoke test against the built package.
You can run all of these locally from `rust/` before pushing — see the
[README's Development section](../README.md#development).
-->
