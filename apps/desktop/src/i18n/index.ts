import enUS from "./en-US.json";
import zhCN from "./zh-CN.json";

/**
 * The two shipped locales. A third one is a JSON file plus one line here —
 * every string in the interface goes through this module, including error
 * copy and context menus.
 */
export type Locale = "zh-CN" | "en-US";

/** What the user picked in Settings → Language. */
export type LanguagePreference = "system" | Locale;

export const LOCALES: Locale[] = ["zh-CN", "en-US"];

const bundles: Record<Locale, Record<string, string>> = {
  "zh-CN": zhCN as Record<string, string>,
  "en-US": enUS as Record<string, string>,
};

/** The browser's preferred locale, mapped onto a shipped bundle. */
export function detectSystemLocale(): Locale {
  const candidates = [navigator.language, ...(navigator.languages ?? [])];
  for (const candidate of candidates) {
    if (!candidate) continue;
    if (candidate.toLowerCase().startsWith("zh")) return "zh-CN";
  }
  return "en-US";
}

/** Turn the stored preference into a concrete locale. */
export function resolveLocale(preference: LanguagePreference): Locale {
  return preference === "system" ? detectSystemLocale() : preference;
}

/** Substitute `{name}` placeholders. */
function interpolate(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, key: string) => {
    const value = params[key];
    return value === undefined ? match : String(value);
  });
}

/**
 * Look a key up, falling back to English and then to the key itself so a
 * missing translation is visible but never breaks the interface.
 */
export function translate(
  locale: Locale,
  key: string,
  params?: Record<string, string | number>,
): string {
  const template = bundles[locale][key] ?? bundles["en-US"][key] ?? key;
  return interpolate(template, params);
}

/** A translator bound to one locale. */
export type Translator = (key: string, params?: Record<string, string | number>) => string;

export function createTranslator(locale: Locale): Translator {
  return (key, params) => translate(locale, key, params);
}

/** Whether every key in the English bundle also exists in `locale`. */
export function missingKeys(locale: Locale): string[] {
  return Object.keys(bundles["en-US"]).filter((key) => !(key in bundles[locale]));
}