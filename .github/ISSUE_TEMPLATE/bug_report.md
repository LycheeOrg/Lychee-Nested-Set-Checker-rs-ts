---
name: Bug report
about: Something in nested-set-checker-wasm isn't working as expected
title: ''
labels: bug
assignees: ''
---

**Describe the bug**
A clear and concise description of what's wrong.

**To reproduce**
```ts
import init, { prepareAlbums } from "@lychee-org/nested-set-checker-wasm";

await init();
const result = prepareAlbums({
	id: ["..."],
	title: ["..."],
	parent_id: [null],
	lft: Int32Array.from([1]),
	rgt: Int32Array.from([2]),
});
```
What did you expect `result` to look like, and what did you actually get?

**Environment**
- `@lychee-org/nested-set-checker-wasm` version:
- Runtime: (browser + version / Node version)
- Bundler (if any):

**Additional context**
Add any other context about the problem here.
