import { createContext, useCallback, useContext, useEffect, useMemo, type ReactNode } from "react";

import { getLocale, setLocale, type Locale } from "@/paraglide/runtime.js";

type I18nContextValue = {
  locale: Locale;
  setLocale: (locale: Locale) => void;
};

const I18nContext = createContext<I18nContextValue | null>(null);

function syncDocumentLang(locale: Locale) {
  document.documentElement.lang = locale;
}

type I18nProviderProps = {
  children: ReactNode;
};

/**
 * Paraglide 的 locale 存在全局 runtime 中；切换语言时直接 reload 页面，
 * 因此 React 侧不再维护额外镜像 state，避免 effect 内同步 setState。
 * - `setLocale(..., { reload: false })`：切换语言时不刷新页面
 * - `strategy: ["localStorage", "preferredLanguage", "baseLocale"]`：首次按系统语言，手动切换后持久化
 */
export function I18nProvider({ children }: I18nProviderProps) {
  const locale = getLocale();

  const setAppLocale = useCallback((next: Locale) => {
    // Paraglide 消息函数在编译时绑定，切换语言需刷新页面才能生效
    setLocale(next, { reload: true });
  }, []);

  useEffect(() => {
    syncDocumentLang(locale);
  }, [locale]);

  const value = useMemo(() => ({ locale, setLocale: setAppLocale }), [locale, setAppLocale]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n() {
  const value = useContext(I18nContext);
  if (!value) {
    throw new Error("useI18n 必须在 I18nProvider 内使用");
  }
  return value;
}
