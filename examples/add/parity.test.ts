import { describe, expect, test } from "bun:test";
import * as napi from "@tschk/eqts-example-add/node-napi";
import * as wasm from "@tschk/eqts-example-add/wasm";
import * as bunFfi from "@tschk/eqts-example-add/bun";
import * as browserWasm from "@tschk/eqts-example-add/wasm-browser";

for (const [name, backend] of Object.entries({ napi, wasm, bunFfi })) {
  describe(name, () => {
    test("exports shared values", () => {
      expect(backend.add(20, 22)).toBe(42);
      expect(backend.negate(true)).toBeFalse();
      expect(backend.addI64(9_007_199_254_740_993n, 2n)).toBe(
        9_007_199_254_740_995n,
      );
      expect(backend.addI64(-42n, 2n)).toBe(-40n);
      expect(backend.addU64(18_000_000_000_000_000_000n, 1n)).toBe(
        18_000_000_000_000_000_001n,
      );
      expect(backend.echoText("eqts")).toBe("eqts");
      expect(
        Array.from(backend.reverseBytes(new Uint8Array([1, 2, 3]))),
      ).toEqual([3, 2, 1]);
      expect(backend.sumValues([10, 12, 20])).toBe(42);
      expect(backend.maybeName(true)).toBe("eqts");
      expect(backend.maybeName(false)).toBeNull();
      expect(backend.maybeBytes(false)).toBeNull();
      expect(backend.maybeBytes(true)).toEqual(new Uint8Array([1, 2, 3]));
      expect(backend.flattenValues([[1, 2], [3]])).toEqual([1, 2, 3]);
      expect(backend.makePerson("Ada", 36)).toEqual({ name: "Ada", age: 36 });
      expect(backend.personOrError(true)).toEqual({ name: "Ada", age: 36 });
      expect(backend.currentStatus()).toBe("Ready");
    });

    test("preserves structured Rust errors", () => {
      try {
        backend.personOrError(false);
        throw new Error("expected personOrError to throw");
      } catch (error) {
        expect(error).toBeInstanceOf(backend.EqtsError);
        expect((error as InstanceType<typeof backend.EqtsError>).code).toBe(
          "RUST_ERROR",
        );
        expect((error as InstanceType<typeof backend.EqtsError>).value).toEqual(
          {
            name: "Ada",
            age: 36,
          },
        );
      }
    });

    test("turns Rust errors into JavaScript errors", () => {
      try {
        backend.checkedDivide(1, 0);
        throw new Error("expected checkedDivide to throw");
      } catch (error) {
        expect(error).toBeInstanceOf(backend.EqtsError);
        expect((error as InstanceType<typeof backend.EqtsError>).code).toBe(
          "RUST_ERROR",
        );
        expect((error as InstanceType<typeof backend.EqtsError>).value).toBe(
          "division by zero",
        );
      }
    });
  });
}

test("browser Wasm exports the canonical API after initialization", async () => {
  await browserWasm.initialize();
  expect(browserWasm.add(20, 22)).toBe(42);
  expect(browserWasm.addI64(40n, 2n)).toBe(42n);
  expect(browserWasm.addU64(40n, 2n)).toBe(42n);
  expect(browserWasm.currentStatus()).toBe("Ready");
  expect(browserWasm.makePerson("Ada", 36)).toEqual({ name: "Ada", age: 36 });
});
