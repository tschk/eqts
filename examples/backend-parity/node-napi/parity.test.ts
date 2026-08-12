import { expect, test } from "bun:test";
import { add } from "./dist/index.js";

test("add matches the shared API", () => {
  expect(add(20, 22)).toBe(42);
});
