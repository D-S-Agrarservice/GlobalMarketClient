export type SupportedLocale =
  | "de"
  | "en"
  | "fr"
  | "es"
  | "pl"
  | "it"
  | "nl"
  | "cz"
  | "pt"
  | "hu"
  | "da"
  | "sv"
  | "ro"
  | "tr"
  | "ru"
  | "uk";

export type BackendMode = "mock" | "live";

export type ThemePreference = "dark" | "light" | "system";

export type AccessTier = "user" | "extended" | "developer";

export type BridgeConnectionState =
  | "pathMissing"
  | "disconnected"
  | "connecting"
  | "online"
  | "backendOffline"
  | "outdated";

export type LogLevel = "info" | "success" | "warn" | "error";

export interface AppInfo {
  appVersion: string;
  protocolVersion: string;
  isBeta: boolean;
  platform: string;
}

export interface SavedTokenProfile {
  id: string;
  customLabel: string | null;
  detectedMarketName: string | null;
  brandingTier: number | null;
  isPrivateMarket: boolean;
  maskedHint: string;
  cachedLogoFile: string | null;
  cachedLogoUrl: string | null;
}

export interface AppConfig {
  clientId?: string;
  accessTier: AccessTier;
  fs25ProfilePath: string | null;
  language: string;
  theme: ThemePreference;
  minimizeToTray: boolean;
  notificationsEnabled: boolean;
  logAutoScroll: boolean;
  /** Official D-S-Agrarservice logo in the FS25 market menu (custom logos are unaffected). */
  showOfficialLogo: boolean;
  autoConnectOnLaunch: boolean;
  backendMode: BackendMode;
  backendUrl: string;
  activeTokenId: string | null;
}

export interface BridgeSnapshot {
  state: BridgeConnectionState;
  connectionEnabled: boolean;
  gameEdition: string | null;
  accessTier: AccessTier;
  backendMode: BackendMode;
  backendUrl: string;
  apiVersion: string | null;
  marketName: string;
  brandingTier: number;
  isPrivateMarket: boolean;
  marketLogoUrl: string | null;
  marketLogoFile: string | null;
  activeTokenId: string | null;
  savedTokens: SavedTokenProfile[];
  profilePath: string | null;
  bridgePath: string | null;
  exeVersion: string;
  minExeVersion: string;
  latestExeVersion: string | null;
  contractsSupported: boolean;
  openContractsCount: number;
  openOffersCount: number;
  vehiclesSupported: boolean;
  openVehicleListingsCount: number;
  approvedCount: number;
  pendingCount: number;
  rejectedCount: number;
  readQueueCount: number;
  writeQueueCount: number;
  activeGreatDemands: string[];
  activeGreatDemandCodes: string[];
  heartbeatSeq: number;
  lastHeartbeatUnix: number | null;
  lastSyncUnix: number | null;
  lastError: string | null;
  /** Unix time (seconds) when the temporary Extended mode ends, `null` outside Extended mode. */
  extendedExpiresUnix: number | null;
  /** Current server notice (maintenance, restart or info; Spec Section 3.1.3). */
  serverNotice: ServerNotice | null;
  /** `maintenance`, `unreachable` or `unsupportedContent` while the backend is offline. */
  offlineReason: OfflineReason | null;
  /** Identity of the current savegame on the shared online pool. */
  serverIdentity: "unknown" | "known" | "rejected";
  /** A developer token for the development server (`X-Dev-Token`) is stored; the token itself never reaches the WebView. */
  hasDevToken: boolean;
  /** `server_env` of the last successful handshake (API 2.10+), `null` while offline or for older backends. */
  serverEnv: "production" | "development" | null;
  /** Loaded map from `modInfo.xml` (map id, map mod, short hash), `null` before mod v0.11.0.43. */
  loadedMap: string | null;
  /** Name of the refused content from a `CONTENT_NOT_SUPPORTED` answer, if the backend sent one. */
  unsupportedContentLabel: string | null;
}

export type ServerNoticeType = "maintenance" | "restart" | "info";
export type OfflineReason = "maintenance" | "unreachable" | "unsupportedContent";

/** Server notice from `/api/v1/meta/status` (Spec Section 3.1.3). Times are UTC unix seconds. */
export interface ServerNotice {
  id: string;
  noticeType: ServerNoticeType;
  startsAt: number | null;
  endsAt: number | null;
  /** Optional extra text per language code (`de`, `en`, ...). */
  message: Record<string, string>;
}

export interface HeartbeatTickPayload {
  seq: number;
  timestampUnix: number;
}

export interface LogEntry {
  timestampMs: number;
  level: LogLevel;
  message: string;
  key?: string;
  params?: Record<string, string>;
}

export interface GreatDemandNotification {
  fillType: string;
  title: string;
  bonusPercent: number;
}

/** Official production backend server URL. */
export const OFFICIAL_PROD_BACKEND_URL = "https://gm.bydora.de";

