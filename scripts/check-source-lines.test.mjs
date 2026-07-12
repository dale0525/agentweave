import assert from "node:assert/strict";
import test from "node:test";

import { overBudgetEntries, parseWcOutput } from "./check-source-lines.mjs";

test("source line parser ignores wc aggregate totals", () => {
  const entries = parseWcOutput(`   12 crates/example.rs
 1200 total
`);

  assert.deepEqual(entries, [{ lines: 12, path: "crates/example.rs" }]);
  assert.deepEqual(overBudgetEntries(entries), []);
});

test("source line guard reports real files at the 1000 line boundary", () => {
  const entries = parseWcOutput(`  999 crates/ok.rs
 1000 crates/too-large.rs
 1999 total
`);

  assert.deepEqual(overBudgetEntries(entries), [
    { lines: 1000, path: "crates/too-large.rs" },
  ]);
});
