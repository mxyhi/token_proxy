import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

/**
 * Vitest 全局初始化：
 * - 统一 mock 掉 Tauri 相关模块，让测试可以在非 Tauri 环境运行（node/jsdom）。
 * - 补齐 jsdom 缺失的 Web API，避免 Radix/Theme 等 UI 依赖在测试中报错。
 *
 * 约束：
 * - 禁止 any：所有 mock 尽量给出明确签名。
 * - mock 的返回值以“最小可用”为准，避免过度模拟导致维护成本上升。
 */

type UnknownRecord = Record<string, unknown>;

// ------------------------------
// Tauri mocks
// ------------------------------

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn<(command: string, args?: UnknownRecord) => Promise<unknown>>(),
}));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn<() => Promise<string>>().mockResolvedValue("0.0.0-test"),
}));

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: vi.fn<() => Promise<string>>().mockResolvedValue("/"),
  join: vi
    .fn<(...segments: string[]) => Promise<string>>()
    .mockImplementation(async (...segments) => segments.join("/")),
}));

vi.mock("@tauri-apps/plugin-autostart", () => ({
  enable: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
  disable: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
  isEnabled: vi.fn<() => Promise<boolean>>().mockResolvedValue(false),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn<(url: string) => Promise<void>>().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn<(options?: unknown) => Promise<unknown>>().mockResolvedValue(null),
}));

vi.mock("@tauri-apps/plugin-deep-link", () => ({
  onOpenUrl: vi
    .fn<(handler: (urls: string[]) => void) => Promise<() => void>>()
    .mockResolvedValue(() => undefined),
}));

vi.mock("@tauri-apps/plugin-process", () => ({
  relaunch: vi.fn<() => Promise<void>>().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/plugin-updater", () => ({
  check: vi.fn<() => Promise<unknown>>().mockResolvedValue(null),
}));

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: vi.fn<(text: string) => Promise<void>>().mockResolvedValue(undefined),
  readText: vi.fn<() => Promise<string>>().mockResolvedValue(""),
}));

// ------------------------------
// jsdom polyfills
// ------------------------------

function createMockStorage(): Storage {
  const store = new Map<string, string>();

  return {
    get length() {
      return store.size;
    },
    clear() {
      store.clear();
    },
    getItem(key) {
      return store.has(key) ? store.get(key) ?? null : null;
    },
    key(index) {
      return Array.from(store.keys())[index] ?? null;
    },
    removeItem(key) {
      store.delete(key);
    },
    setItem(key, value) {
      store.set(String(key), String(value));
    },
  };
}

// Node 25 默认暴露了 `globalThis.localStorage`，但在未提供 `--localstorage-file` 时会输出 warning；
// Paraglide 的 locale 策略也会直接访问 localStorage。为保证测试“安静且稳定”，这里无条件覆盖为内存版实现。
const localStorageMock = createMockStorage();
Object.defineProperty(globalThis, "localStorage", { value: localStorageMock, configurable: true });
Object.defineProperty(window, "localStorage", { value: localStorageMock, configurable: true });

const sessionStorageMock = createMockStorage();
Object.defineProperty(globalThis, "sessionStorage", { value: sessionStorageMock, configurable: true });
Object.defineProperty(window, "sessionStorage", { value: sessionStorageMock, configurable: true });

function createMockMatchMedia(): Window["matchMedia"] {
  return (query) => {
    const noop = () => undefined;
    const noopWithReturnFalse = () => false;

    const addEventListener = vi.fn<MediaQueryList["addEventListener"]>(noop);
    const removeEventListener = vi.fn<MediaQueryList["removeEventListener"]>(noop);
    const dispatchEvent = vi.fn<MediaQueryList["dispatchEvent"]>(noopWithReturnFalse);

    // 兼容旧 API：很多库仍会调用 addListener/removeListener。
    const addListener = vi.fn<MediaQueryList["addListener"]>(noop);
    const removeListener = vi.fn<MediaQueryList["removeListener"]>(noop);

    const mediaQueryList: MediaQueryList = {
      matches: false,
      media: query,
      onchange: null,
      addListener,
      removeListener,
      addEventListener,
      removeEventListener,
      dispatchEvent,
    };

    return mediaQueryList;
  };
}

if (typeof window.matchMedia !== "function") {
  window.matchMedia = createMockMatchMedia();
}

if (typeof globalThis.ResizeObserver === "undefined") {
  class MockResizeObserver {
    constructor(_callback: ResizeObserverCallback) {}

    observe = vi.fn<ResizeObserver["observe"]>();
    unobserve = vi.fn<ResizeObserver["unobserve"]>();
    disconnect = vi.fn<ResizeObserver["disconnect"]>();
  }

  globalThis.ResizeObserver = MockResizeObserver;
}

if (typeof globalThis.IntersectionObserver === "undefined") {
  class MockIntersectionObserver {
    readonly root: Element | Document | null = null;
    readonly rootMargin = "";
    readonly thresholds: ReadonlyArray<number> = [];

    constructor(
      _callback: IntersectionObserverCallback,
      _options?: IntersectionObserverInit,
    ) {}

    observe = vi.fn<IntersectionObserver["observe"]>();
    unobserve = vi.fn<IntersectionObserver["unobserve"]>();
    disconnect = vi.fn<IntersectionObserver["disconnect"]>();
    takeRecords = vi.fn<IntersectionObserver["takeRecords"]>(() => []);
  }

  globalThis.IntersectionObserver = MockIntersectionObserver;
}