/** Official development / staging backend server URL (Developer tier). */
export const OFFICIAL_DEV_BACKEND_URL = "https://dev.gm.bydora.de";

/** Official dedicated CDN / image origin for Community & Logo keys (`market_logo_url`). */
export const OFFICIAL_LOGO_CDN_URL = "https://img.gm.bydora.de";

/** Local loopback FastAPI server URL preset (Developer tier). */
export const LOCAL_DEV_BACKEND_URL = "http://127.0.0.1:8000";

/** Permanent invite to the D-S-Agrarservice Discord server, landing in the `gm-info` channel. */
export const DSA_DISCORD_URL = "https://discord.gg/K382CfppjF";

/** Hosting partner website shown as "powered by bydora.de" in the header. */
export const BYDORA_URL = "https://bydora.de";

/** Official GitHub repository URL (D-S-Agrarservice). */
export const GITHUB_REPO_URL =
  "https://github.com/D-S-Agrarservice/GlobalMarketClient";

/** GitHub Releases page for manual / fallback updates (D-S-Agrarservice). */
export const GITHUB_RELEASES_URL =
  "https://github.com/D-S-Agrarservice/GlobalMarketClient/releases";

export interface CreditMember {
  name: string;
  roleKey?:
    | "creditsRoleKastor"
    | "creditsRoleTensuko"
    | "creditsRoleFarmsimTim";
}

/** Tier 1: Core modding & QA team directly supporting the project. */
export const CREDITS_CORE_TEAM: ReadonlyArray<CreditMember> = [
  { name: "Kastor", roleKey: "creditsRoleKastor" },
  { name: "Murphy" },
  { name: "DSA-Testteam" },
];

/** Tier 2: Concept ideas & external consulting. */
export const CREDITS_ADVISORY: ReadonlyArray<CreditMember> = [
  { name: "Tensuko", roleKey: "creditsRoleTensuko" },
  { name: "Farmsim Tim", roleKey: "creditsRoleFarmsimTim" },
];

/** Tier 3: Field testing, suggestions, and community support. */
export const CREDITS_FEEDBACK_TESTERS: ReadonlyArray<string> = [
  "Flamy",
  "PascalE",
  "WeekendFarmers-Testcrew",
  "OMG_its_Breide",
  "OnkelSander83",
];

export interface OpenSourcePackage {
  name: string;
  author: string;
  license: string;
  url: string;
}

/** Direct open-source libraries & frameworks credited in the Open-Source Notices modal. */
export const OPEN_SOURCE_PACKAGES: ReadonlyArray<OpenSourcePackage> = [
  {
    name: "Tauri v2 & Official Plugins",
    author: "The Tauri Programme within The Commons Conservancy",
    license: "MIT OR Apache-2.0",
    url: "https://github.com/tauri-apps/tauri",
  },
  {
    name: "Tokio Async Runtime",
    author: "Tokio Contributors",
    license: "MIT",
    url: "https://github.com/tokio-rs/tokio",
  },
  {
    name: "Reqwest HTTP Client",
    author: "Sean McArthur & Contributors",
    license: "MIT OR Apache-2.0",
    url: "https://github.com/seanmonstar/reqwest",
  },
  {
    name: "Rustls TLS Stack",
    author: "Joe Birr-Pixton & Rustls Contributors",
    license: "Apache-2.0 OR ISC OR MIT",
    url: "https://github.com/rustls/rustls",
  },
  {
    name: "Serde & serde_json",
    author: "Erick Tryzelaar & David Tolnay",
    license: "MIT OR Apache-2.0",
    url: "https://github.com/serde-rs/serde",
  },
  {
    name: "quick-xml",
    author: "Johann Tuffe & Contributors",
    license: "MIT",
    url: "https://github.com/tafia/quick-xml",
  },
  {
    name: "image & image-webp",
    author: "Image-rs Contributors",
    license: "MIT OR Apache-2.0",
    url: "https://github.com/image-rs/image",
  },
  {
    name: "RustCrypto sha2",
    author: "RustCrypto Contributors",
    license: "MIT OR Apache-2.0",
    url: "https://github.com/RustCrypto/hashes",
  },
  {
    name: "Notify File Watcher",
    author: "Felix Passenberg & Notify Contributors",
    license: "CC0-1.0",
    url: "https://github.com/notify-rs/notify",
  },
  {
    name: "thiserror",
    author: "David Tolnay & Contributors",
    license: "MIT OR Apache-2.0",
    url: "https://github.com/dtolnay/thiserror",
  },
  {
    name: "Tailwind CSS v4",
    author: "Tailwind Labs, Inc.",
    license: "MIT",
    url: "https://github.com/tailwindlabs/tailwindcss",
  },
  {
    name: "Vite",
    author: "VoidZero Inc. & Vite Contributors",
    license: "MIT",
    url: "https://github.com/vitejs/vite",
  },
  {
    name: "TypeScript",
    author: "Microsoft Corporation",
    license: "Apache-2.0",
    url: "https://github.com/microsoft/TypeScript",
  },
];
