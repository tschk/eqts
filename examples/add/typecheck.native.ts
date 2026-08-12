import * as bunFfi from "./dist/bun/index.js";
import * as denoFfi from "./dist/deno/index.js";
import * as koffi from "./dist/node-koffi/index.js";

const bunResult: number = bunFfi.add(20, 22);
const denoResult: number = denoFfi.add(20, 22);
const koffiResult: number = koffi.add(20, 22);
const bunBigint: bigint = bunFfi.addU64(9_007_199_254_740_992n, 1n);
const denoBigint: bigint = denoFfi.addI64(-9_007_199_254_740_992n, 1n);
const koffiPerson: koffi.Person = koffi.makePerson("Ada", 36);

if (
  bunResult !== denoResult ||
  denoResult !== koffiResult ||
  bunBigint !== 9_007_199_254_740_993n ||
  denoBigint !== -9_007_199_254_740_991n ||
  koffiPerson.name !== "Ada"
) {
  throw new Error("native backend parity failed");
}
