// Node 26 exposes a native `localStorage` getter that returns undefined (requires
// --localstorage-file flag). This shadows jsdom's implementation when vitest
// copies the jsdom window into globalThis. Restore jsdom's object here so that
// any module that calls localStorage.getItem/setItem/removeItem works in tests.
if (typeof localStorage === "undefined") {
  // jsdom stores its Storage instance on window._localStorage
  const jsdomStorage = (window as unknown as { _localStorage?: Storage })
    ._localStorage;
  if (jsdomStorage) {
    Object.defineProperty(globalThis, "localStorage", {
      get: () => jsdomStorage,
      configurable: true,
    });
  }
}
