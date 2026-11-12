import en from "./locales/en.json";
import de from "./locales/de.json";
import fr from "./locales/fr.json";
import es from "./locales/es.json";
import pl from "./locales/pl.json";
import type { AccessTier, LogEntry, SupportedLocale } from "./types";

export type TranslationKey = keyof typeof en;
type Dictionary = Record<TranslationKey, string>;

const DICTIONARIES: Record<SupportedLocale, Dictionary> = {
  en,
  de,
  fr,
  es,
  pl,
};

/**
 * 5-language fallback lookup for standard FS25 basegame fillTypes (`[de, en, fr, es, pl]`).
 * Ensures historic log lines and Mock simulation badges switch language dynamically on the fly.
 */
const BASEGAME_FILLTYPE_NAMES: Record<
  string,
  readonly [string, string, string, string, string]
> = {
  WHEAT: ["Weizen", "Wheat", "Blé", "Trigo", "Pszenica"],
  BARLEY: ["Gerste", "Barley", "Orge", "Cebada", "Jęczmień"],
  OAT: ["Hafer", "Oats", "Avoine", "Avena", "Owies"],
  CANOLA: ["Raps", "Canola", "Colza", "Colza", "Rzepak"],
  SORGHUM: ["Sorghumhirse", "Sorghum", "Sorgho", "Sorgo", "Sorgo"],
  SUNFLOWER: ["Sonnenblumen", "Sunflowers", "Tournesol", "Girasoles", "Słonecznik"],
  SOYBEAN: ["Sojabohnen", "Soybeans", "Soja", "Soja", "Soja"],
  CORN: ["Mais", "Corn", "Maïs", "Maíz", "Kukurydza"],
  MAIZE: ["Mais", "Corn", "Maïs", "Maíz", "Kukurydza"],
  POTATO: ["Kartoffeln", "Potatoes", "Pommes de terre", "Patatas", "Ziemniaki"],
  SUGARBEET: ["Zuckerrüben", "Sugar Beet", "Betterave sucrière", "Remolacha azucarera", "Buraki cukrowe"],
  SUGARCANE: ["Zuckerrohr", "Sugarcane", "Canne à sucre", "Caña de azúcar", "Trzcina cukrowa"],
  COTTON: ["Baumwolle", "Cotton", "Coton", "Algodón", "Bawełna"],
  OLIVE: ["Oliven", "Olives", "Olives", "Aceitunas", "Oliwki"],
  GRAPE: ["Trauben", "Grapes", "Raisins", "Uvas", "Winogrona"],
  RICE: ["Reis", "Rice", "Riz", "Arroz", "Ryż"],
  LONGGRAINRICE: ["Langkornreis", "Long Grain Rice", "Riz long grain", "Arroz de grano largo", "Ryż długoziarnisty"],
  PEAS: ["Erbsen", "Peas", "Petits pois", "Guisantes", "Groch"],
  SPINACH: ["Spinat", "Spinach", "Épinards", "Espinacas", "Szpinak"],
  GREENBEAN: ["Grüne Bohnen", "Green Beans", "Haricots verts", "Judías verdes", "Fasolka szparagowa"],
  SILAGE: ["Silage", "Silage", "Ensilage", "Ensilaje", "Kiszonka"],
  STRAW: ["Stroh", "Straw", "Paille", "Paja", "Słoma"],
  DRYGRASS_WINDROW: ["Heu", "Hay", "Foin", "Heno", "Siano"],
  HAY: ["Heu", "Hay", "Foin", "Heno", "Siano"],
  GRASS_WINDROW: ["Gras", "Grass", "Herbe", "Hierba", "Trawa"],
  GRASS: ["Gras", "Grass", "Herbe", "Hierba", "Trawa"],
  WOODCHIPS: ["Hackschnitzel", "Wood Chips", "Copeaux de bois", "Astillas de madera", "Zrębki"],
  MILK: ["Milch", "Milk", "Lait", "Leche", "Mleko"],
  EGG: ["Eier", "Eggs", "Œufs", "Huevos", "Jajka"],
  WOOL: ["Wolle", "Wool", "Laine", "Lana", "Wełna"],
  HONEY: ["Honig", "Honey", "Miel", "Miel", "Miód"],
  FLOUR: ["Mehl", "Flour", "Farine", "Harina", "Mąka"],
  BREAD: ["Brot", "Bread", "Pain", "Pan", "Chleb"],
  SUGAR: ["Zucker", "Sugar", "Sucre", "Azúcar", "Cukier"],
  BOARDS: ["Bretter", "Planks", "Planches", "Tablones", "Deski"],
};

export const LANGUAGE_OPTIONS: ReadonlyArray<{
  code: SupportedLocale;
  shortLabel: string;
  nativeName: string;
}> = [
  { code: "de", shortLabel: "DE", nativeName: "Deutsch" },
  { code: "en", shortLabel: "EN", nativeName: "English" },
  { code: "fr", shortLabel: "FR", nativeName: "Français" },
  { code: "es", shortLabel: "ES", nativeName: "Español" },
  { code: "pl", shortLabel: "PL", nativeName: "Polski" },
];

let activeLocale: SupportedLocale = "de";

export function normalizeLocale(raw: string | null | undefined): SupportedLocale {
  const short = (raw ?? "en").trim().toLowerCase().split(/[-_]/)[0];
  if (short === "de" || short === "en" || short === "fr" || short === "es" || short === "pl") {
    return short;
  }
  return "en";
}

export function setLocale(raw: string): SupportedLocale {
  activeLocale = normalizeLocale(raw);
  document.documentElement.lang = activeLocale;
  return activeLocale;
}

export function getLocale(): SupportedLocale {
  return activeLocale;
}

/**
 * Translates a key using the active locale with guaranteed fallback to English (`en`).
 * Supports `{placeholder}` variable interpolation.
 */
export function t(
  key: TranslationKey,
  vars?: Record<string, string | number>,
): string {
  const dict = DICTIONARIES[activeLocale] ?? DICTIONARIES.en;
  let template = dict[key] ?? DICTIONARIES.en[key] ?? key;

  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      template = template.replaceAll(`{${k}}`, String(v));
    }
  }
  return template;
}

/** Localizes a basegame FS25 fillType code (`WHEAT` -> `Weizen`/`Wheat`/`Blé`/`Trigo`/`Pszenica`). */
export function localizeFillTypeCode(code: string): string | null {
  const row = BASEGAME_FILLTYPE_NAMES[code.trim().toUpperCase()];
  if (!row) return null;
  switch (activeLocale) {
    case "de":
      return row[0];
    case "fr":
      return row[2];
    case "es":
      return row[3];
    case "pl":
      return row[4];
    default:
      return row[1];
  }
}

export function formatAccessTierName(tier: AccessTier | string): string {
  switch (tier) {
    case "developer":
      return t("tierNameDeveloper");
    case "extended":
      return t("tierNameExtended");
    default:
      return t("tierNameUser");
  }
}

/**
 * Renders a `LogEntry` in the currently selected UI language, re-evaluating crop codes
 * and tier labels dynamically whenever the user switches languages.
 */
export function formatLogEntryMessage(entry: LogEntry): string {
  if (!entry.key || !(entry.key in DICTIONARIES.en)) {
    return entry.message;
  }

  const vars: Record<string, string> = { ...(entry.params ?? {}) };

  if (vars.itemCode) {
    const localized = localizeFillTypeCode(vars.itemCode);
    if (localized) {
      vars.item = localized;
    }
  }
  if (vars.tier) {
    vars.tier = formatAccessTierName(vars.tier);
  }

  return t(entry.key as TranslationKey, vars);
}
