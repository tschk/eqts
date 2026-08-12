import { describe, expect, test } from "bun:test";
import * as napi from "@tschk/eqts-example-add/node-napi";
import * as wasm from "@tschk/eqts-example-add/wasm";

for (const [name, backend] of Object.entries({ napi, wasm })) {
  describe(name, () => {
    test("exports shared values", () => {
      expect(backend.add(20, 22)).toBe(42);
      expect(backend.echoText("eqts")).toBe("eqts");
      expect(Array.from(backend.reverseBytes(new Uint8Array([1, 2, 3])))).toEqual([3, 2, 1]);
      expect(backend.sumValues(new Uint32Array([10, 12, 20]))).toBe(42);
      expect(backend.maybeName(true)).toBe("eqts");
      expect(backend.maybeName(false) == null).toBeTrue();
      expect(backend.makePerson("Ada", 36)).toMatchObject({ name: "Ada", age: 36 });
      expect(backend.currentStatus()).toBe(backend.Status.Ready);
    });

    test("turns Rust errors into JavaScript errors", () => {
      expect(() => backend.checkedDivide(1, 0)).toThrow("division by zero");
    });
  });
}
