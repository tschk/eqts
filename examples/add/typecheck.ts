import * as napi from "@tschk/eqts-example-add/node-napi";
import * as wasm from "@tschk/eqts-example-add/wasm";

const napiResult: number = napi.add(20, 22);
const wasmResult: number = wasm.add(20, 22);
const napiPerson: napi.Person = napi.makePerson("Ada", 36);
const wasmPerson: wasm.Person = wasm.makePerson("Ada", 36);

if (napiResult !== wasmResult || napiPerson.name !== wasmPerson.name) {
  throw new Error("backend parity failed");
}
