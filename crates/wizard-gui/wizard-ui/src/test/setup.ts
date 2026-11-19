import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// Node 25 also exposes Web Storage; tests must use this JSDOM document's storage.
const testWindow = (
  globalThis as typeof globalThis & { jsdom: { window: Window } }
).jsdom.window;
vi.stubGlobal("localStorage", testWindow.localStorage);
vi.stubGlobal("sessionStorage", testWindow.sessionStorage);

// JSDOM tests behavior; viewport geometry is verified in the real browser.
Object.defineProperty(window, "matchMedia", {
  writable: true,
  value: vi.fn((media: string) => ({
    matches: false,
    media,
    onchange: null,
    addListener() {},
    removeListener() {},
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent() {
      return false;
    },
  })),
});
class ResizeObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
}
class IntersectionObserverMock {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() {
    return [];
  }
}
vi.stubGlobal("ResizeObserver", ResizeObserverMock);
vi.stubGlobal("IntersectionObserver", IntersectionObserverMock);
window.scrollTo = () => {};
Element.prototype.scrollIntoView = () => {};
Element.prototype.getAnimations = () => [];
