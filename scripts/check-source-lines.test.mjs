import assert from "node:assert/strict";
import test from "node:test";

import { countPhysicalLines, overBudgetEntries } from "./check-source-lines.mjs";

test("physical line count treats an empty file as zero lines", () => {
  assert.equal(countPhysicalLines(Buffer.alloc(0)), 0);
});

for (const finalNewline of [true, false]) {
  test(`physical line count handles 999 lines with final newline=${finalNewline}`, () => {
    assert.equal(countPhysicalLines(lines(999, finalNewline)), 999);
  });

  test(`physical line count handles 1000 lines with final newline=${finalNewline}`, () => {
    assert.equal(countPhysicalLines(lines(1000, finalNewline)), 1000);
  });
}

test("source line guard reports real files at the 1000 line boundary", () => {
  const entries = [
    { lines: countPhysicalLines(lines(999, false)), path: "crates/ok.rs" },
    { lines: countPhysicalLines(lines(1000, false)), path: "crates/too-large.rs" },
  ];

  assert.deepEqual(overBudgetEntries(entries), [
    { lines: 1000, path: "crates/too-large.rs" },
  ]);
});

function lines(count, finalNewline) {
  const content = Array.from({ length: count }, () => "line").join("\n");
  return Buffer.from(finalNewline && count > 0 ? `${content}\n` : content);
}
