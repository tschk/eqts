import { describe, expect, test } from "bun:test";
import * as bunFfi from "@tschk/eqts-example-add/bun";
import * as napi from "@tschk/eqts-example-add/node-napi";
import * as wasm from "@tschk/eqts-example-add/wasm";
import * as browserWasm from "@tschk/eqts-example-add/wasm-browser";

type Backend = typeof napi;

type Greeter = {
  greet(name: string): string;
  dispose(): void;
};

function reactiveSuite(name: string, backend: Backend) {
  describe(name, () => {
    test("handle disposal is idempotent and observable", async () => {
      const handle = backend.words();
      expect(handle.disposed).toBeFalse();
      expect(await handle.next()).toEqual({ value: "one", done: false });
      handle.dispose();
      handle.dispose();
      expect(handle.disposed).toBeTrue();
      expect(handle.next()).rejects.toMatchObject({
        code: "USE_AFTER_DISPOSE",
      });
    });

    test("AbortSignal cancels pending work with its reason", async () => {
      const controller = new AbortController();
      const reason = new DOMException("stop", "AbortError");
      const pending = backend.pendingTask({ signal: controller.signal });
      controller.abort(reason);
      expect(pending).rejects.toBe(reason);
    });

    test("object methods preserve mutable state and cancel async work", async () => {
      const counter = backend.counter(40);
      expect(counter.current()).toBe(40);
      expect(counter.increment(2)).toBe(42);
      expect(counter.current()).toBe(42);

      const controller = new AbortController();
      const reason = new DOMException("stop method", "AbortError");
      const pending = counter.pending({ signal: controller.signal });
      controller.abort(reason);
      expect(pending).rejects.toBe(reason);
      counter.dispose();
      expect(() => counter.current()).toThrow();
    });

    test("trait exports are structurally usable", () => {
      const greeter: Greeter = backend.greeter("hello");
      expect(greeter.greet("eqts")).toBe("hello eqts");
      greeter.dispose();
    });

    test("AsyncIterable enforces one outstanding demand and return cancellation", async () => {
      const values = backend.numbers();
      const first = values.next();
      expect(values.next()).rejects.toMatchObject({ code: "CONCURRENT_NEXT" });
      expect(await first).toEqual({ value: 1, done: false });
      expect(await values.return?.()).toEqual({ value: undefined, done: true });
      expect(values.disposed).toBeTrue();
      expect(values.next()).rejects.toMatchObject({
        code: "USE_AFTER_DISPOSE",
      });
    });

    test("callbacks run in production order on the JavaScript thread", async () => {
      const calls: string[] = [];
      const events = backend.callbackEvents((value) => {
        calls.push(value);
      });
      while (calls.length < 2) {
        await Bun.sleep(0);
      }
      expect(calls).toEqual(["first", "second"]);
      events.dispose();
    });
  });
}

reactiveSuite("napi reactive", napi);
reactiveSuite("wasm reactive", wasm);
reactiveSuite("bun ffi reactive", bunFfi);

test("browser Wasm reactive surface initializes", async () => {
  await browserWasm.initialize();
  const words = browserWasm.words();
  expect(await words.next()).toEqual({ value: "one", done: false });
  expect(await words.return?.()).toEqual({ value: undefined, done: true });
});
