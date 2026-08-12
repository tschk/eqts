import { add } from "./dist/index.js";

const result: number = add(20, 22);

if (result !== 42) {
  throw new Error(`expected 42, received ${result}`);
}
