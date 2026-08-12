import * as napi from "@tschk/eqts-example-add/node-napi";
import * as wasm from "@tschk/eqts-example-add/wasm";
import * as bunFfi from "./dist/bun/index.js";
import * as denoFfi from "./dist/deno/index.js";
import * as koffi from "./dist/node-koffi/index.js";

const napiResult: number = napi.add(20, 22);
const wasmResult: number = wasm.add(20, 22);
const napiPerson: napi.Person = napi.makePerson("Ada", 36);
const wasmPerson: wasm.Person = wasm.makePerson("Ada", 36);
const bunPerson: bunFfi.Person = bunFfi.makePerson("Ada", 36);
const denoPerson: denoFfi.Person = denoFfi.makePerson("Ada", 36);
const koffiPerson: koffi.Person = koffi.makePerson("Ada", 36);
const nativeBytes: Uint8Array = bunFfi.reverseBytes(new Uint8Array([1, 2, 3]));
const nativeOption: string | null = denoFfi.maybeName(true);
const nativeStatus: koffi.Status = koffi.currentStatus();

if (
  napiResult !== wasmResult ||
  napiPerson.name !== wasmPerson.name ||
  bunPerson.name !== denoPerson.name ||
  denoPerson.name !== koffiPerson.name ||
  nativeBytes.length !== 3 ||
  nativeOption !== "eqts" ||
  nativeStatus !== "Ready"
) {
  throw new Error("backend parity failed");
}
