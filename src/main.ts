import "./styles.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open as openFolderDialog } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
} from "@tauri-apps/plugin-notification";
import { openUrl } from "@tauri-apps/plugin-opener";
import dsaLogoUrl from "./assets/dsa-logo.png";
import {
  formatAccessTierName,
  formatLogEntryMessage,
  getLocale,
  LANGUAGE_OPTIONS,
  localizeFillTypeCode,
  setLocale,
  t,
} from "./i18n";
import {
  type AccessTier,
  type AppConfig,
  type AppInfo,
  type BackendMode,
  type BridgeConnectionState,
  type BridgeSnapshot,
  CREDITS_ADVISORY,
  CREDITS_CORE_TEAM,
  CREDITS_FEEDBACK_TESTERS,
  DSA_DISCORD_URL,
  GITHUB_RELEASES_URL,
  GITHUB_REPO_URL,
  type GreatDemandNotification,
  LOCAL_DEV_BACKEND_URL,
  type LogEntry,
  OFFICIAL_DEV_BACKEND_URL,
  OFFICIAL_PROD_BACKEND_URL,
  OPEN_SOURCE_PACKAGES,
} from "./types";

const MAX_UI_LOGS = 250;
const SECRET_CLICK_THRESHOLD = 5;
const SECRET_CLICK_WINDOW_MS = 4000;
const squareLogoCache = new Map<string, boolean>([[dsaLogoUrl, false]]);

type ActiveModal = "none" | "settings" | "credits" | "licenses" | "access";

interface UiState {
  info: AppInfo;
  config: AppConfig;
  snapshot: BridgeSnapshot;
  logs: LogEntry[];
  activeModal: ActiveModal;
  pathHelpOpen: boolean;
  accessKeyError: string | null;
  mockToolsExpanded: boolean;
  mockSimulatedOffline: boolean;
  mockLockedMinVersion: boolean;
  copiedDiagFeedback: boolean;
  toastMessage: string | null;
}

const state: UiState = {
  info: {
    appVersion: "1.0.0",
    protocolVersion: "1.0.0",
    platform: "desktop",
  },
  config: {
    accessTier: "user",
    fs25ProfilePath: null,
    language: "de",
    minimizeToTray: true,
    notificationsEnabled: true,
    logAutoScroll: true,
    backendMode: "live",
    backendUrl: OFFICIAL_PROD_BACKEND_URL,
    marketToken: null,
  },
  snapshot: {
    state: "connecting",
    gameEdition: null,
    accessTier: "user",
    backendMode: "live",
    backendUrl: OFFICIAL_PROD_BACKEND_URL,
    apiVersion: null,
    marketName: "Global Market",
    marketLogoUrl: null,
    profilePath: null,
    bridgePath: null,
    exeVersion: "1.0.0",
    minExeVersion: "1.0.0",
    approvedCount: 0,
    pendingCount: 0,
    rejectedCount: 0,
    readQueueCount: 0,
    writeQueueCount: 0,
    activeGreatDemands: [],
    activeGreatDemandCodes: [],
    lastSyncUnix: null,
    lastError: null,
  },
  logs: [],
  activeModal: "none",
  pathHelpOpen: false,
  accessKeyError: null,
  mockToolsExpanded: true,
  mockSimulatedOffline: false,
  mockLockedMinVersion: false,
  copiedDiagFeedback: false,
  toastMessage: null,
};

let toastTimer: number | undefined;
let copyFeedbackTimer: number | undefined;
let versionClickCount = 0;
let versionClickResetTimer: number | undefined;

function escapeHtml(raw: string): string {
  return raw
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function formatLocalTime(timestampMs: number): string {
  const date = new Date(timestampMs);
  const hh = String(date.getHours()).padStart(2, "0");
  const mm = String(date.getMinutes()).padStart(2, "0");
  const ss = String(date.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

function showToast(msg: string): void {
  state.toastMessage = msg;
  updateToastBanner();
  if (toastTimer !== undefined) {
    window.clearTimeout(toastTimer);
  }
  toastTimer = window.setTimeout(() => {
    state.toastMessage = null;
    updateToastBanner();
  }, 3200);
}

async function openExternalUrl(url: string): Promise<void> {
  try {
    await openUrl(url);
  } catch {
    window.open(url, "_blank");
  }
}

async function ensureNotificationPermissionIfEnabled(): Promise<void> {
  if (!state.config.notificationsEnabled) return;
  try {
    const granted = await isPermissionGranted();
    if (!granted) {
      await requestPermission();
    }
  } catch {
    // Ignore outside Tauri runtime or on platforms without explicit permission prompt.
  }
}

function getEffectiveTier(): AccessTier {
  return state.snapshot.accessTier ?? state.config.accessTier ?? "user";
}

function getEffectiveBackendMode(): BackendMode {
  return state.snapshot.backendMode ?? state.config.backendMode ?? "live";
}

function getBrandTitle(): string {
  const edition = state.snapshot.gameEdition?.trim();
  return edition ? `${edition} GlobalMarket` : "GlobalMarket";
}

function getDisplayMarketName(isMock: boolean): string {
  if (isMock) {
    return t("marketNameMock");
  }
  const raw = state.snapshot.marketName?.trim() ?? "";
  if (!raw || raw === "Global Market" || raw === "Local Simulation (Mock)") {
    return t("officialMarketName");
  }
  return raw;
}

function normalizeUrlForCompare(raw: string): string {
  return raw.trim().replace(/\/+$/, "").toLowerCase();
}

function getSafeHeaderLogoUrl(): string {
  const isMock =
    getEffectiveTier() === "developer" && getEffectiveBackendMode() === "mock";
  const hasToken = Boolean(state.config.marketToken?.trim());
  const custom = state.snapshot.marketLogoUrl?.trim();
  if ((!hasToken && !isMock) || !custom) {
    return dsaLogoUrl;
  }
  const lower = custom.toLowerCase();
  if (
    isMock &&
    (lower.startsWith("data:image/png;base64,") ||
      lower.startsWith("data:image/webp;base64,") ||
      lower.startsWith("data:image/jpeg;base64,"))
  ) {
    return custom;
  }
  try {
    const parsed = new URL(custom);
    if (parsed.username || parsed.password) {
      return dsaLogoUrl;
    }
    const host = parsed.hostname.toLowerCase();
    if (
      parsed.protocol === "https:" &&
      (host === "img.gm.bydora.de" ||
        host === "gm.bydora.de" ||
        host === "dev.gm.bydora.de") &&
      (parsed.port === "" || parsed.port === "443")
    ) {
      return custom;
    }
    if (
      (parsed.protocol === "http:" || parsed.protocol === "https:") &&
      (host === "127.0.0.1" || host === "localhost")
    ) {
      return custom;
    }
    const activeBase = state.snapshot.backendUrl || state.config.backendUrl;
    if (parsed.protocol === "https:" && activeBase) {
      const baseParsed = new URL(activeBase);
      if (baseParsed.protocol === "https:" && parsed.host === baseParsed.host) {
        return custom;
      }
    }
  } catch {
    return dsaLogoUrl;
  }
  return dsaLogoUrl;
}

function syncDocumentTitle(): void {
  document.title = `${getBrandTitle()} – D-S-Agrarservice`;
}

function applySnapshot(snap: BridgeSnapshot): void {
  state.snapshot = snap;
  state.config.accessTier = snap.accessTier;
  state.config.backendMode = snap.backendMode;
  state.config.backendUrl = snap.backendUrl;
  if (snap.profilePath) {
    state.config.fs25ProfilePath = snap.profilePath;
  }
  syncDocumentTitle();
}

function getConnectionVisuals(conn: BridgeConnectionState): {
  label: string;
  dotClass: string;
  pillClass: string;
} {
  switch (conn) {
    case "online":
      return {
        label: t("stateOnline"),
        dotClass: "bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.8)]",
        pillClass: "border-emerald-500/40 bg-emerald-500/10 text-emerald-300",
      };
    case "connecting":
      return {
        label: t("stateConnecting"),
        dotClass: "bg-amber-400 animate-pulse",
        pillClass: "border-amber-500/40 bg-amber-500/10 text-amber-300",
      };
    case "outdated":
      return {
        label: t("stateOutdated"),
        dotClass: "bg-red-500 animate-ping",
        pillClass: "border-red-500/50 bg-red-500/15 text-red-300",
      };
    case "pathMissing":
      return {
        label: t("statePathMissing"),
        dotClass: "bg-amber-500",
        pillClass: "border-amber-500/40 bg-amber-500/10 text-amber-300",
      };
    case "backendOffline":
    default:
      return {
        label: t("stateBackendOffline"),
        dotClass: "bg-red-500",
        pillClass: "border-red-500/40 bg-red-500/10 text-red-300",
      };
  }
}

function resolveGreatDemandDisplayNames(): string[] {
  const codes = state.snapshot.activeGreatDemandCodes ?? [];
  const titles = state.snapshot.activeGreatDemands ?? [];
  if (codes.length === titles.length && codes.length > 0) {
    return codes.map((code, idx) => localizeFillTypeCode(code) ?? titles[idx] ?? code);
  }
  return titles;
}

function renderLogEntryHtml(entry: LogEntry): string {
  const timeStr = formatLocalTime(entry.timestampMs);
  const localizedMsg = formatLogEntryMessage(entry);
  let badgeClass = "bg-zinc-700/80 text-zinc-300";
  let badgeText = "INFO";
  let textClass = "text-zinc-300";

  if (entry.level === "success") {
    badgeClass = "bg-emerald-500/20 text-emerald-300 border border-emerald-500/30";
    badgeText = "OK";
    textClass = "text-zinc-100";
  } else if (entry.level === "warn") {
    badgeClass = "bg-amber-500/20 text-amber-300 border border-amber-500/30";
    badgeText = "WARN";
    textClass = "text-amber-200";
  } else if (entry.level === "error") {
    badgeClass = "bg-red-500/20 text-red-300 border border-red-500/30";
    badgeText = "ERR";
    textClass = "text-red-200";
  }

  return `
    <div class="flex items-start gap-2.5 py-1 border-b border-zinc-800/60 last:border-b-0 text-xs leading-relaxed">
      <span class="font-mono text-[11px] text-zinc-500 shrink-0 pt-0.5">${timeStr}</span>
      <span class="px-1.5 py-0.2 rounded text-[10px] font-semibold tracking-wide shrink-0 ${badgeClass}">${badgeText}</span>
      <span class="break-words min-w-0 ${textClass}">${escapeHtml(localizedMsg)}</span>
    </div>
  `;
}

function renderDashboardHtml(): string {
  const conn = getConnectionVisuals(state.snapshot.state);
  const tier = getEffectiveTier();
  const isExtendedOrDev = tier === "extended" || tier === "developer";
  const isDev = tier === "developer";
  const isMock = isDev && getEffectiveBackendMode() === "mock";
  const currentLang = getLocale();

  const langOptionsHtml = LANGUAGE_OPTIONS.map(
    (opt) =>
      `<option value="${opt.code}" ${opt.code === currentLang ? "selected" : ""} class="bg-[#252528] text-zinc-100">🌐 ${escapeHtml(opt.nativeName)} (${opt.shortLabel})</option>`,
  ).join("");

  const tierBadgeHtml =
    tier === "developer"
      ? `<button id="btn-open-access-badge" type="button" class="rounded px-1.5 py-0.5 text-[10px] font-bold tracking-wider uppercase bg-purple-500/20 text-purple-300 border border-purple-500/40 hover:bg-purple-500/35 hover:border-purple-400 transition cursor-pointer">${escapeHtml(t("tierBadgeDeveloper"))}</button>`
      : tier === "extended"
        ? `<button id="btn-open-access-badge" type="button" class="rounded px-1.5 py-0.5 text-[10px] font-bold tracking-wider uppercase bg-cyan-500/20 text-cyan-300 border border-cyan-500/40 hover:bg-cyan-500/35 hover:border-cyan-400 transition cursor-pointer">${escapeHtml(t("tierBadgeExtended"))}</button>`
        : "";

  const outdatedBannerHtml =
    state.snapshot.state === "outdated"
      ? `
      <div class="flex items-center justify-between gap-3 rounded-lg border border-red-500/50 bg-red-950/40 px-3.5 py-2.5 text-xs">
        <div class="min-w-0">
          <p class="font-semibold text-red-200">
            ${escapeHtml(
              t("outdatedTitle", { minVersion: state.snapshot.minExeVersion }),
            )}
          </p>
          <p class="text-red-300/90 mt-0.5">
            ${escapeHtml(
              t("outdatedDesc", {
                minVersion: state.snapshot.minExeVersion,
                currentVersion: state.snapshot.exeVersion,
              }),
            )}
          </p>
        </div>
        <button
          id="btn-open-releases"
          type="button"
          class="shrink-0 rounded-md bg-[#ff9600] px-3 py-1.5 text-xs font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
        >
          ${escapeHtml(t("outdatedAction"))}
        </button>
      </div>
    `
      : "";

  const gdLabels = resolveGreatDemandDisplayNames();
  const gdText =
    gdLabels.length > 0
      ? `<span class="font-semibold text-[#ff9600]">🔥 ${escapeHtml(gdLabels.join(", "))}</span>`
      : `<span class="text-zinc-500">${escapeHtml(t("greatDemandNone"))}</span>`;

  const pathHelpPopoverHtml = state.pathHelpOpen
    ? `
      <div id="path-help-popover" class="mt-2 rounded-lg border border-[#ff9600]/50 bg-[#18181b] p-3 text-xs text-zinc-200 shadow-xl space-y-2">
        <div class="flex items-center justify-between gap-2">
          <p class="font-semibold text-[#ff9600]">${escapeHtml(t("pathHelpTitle"))}</p>
          <button id="btn-close-path-help" type="button" class="text-zinc-400 hover:text-zinc-100 px-1 cursor-pointer">✕</button>
        </div>
        <p class="text-zinc-300 leading-relaxed">${escapeHtml(t("pathHelpIntro"))}</p>
        <div class="space-y-1 rounded border border-zinc-800 bg-[#202023] p-2 font-mono text-[11px] text-zinc-300">
          <div><span class="text-zinc-400 font-sans font-semibold">${escapeHtml(t("pathHelpWinLabel"))}</span> …\\Documents\\My Games\\FarmingSimulator20XX</div>
          <div><span class="text-zinc-400 font-sans font-semibold">${escapeHtml(t("pathHelpMacLabel"))}</span> ~/Library/Application Support/FarmingSimulator20XX</div>
          <div><span class="text-zinc-400 font-sans font-semibold">${escapeHtml(t("pathHelpLinuxLabel"))}</span> …/compatdata/&lt;AppID&gt;/pfx/…/FarmingSimulator20XX</div>
        </div>
        <p class="text-[11px] text-zinc-400 italic">${escapeHtml(t("pathHelpTip"))}</p>
      </div>
    `
    : "";

  const activeServerUrl = state.snapshot.backendUrl || state.config.backendUrl;
  const serverInfoInlineHtml = isExtendedOrDev
    ? `
      <span class="text-zinc-600">&bull;</span>
      <span class="text-zinc-400">${escapeHtml(t("serverLabel"))}</span>
      <span class="font-mono text-[11px] text-sky-300 truncate max-w-[240px]" title="${escapeHtml(activeServerUrl)}">
        ${escapeHtml(isMock ? "in-memory://mock" : activeServerUrl)}
      </span>
      ${
        state.snapshot.apiVersion
          ? `<span class="rounded bg-zinc-800 border border-zinc-700 px-1.5 py-0.2 font-mono text-[10px] text-zinc-400">API v${escapeHtml(state.snapshot.apiVersion)}</span>`
          : ""
      }
    `
    : "";

  const lastSyncFormatted = state.snapshot.lastSyncUnix
    ? formatLocalTime(state.snapshot.lastSyncUnix * 1000)
    : "—";

  const extendedDebugStripHtml = isExtendedOrDev
    ? `
      <div class="flex flex-wrap items-center justify-between gap-2 pt-1.5 border-t border-zinc-800/80 text-[11px] text-zinc-400 font-mono">
        <div class="truncate max-w-[58%]" title="${escapeHtml(state.snapshot.bridgePath ?? "—")}">
          <span class="font-sans text-zinc-500">${escapeHtml(t("debugBridgeLabel"))}</span>
          <span class="text-zinc-300">${escapeHtml(state.snapshot.bridgePath ?? "—")}</span>
        </div>
        <div class="flex items-center gap-3 shrink-0">
          <span>
            <span class="font-sans text-zinc-500">${escapeHtml(t("debugQueuesLabel"))}</span>
            R:${state.snapshot.readQueueCount} / W:${state.snapshot.writeQueueCount}
          </span>
          <span>
            <span class="font-sans text-zinc-500">${escapeHtml(t("debugLastSyncLabel"))}</span>
            ${lastSyncFormatted}
          </span>
        </div>
      </div>
    `
    : "";

  const activeUrlNorm = normalizeUrlForCompare(activeServerUrl);
  const isLiveProd =
    !isMock && activeUrlNorm === normalizeUrlForCompare(OFFICIAL_PROD_BACKEND_URL);
  const isLiveDev =
    !isMock && activeUrlNorm === normalizeUrlForCompare(OFFICIAL_DEV_BACKEND_URL);

  const mockSectionHtml = isDev
    ? `
      <section class="rounded-lg border ${
        isMock ? "border-[#ff9600]/40" : "border-purple-500/35"
      } bg-[#252528] px-3.5 py-2 shrink-0">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <button
            id="btn-toggle-mock-tools"
            type="button"
            class="flex items-center gap-2 text-xs font-semibold ${
              isMock ? "text-[#ff9600] hover:text-[#ffa726]" : "text-purple-300 hover:text-purple-200"
            } cursor-pointer"
          >
            <span>${state.mockToolsExpanded ? "▾" : "▸"}</span>
            <span>${escapeHtml(t("devToolbarTitle"))}</span>
          </button>

          <div class="flex items-center gap-2">
            <div class="inline-flex rounded-md border border-zinc-700 bg-[#18181b] p-0.5 text-[11px] font-semibold">
              <button
                id="btn-dev-switch-prod"
                type="button"
                title="${escapeHtml(OFFICIAL_PROD_BACKEND_URL)}"
                class="rounded px-2.5 py-0.5 transition cursor-pointer ${
                  isLiveProd
                    ? "bg-sky-500/25 text-sky-200 border border-sky-500/40"
                    : "text-zinc-400 hover:text-zinc-200"
                }"
              >
                🌐 ${escapeHtml(t("devSwitchProd"))}
              </button>
              <button
                id="btn-dev-switch-dev"
                type="button"
                title="${escapeHtml(OFFICIAL_DEV_BACKEND_URL)}"
                class="rounded px-2.5 py-0.5 transition cursor-pointer ${
                  isLiveDev
                    ? "bg-purple-500/25 text-purple-200 border border-purple-500/40"
                    : "text-zinc-400 hover:text-zinc-200"
                }"
              >
                🛠 ${escapeHtml(t("devSwitchDev"))}
              </button>
              <button
                id="btn-dev-switch-mock"
                type="button"
                class="rounded px-2.5 py-0.5 transition cursor-pointer ${
                  isMock
                    ? "bg-[#ff9600] text-zinc-950 font-bold"
                    : "text-zinc-400 hover:text-zinc-200"
                }"
              >
                🧪 ${escapeHtml(t("devSwitchMock"))}
              </button>
            </div>
            <span class="text-[10px] font-mono uppercase tracking-wider text-zinc-400">v${escapeHtml(state.snapshot.exeVersion)}</span>
          </div>
        </div>
        ${
          state.mockToolsExpanded
            ? isMock
              ? `
          <div class="mt-2 flex flex-wrap items-center gap-2">
            <button
              id="btn-mock-gd"
              type="button"
              class="rounded bg-[#ff9600]/15 border border-[#ff9600]/40 px-2.5 py-1 text-xs font-medium text-[#ff9600] hover:bg-[#ff9600] hover:text-zinc-950 transition cursor-pointer"
            >
              🔥 ${escapeHtml(t("mockBtnGreatDemand"))}
            </button>
            <button
              id="btn-mock-approve"
              type="button"
              class="rounded bg-emerald-500/15 border border-emerald-500/40 px-2.5 py-1 text-xs font-medium text-emerald-300 hover:bg-emerald-500 hover:text-zinc-950 transition cursor-pointer"
            >
              ✓ ${escapeHtml(t("mockBtnApprove"))}
            </button>
            <button
              id="btn-mock-offline"
              type="button"
              class="rounded border px-2.5 py-1 text-xs font-medium transition cursor-pointer ${
                state.mockSimulatedOffline
                  ? "bg-amber-500 text-zinc-950 border-amber-400"
                  : "bg-zinc-800 border-zinc-700 text-zinc-300 hover:border-zinc-500"
              }"
            >
              ⚡ ${escapeHtml(state.mockSimulatedOffline ? t("mockBtnOfflineOff") : t("mockBtnOfflineOn"))}
            </button>
            <button
              id="btn-mock-minver"
              type="button"
              class="rounded border px-2.5 py-1 text-xs font-medium transition cursor-pointer ${
                state.mockLockedMinVersion
                  ? "bg-red-500 text-white border-red-400"
                  : "bg-zinc-800 border-zinc-700 text-zinc-300 hover:border-zinc-500"
              }"
            >
              ⬆ ${escapeHtml(state.mockLockedMinVersion ? t("mockBtnMinVerOff") : t("mockBtnMinVerOn"))}
            </button>
            <button
              id="btn-mock-logo"
              type="button"
              class="rounded border border-sky-500/40 bg-sky-500/15 px-2.5 py-1 text-xs font-medium text-sky-300 hover:bg-sky-500 hover:text-zinc-950 transition cursor-pointer"
            >
              🖼 ${escapeHtml(t("mockBtnLogoSet"))}
            </button>
            ${
              state.snapshot.marketLogoUrl
                ? `
            <button
              id="btn-mock-logo-reset"
              type="button"
              class="rounded border border-zinc-600 bg-zinc-800 px-2.5 py-1 text-xs font-medium text-zinc-300 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
            >
              ↺ ${escapeHtml(t("mockBtnLogoReset"))}
            </button>
            `
                : ""
            }
          </div>
        `
              : `
          <p class="mt-1.5 text-[11px] text-zinc-400">
            ${escapeHtml(t("devMockInactiveHint"))}
          </p>
        `
            : ""
        }
      </section>
    `
    : "";

  const logsBodyHtml =
    state.logs.length === 0
      ? `<p class="text-xs text-zinc-500 italic py-4 text-center">${escapeHtml(t("logEmpty"))}</p>`
      : state.logs.map(renderLogEntryHtml).join("");

  const profileValue = state.snapshot.profilePath ?? state.config.fs25ProfilePath;
  const brandTitle = getBrandTitle();
  const displayMarketName = getDisplayMarketName(isMock);
  const headerLogoUrl = getSafeHeaderLogoUrl();
  const isSquareLogo = squareLogoCache.get(headerLogoUrl) ?? false;
  const logoSizeClass = isSquareLogo ? "h-[104px]" : "h-[72px]";
  const editionBadgeHtml = state.snapshot.gameEdition
    ? `<span class="rounded bg-[#ff9600]/15 border border-[#ff9600]/35 px-1.5 py-0.2 font-mono text-[10px] font-bold text-[#ff9600]">${escapeHtml(state.snapshot.gameEdition)}</span>`
    : "";

  return `
    <!-- 3-Column Hero Header: Left Brand/Version | Center Logo + Hidden 5x Trigger + Status | Right Settings + Language -->
    <header class="grid grid-cols-[1fr_auto_1fr] items-center gap-4 rounded-lg border border-zinc-800 bg-gradient-to-b from-[#252528] to-[#1e1e21] px-5 py-3.5 shrink-0">
      <!-- Left Column: Dynamic Edition Title, Subtitle, Creator & Version -->
      <div class="flex flex-col items-start gap-1 min-w-0">
        <div class="flex items-center gap-2 flex-wrap">
          <h1 class="text-base font-bold tracking-tight text-[#ff9600] truncate">${escapeHtml(brandTitle)}</h1>
          <span class="rounded px-1.5 py-0.5 text-[10px] font-bold tracking-wider uppercase ${
            isMock
              ? "bg-amber-500/15 text-amber-300 border border-amber-500/30"
              : "bg-sky-500/15 text-sky-300 border border-sky-500/30"
          }">
            ${escapeHtml(isMock ? t("modeMockBadge") : t("modeLiveBadge"))}
          </span>
          ${tierBadgeHtml}
        </div>
        <p class="text-xs text-zinc-300 truncate">${escapeHtml(t("appSubtitle"))}</p>
        <p class="text-[11px] text-zinc-500 truncate">
          D-S-Agrarservice &bull; <span data-secret-trigger="true" class="font-mono select-none">v${escapeHtml(state.info.protocolVersion)}</span>
        </p>
      </div>

      <!-- Center Column: Prominent Logo (Hidden 5x Trigger) -> Market Name -> Connection Status Pill -->
      <div class="flex flex-col items-center text-center gap-2 px-3">
        <img
          id="img-header-logo"
          data-secret-trigger="true"
          draggable="false"
          src="${escapeHtml(headerLogoUrl)}"
          alt="${escapeHtml(displayMarketName)}"
          class="${logoSizeClass} w-auto max-w-[260px] object-contain select-none ${
            headerLogoUrl === dsaLogoUrl
              ? "drop-shadow-[0_2px_10px_rgba(255,150,0,0.14)]"
              : "drop-shadow-[0_3px_12px_rgba(0,0,0,0.55)]"
          }"
        />
        <button
          id="btn-secret-version"
          data-secret-trigger="true"
          type="button"
          class="mt-0.5 text-xs font-semibold text-zinc-200 focus:outline-none cursor-default select-none max-w-[260px] truncate"
        >${escapeHtml(displayMarketName)}</button>
        <div class="inline-flex items-center gap-2 rounded-full border px-3.5 py-1 text-xs font-semibold ${conn.pillClass}">
          <span class="inline-block h-2.5 w-2.5 rounded-full ${conn.dotClass}"></span>
          <span>${escapeHtml(conn.label)}</span>
        </div>
      </div>

      <!-- Right Column: Settings Button on top + Quick Language Selector stacked underneath -->
      <div class="flex flex-col items-end gap-2">
        <button
          id="btn-open-settings"
          type="button"
          title="${escapeHtml(t("btnSettings"))}"
          class="w-40 flex items-center justify-center gap-1.5 rounded-md border border-zinc-700 bg-[#252528] px-3 py-1.5 text-xs font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
        >
          <svg class="h-3.5 w-3.5 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="3"></circle>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
          </svg>
          <span class="truncate">${escapeHtml(t("btnSettings"))}</span>
        </button>

        <select
          id="select-quick-lang"
          aria-label="Language"
          class="w-40 rounded-md border border-zinc-700 bg-[#252528] px-2.5 py-1.5 text-xs font-semibold text-zinc-200 hover:border-[#ff9600]/60 focus:border-[#ff9600] focus:outline-none cursor-pointer"
        >
          ${langOptionsHtml}
        </select>
      </div>
    </header>

    ${outdatedBannerHtml}

    <!-- Farming Simulator Path & Market Counters Card -->
    <section class="rounded-lg border border-zinc-800 bg-[#252528] p-3 space-y-2 shrink-0">
      <div class="min-w-0">
        <div class="flex items-center gap-1.5 mb-1">
          <label class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
            ${escapeHtml(t("pathSectionTitle"))}
          </label>
          ${editionBadgeHtml}
          <button
            id="btn-toggle-path-help"
            type="button"
            title="${escapeHtml(t("pathHelpBtnTitle"))}"
            aria-expanded="${state.pathHelpOpen ? "true" : "false"}"
            class="inline-flex h-4 w-4 items-center justify-center rounded-full border text-[10px] font-bold transition cursor-pointer ${
              state.pathHelpOpen
                ? "border-[#ff9600] bg-[#ff9600] text-zinc-950"
                : "border-zinc-600 bg-zinc-800 text-zinc-300 hover:border-[#ff9600] hover:text-[#ff9600]"
            }"
          >
            ?
          </button>
        </div>

        <div class="flex items-center gap-2">
          <div
            class="flex-1 truncate rounded border px-2.5 py-1.5 font-mono text-xs ${
              profileValue
                ? "border-zinc-700 bg-[#18181b] text-zinc-200"
                : "border-amber-500/50 bg-amber-950/20 text-amber-300 italic"
            }"
            title="${escapeHtml(t("pathHelpIntro"))}${profileValue ? `\n\n${profileValue}` : ""}"
          >
            ${profileValue ? escapeHtml(profileValue) : escapeHtml(t("pathPlaceholder"))}
          </div>
          <button
            id="btn-browse-path"
            type="button"
            title="${escapeHtml(t("pathHelpIntro"))}"
            class="shrink-0 rounded-md bg-[#ff9600] px-3 py-1.5 text-xs font-semibold text-zinc-950 hover:bg-[#ffa726] active:scale-[0.99] transition cursor-pointer"
          >
            ${escapeHtml(t("pathBrowseBtn"))}
          </button>
        </div>

        ${pathHelpPopoverHtml}
      </div>

      <!-- Great Demand & Whitelist Moderation Strip -->
      <div class="flex flex-wrap items-center justify-between gap-2 pt-1.5 border-t border-zinc-800/80 text-xs">
        <div class="flex items-center gap-1.5 min-w-0 flex-wrap">
          <span class="text-zinc-400">${escapeHtml(t("greatDemandActive"))}</span>
          ${gdText}
          ${serverInfoInlineHtml}
        </div>

        <div class="flex flex-wrap items-center gap-1.5">
          <span class="rounded bg-emerald-500/15 border border-emerald-500/30 px-2 py-0.5 text-[11px] font-medium text-emerald-300">
            ✓ ${escapeHtml(t("whitelistApproved", { count: state.snapshot.approvedCount }))}
          </span>
          <span class="rounded bg-amber-500/15 border border-amber-500/30 px-2 py-0.5 text-[11px] font-medium text-amber-300">
            ⏳ ${escapeHtml(t("whitelistPending", { count: state.snapshot.pendingCount }))}
          </span>
          <span class="rounded bg-zinc-700/50 border border-zinc-600/50 px-2 py-0.5 text-[11px] font-medium text-zinc-300">
            ${escapeHtml(t("whitelistRejected", { count: state.snapshot.rejectedCount }))}
          </span>
        </div>
      </div>

      ${
        state.snapshot.lastError
          ? `<div class="text-red-400 text-[11px] truncate" title="${escapeHtml(state.snapshot.lastError)}">⚠ ${escapeHtml(state.snapshot.lastError)}</div>`
          : ""
      }

      ${extendedDebugStripHtml}
    </section>

    ${mockSectionHtml}

    <!-- Live Activity Log -->
    <section class="flex flex-col flex-1 min-h-0 rounded-lg border border-zinc-800 bg-[#252528] overflow-hidden">
      <div class="flex items-center justify-between px-3.5 py-2 border-b border-zinc-800 bg-[#202023] shrink-0">
        <span class="text-xs font-semibold uppercase tracking-wider text-zinc-300">
          ${escapeHtml(t("logTitle"))}
        </span>
        <label class="inline-flex items-center gap-1.5 text-[11px] font-medium text-zinc-300 hover:text-zinc-100 cursor-pointer select-none">
          <input
            id="chk-log-autoscroll"
            type="checkbox"
            ${state.config.logAutoScroll ? "checked" : ""}
            class="h-3.5 w-3.5 accent-[#ff9600] rounded cursor-pointer"
          />
          <span>${escapeHtml(t("logAutoScroll"))}</span>
        </label>
      </div>
      <div id="log-scroll-box" class="dsa-scrollbar flex-1 overflow-y-auto px-3.5 py-2 bg-[#18181b]/70">
        <div id="log-list">
          ${logsBodyHtml}
        </div>
      </div>
    </section>

    <!-- Bottom Support, Links, Credits & Quit Footer -->
    <footer class="flex items-center justify-between gap-2 border-t border-zinc-800 pt-2.5 text-xs shrink-0">
      <div class="flex flex-wrap items-center gap-1.5">
        <button
          id="btn-copy-diag"
          type="button"
          class="flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 font-medium transition cursor-pointer ${
            state.copiedDiagFeedback
              ? "border-emerald-500 bg-emerald-500/20 text-emerald-300"
              : "border-zinc-700 bg-[#252528] text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600]"
          }"
        >
          <span>${state.copiedDiagFeedback ? "✓" : "📋"}</span>
          <span>${escapeHtml(state.copiedDiagFeedback ? t("btnCopiedDiag") : t("btnCopyDiag"))}</span>
        </button>

        <button
          id="btn-open-discord"
          type="button"
          class="flex items-center gap-1.5 rounded-md border border-indigo-500/40 bg-indigo-500/15 px-2.5 py-1.5 font-medium text-indigo-300 hover:bg-indigo-500/25 hover:border-indigo-400 transition cursor-pointer"
        >
          <span>💬</span>
          <span>${escapeHtml(t("btnDiscord"))}</span>
        </button>

        <button
          id="btn-open-github"
          type="button"
          class="flex items-center gap-1.5 rounded-md border border-zinc-700 bg-[#252528] px-2.5 py-1.5 font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
        >
          <svg class="h-3.5 w-3.5 fill-current" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 .5C5.65.5.5 5.65.5 12a11.5 11.5 0 0 0 7.86 10.92c.58.1.79-.25.79-.56v-2.02c-3.2.7-3.88-1.37-3.88-1.37-.52-1.33-1.28-1.68-1.28-1.68-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.77 2.7 1.26 3.36.96.1-.75.4-1.26.73-1.55-2.55-.29-5.24-1.28-5.24-5.68 0-1.26.45-2.28 1.18-3.09-.12-.29-.51-1.46.11-3.05 0 0 .97-.31 3.17 1.18a11 11 0 0 1 5.78 0c2.2-1.49 3.16-1.18 3.16-1.18.63 1.59.24 2.76.12 3.05.74.81 1.18 1.83 1.18 3.09 0 4.41-2.7 5.38-5.27 5.67.42.36.78 1.06.78 2.14v3.17c0 .31.21.67.8.56A11.5 11.5 0 0 0 23.5 12C23.5 5.65 18.35.5 12 .5Z"/>
          </svg>
          <span>${escapeHtml(t("btnGithub"))}</span>
        </button>

        <button
          id="btn-open-credits"
          type="button"
          class="flex items-center gap-1.5 rounded-md border border-zinc-700 bg-[#252528] px-2.5 py-1.5 font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
        >
          <span>ⓘ</span>
          <span>${escapeHtml(t("btnCredits"))}</span>
        </button>
      </div>

      <div class="flex items-center gap-3 shrink-0">
        ${
          state.config.minimizeToTray
            ? `<span class="hidden md:inline text-[11px] text-zinc-500">${escapeHtml(t("trayHint"))}</span>`
            : ""
        }
        <button
          id="btn-quit-app"
          type="button"
          class="rounded-md bg-[#ff9600] px-3.5 py-1.5 font-semibold text-zinc-950 hover:bg-[#ffa726] active:scale-[0.99] transition cursor-pointer"
        >
          ${escapeHtml(t("btnQuit"))}
        </button>
      </div>
    </footer>
  `;
}

function renderSettingsModalHtml(): string {
  const currentLang = normalizeLangForForm(state.config.language);
  const isDev = getEffectiveTier() === "developer";
  const activeMode = getEffectiveBackendMode();

  const langSelectHtml = LANGUAGE_OPTIONS.map(
    (opt) =>
      `<option value="${opt.code}" ${opt.code === currentLang ? "selected" : ""} class="bg-[#252528] text-zinc-100">${opt.nativeName} (${opt.shortLabel})</option>`,
  ).join("");

  const devServerFieldsHtml = isDev
    ? `
      <div>
        <label for="cfg-mode" class="block font-semibold text-purple-300 mb-1">
          ${escapeHtml(t("settingsModeLabel"))}
        </label>
        <select
          id="cfg-mode"
          class="w-full rounded-md border border-purple-500/40 bg-[#18181b] px-3 py-2 text-zinc-100 focus:border-[#ff9600] focus:outline-none"
        >
          <option value="mock" ${activeMode === "mock" ? "selected" : ""} class="bg-[#252528]">
            ${escapeHtml(t("settingsModeMock"))}
          </option>
          <option value="live" ${activeMode === "live" ? "selected" : ""} class="bg-[#252528]">
            ${escapeHtml(t("settingsModeLive"))}
          </option>
        </select>
      </div>

      <div>
        <label for="cfg-url" class="block font-semibold text-purple-300 mb-1">
          ${escapeHtml(t("settingsServerUrl"))}
        </label>
        <input
          id="cfg-url"
          type="text"
          value="${escapeHtml(state.config.backendUrl)}"
          placeholder="${escapeHtml(OFFICIAL_PROD_BACKEND_URL)}"
          class="w-full rounded-md border border-purple-500/40 bg-[#18181b] px-3 py-1.5 font-mono text-zinc-100 focus:border-[#ff9600] focus:outline-none"
        />
        <div class="mt-1.5 flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            data-preset-url="${escapeHtml(OFFICIAL_PROD_BACKEND_URL)}"
            class="rounded border border-sky-500/35 bg-sky-500/10 px-2 py-0.5 text-[11px] font-medium text-sky-300 hover:bg-sky-500/25 transition cursor-pointer"
          >
            🌐 ${escapeHtml(t("settingsPresetProd"))}
          </button>
          <button
            type="button"
            data-preset-url="${escapeHtml(OFFICIAL_DEV_BACKEND_URL)}"
            class="rounded border border-purple-500/35 bg-purple-500/10 px-2 py-0.5 text-[11px] font-medium text-purple-300 hover:bg-purple-500/25 transition cursor-pointer"
          >
            🛠 ${escapeHtml(t("settingsPresetDev"))}
          </button>
          <button
            type="button"
            data-preset-url="${escapeHtml(LOCAL_DEV_BACKEND_URL)}"
            class="rounded border border-zinc-700 bg-zinc-800 px-2 py-0.5 text-[11px] font-medium text-zinc-300 hover:border-zinc-500 transition cursor-pointer"
          >
            💻 ${escapeHtml(t("settingsPresetLocal"))}
          </button>
        </div>
      </div>
    `
    : "";

  return `
    <div id="modal-backdrop" class="fixed inset-0 z-40 flex items-center justify-center bg-black/70 backdrop-blur-[2px] p-4">
      <div class="w-full max-w-lg rounded-xl border border-zinc-700 bg-[#252528] shadow-2xl overflow-hidden">
        <div class="flex items-center justify-between border-b border-zinc-800 px-5 py-3.5 bg-[#202023]">
          <h2 class="text-sm font-bold text-[#ff9600] tracking-wide uppercase">
            ${escapeHtml(t("settingsTitle"))}
          </h2>
          <button
            id="btn-close-modal"
            type="button"
            class="text-zinc-400 hover:text-zinc-100 text-sm px-1.5 py-0.5 rounded cursor-pointer"
          >
            ✕
          </button>
        </div>

        <form id="form-settings" class="p-5 space-y-4 text-xs max-h-[78vh] overflow-y-auto dsa-scrollbar">
          <!-- Language -->
          <div>
            <label for="cfg-lang" class="block font-semibold text-zinc-200 mb-1">
              ${escapeHtml(t("settingsLanguage"))}
            </label>
            <select
              id="cfg-lang"
              class="w-full rounded-md border border-zinc-700 bg-[#18181b] px-3 py-2 text-zinc-100 focus:border-[#ff9600] focus:outline-none"
            >
              ${langSelectHtml}
            </select>
          </div>

          <!-- Window & Notifications -->
          <div class="pt-2 border-t border-zinc-800 space-y-3">
            <p class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
              ${escapeHtml(t("settingsBehaviorSection"))}
            </p>

            <label class="flex items-start gap-3 cursor-pointer">
              <input
                id="cfg-tray"
                type="checkbox"
                ${state.config.minimizeToTray ? "checked" : ""}
                class="mt-0.5 h-4 w-4 accent-[#ff9600] rounded cursor-pointer"
              />
              <div>
                <span class="block font-medium text-zinc-100">${escapeHtml(t("settingsMinimizeTray"))}</span>
                <span class="block text-zinc-400 text-[11px]">${escapeHtml(t("settingsMinimizeTrayDesc"))}</span>
              </div>
            </label>

            <label class="flex items-start gap-3 cursor-pointer">
              <input
                id="cfg-notify"
                type="checkbox"
                ${state.config.notificationsEnabled ? "checked" : ""}
                class="mt-0.5 h-4 w-4 accent-[#ff9600] rounded cursor-pointer"
              />
              <div>
                <span class="block font-medium text-zinc-100">${escapeHtml(t("settingsNotifications"))}</span>
                <span class="block text-zinc-400 text-[11px]">${escapeHtml(t("settingsNotificationsDesc"))}</span>
              </div>
            </label>

            <label class="flex items-start gap-3 cursor-pointer">
              <input
                id="cfg-autoscroll"
                type="checkbox"
                ${state.config.logAutoScroll ? "checked" : ""}
                class="mt-0.5 h-4 w-4 accent-[#ff9600] rounded cursor-pointer"
              />
              <div>
                <span class="block font-medium text-zinc-100">${escapeHtml(t("settingsAutoScroll"))}</span>
                <span class="block text-zinc-400 text-[11px]">${escapeHtml(t("settingsAutoScrollDesc"))}</span>
              </div>
            </label>
          </div>

          <!-- Market Connection -->
          <div class="pt-2 border-t border-zinc-800 space-y-3">
            <p class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
              ${escapeHtml(t("settingsConnectionSection"))}
            </p>

            ${devServerFieldsHtml}

            <div>
              <label for="cfg-token" class="block font-semibold text-zinc-200 mb-1">
                ${escapeHtml(t("settingsMarketToken"))}
              </label>
              <input
                id="cfg-token"
                type="password"
                value="${escapeHtml(state.config.marketToken ?? "")}"
                placeholder="${escapeHtml(t("settingsMarketTokenPlaceholder"))}"
                class="w-full rounded-md border border-zinc-700 bg-[#18181b] px-3 py-1.5 font-mono text-zinc-100 focus:border-[#ff9600] focus:outline-none"
              />
              <p class="mt-1 text-[11px] text-zinc-500">${escapeHtml(t("settingsMarketTokenHint"))}</p>
            </div>
          </div>

          <div class="flex items-center justify-end gap-2 pt-3 border-t border-zinc-800">
            <button
              id="btn-cancel-modal"
              type="button"
              class="rounded-md border border-zinc-700 bg-zinc-800 px-3.5 py-1.5 font-medium text-zinc-200 hover:bg-zinc-700 transition cursor-pointer"
            >
              ${escapeHtml(t("settingsCancel"))}
            </button>
            <button
              type="submit"
              class="rounded-md bg-[#ff9600] px-4 py-1.5 font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
            >
              ${escapeHtml(t("settingsSave"))}
            </button>
          </div>
        </form>
      </div>
    </div>
  `;
}

function renderCreditsModalHtml(): string {
  const coreTeamHtml = CREDITS_CORE_TEAM.map((member) => {
    const roleSuffix = member.roleKey
      ? ` <span class="text-zinc-400 font-normal">&bull; ${escapeHtml(t(member.roleKey))}</span>`
      : "";
    return `<span class="rounded-md border border-[#ff9600]/35 bg-[#18181b] px-3 py-1.5 text-xs font-semibold text-amber-100">${escapeHtml(member.name)}${roleSuffix}</span>`;
  }).join("");

  const advisoryHtml = CREDITS_ADVISORY.map((member) => {
    const roleSuffix = member.roleKey
      ? ` <span class="text-zinc-400 font-normal">&bull; ${escapeHtml(t(member.roleKey))}</span>`
      : "";
    return `<span class="rounded-md border border-zinc-600/80 bg-[#18181b] px-2.5 py-1.5 text-xs font-semibold text-zinc-100">${escapeHtml(member.name)}${roleSuffix}</span>`;
  }).join("");

  const feedbackHtml = CREDITS_FEEDBACK_TESTERS.map(
    (name) =>
      `<span class="rounded-md border border-zinc-800 bg-[#18181b]/80 px-2.5 py-1 text-[11px] font-medium text-zinc-300">${escapeHtml(name)}</span>`,
  ).join("");

  return `
    <div id="modal-backdrop" class="fixed inset-0 z-40 flex items-center justify-center bg-black/75 backdrop-blur-[2px] p-4">
      <div class="w-full max-w-xl rounded-xl border border-zinc-700 bg-[#252528] shadow-2xl overflow-hidden">
        <div class="flex items-center justify-between border-b border-zinc-800 px-5 py-3.5 bg-[#202023]">
          <div class="flex items-center gap-2.5">
            <img src="${dsaLogoUrl}" alt="D-S-Agrarservice" class="h-6 w-auto object-contain" />
            <h2 class="text-sm font-bold text-[#ff9600] tracking-wide uppercase">
              ${escapeHtml(t("creditsModalTitle"))}
            </h2>
          </div>
          <button
            id="btn-close-modal"
            type="button"
            class="text-zinc-400 hover:text-zinc-100 text-sm px-1.5 py-0.5 rounded cursor-pointer"
          >
            ✕
          </button>
        </div>

        <div class="p-5 space-y-3.5 text-xs max-h-[80vh] overflow-y-auto dsa-scrollbar">
          <!-- Project Lead & Development -->
          <div class="space-y-2">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
              ${escapeHtml(t("creditsLeadSection"))}
            </h3>
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
              <div class="rounded-lg border border-zinc-700/80 bg-[#18181b] p-3">
                <p class="font-bold text-sm text-[#ff9600]">Floowy <span class="text-xs font-normal text-zinc-400">(D-S-Agrarservice)</span></p>
                <p class="text-zinc-300 mt-0.5 leading-snug">${escapeHtml(t("creditsRoleFloowy"))}</p>
              </div>
              <div class="rounded-lg border border-zinc-700/80 bg-[#18181b] p-3">
                <p class="font-bold text-sm text-[#ff9600]">FGBxRamel</p>
                <p class="text-zinc-300 mt-0.5 leading-snug">${escapeHtml(t("creditsRoleSon"))}</p>
              </div>
            </div>
          </div>

          <!-- Special Mention: kevink98 -->
          <div class="space-y-1.5">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-[#ff9600]">
              ★ ${escapeHtml(t("creditsSpecialSection"))}
            </h3>
            <div class="rounded-lg border border-[#ff9600]/40 bg-[#ff9600]/10 p-3.5">
              <p class="font-bold text-sm text-amber-200">kevink98</p>
              <p class="text-zinc-200 mt-1 leading-relaxed">
                ${escapeHtml(t("creditsSpecialKevink"))}
              </p>
            </div>
          </div>

          <!-- Tier 1: Core Team & Project Support -->
          <div class="space-y-1.5">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-300">
              ${escapeHtml(t("creditsCoreTeamSection"))}
            </h3>
            <div class="flex flex-wrap gap-1.5">
              ${coreTeamHtml}
            </div>
          </div>

          <!-- Tier 2: Ideas & External Consulting -->
          <div class="space-y-1.5">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400">
              ${escapeHtml(t("creditsAdvisorySection"))}
            </h3>
            <div class="flex flex-wrap gap-1.5">
              ${advisoryHtml}
            </div>
          </div>

          <!-- Tier 3: Testing, Suggestions & Feedback -->
          <div class="space-y-1.5">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-500">
              ${escapeHtml(t("creditsFeedbackSection"))}
            </h3>
            <div class="flex flex-wrap gap-1.5">
              ${feedbackHtml}
            </div>
          </div>

          <div class="flex flex-wrap items-center justify-between gap-2 pt-3 border-t border-zinc-800">
            <span class="font-mono text-[11px] text-zinc-500">
              ${escapeHtml(getBrandTitle())} v${escapeHtml(state.info.appVersion)} (${escapeHtml(state.info.platform)})
            </span>
            <div class="flex items-center gap-2 ml-auto">
              <button
                id="btn-open-licenses"
                type="button"
                class="flex items-center gap-1.5 rounded-md border border-zinc-700 bg-[#18181b] px-3 py-1.5 font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
              >
                <span>📜</span>
                <span>${escapeHtml(t("creditsOpenSourceBtn"))}</span>
              </button>
              <button
                id="btn-cancel-modal"
                type="button"
                class="rounded-md bg-[#ff9600] px-4 py-1.5 font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
              >
                ${escapeHtml(t("creditsCloseBtn"))}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  `;
}

function renderLicensesModalHtml(): string {
  const packagesHtml = OPEN_SOURCE_PACKAGES.map(
    (pkg) => `
      <div class="flex items-start justify-between gap-3 rounded-lg border border-zinc-800 bg-[#18181b] p-3">
        <div class="min-w-0">
          <div class="flex items-center gap-2 flex-wrap">
            <button
              type="button"
              data-ext-url="${escapeHtml(pkg.url)}"
              class="font-semibold text-zinc-100 hover:text-[#ff9600] transition cursor-pointer text-left"
            >
              ${escapeHtml(pkg.name)} ↗
            </button>
          </div>
          <p class="text-[11px] text-zinc-400 mt-0.5">${escapeHtml(pkg.author)}</p>
        </div>
        <span class="shrink-0 rounded border border-[#ff9600]/30 bg-[#ff9600]/10 px-2 py-0.5 font-mono text-[10px] font-semibold text-[#ff9600]">
          ${escapeHtml(pkg.license)}
        </span>
      </div>
    `,
  ).join("");

  const noticesUrl = `${GITHUB_REPO_URL}/blob/main/THIRD-PARTY-NOTICES.md`;

  return `
    <div id="modal-backdrop" class="fixed inset-0 z-40 flex items-center justify-center bg-black/75 backdrop-blur-[2px] p-4">
      <div class="w-full max-w-xl rounded-xl border border-zinc-700 bg-[#252528] shadow-2xl overflow-hidden">
        <div class="flex items-center justify-between border-b border-zinc-800 px-5 py-3.5 bg-[#202023]">
          <h2 class="text-sm font-bold text-[#ff9600] tracking-wide uppercase">
            📜 ${escapeHtml(t("licensesModalTitle"))}
          </h2>
          <button
            id="btn-close-modal"
            type="button"
            class="text-zinc-400 hover:text-zinc-100 text-sm px-1.5 py-0.5 rounded cursor-pointer"
          >
            ✕
          </button>
        </div>

        <div class="p-5 space-y-3.5 text-xs">
          <p class="text-zinc-300 leading-relaxed">
            ${escapeHtml(t("licensesModalIntro"))}
          </p>

          <div class="space-y-2 max-h-[50vh] overflow-y-auto pr-1 dsa-scrollbar">
            ${packagesHtml}
          </div>

          <div class="flex flex-wrap items-center justify-between gap-2 pt-3 border-t border-zinc-800">
            <button
              id="btn-back-credits"
              type="button"
              class="rounded-md border border-zinc-700 bg-zinc-800 px-3 py-1.5 font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
            >
              ← ${escapeHtml(t("licensesBackBtn"))}
            </button>

            <div class="flex items-center gap-2 ml-auto">
              <button
                type="button"
                data-ext-url="${escapeHtml(noticesUrl)}"
                class="rounded-md border border-zinc-700 bg-[#18181b] px-3 py-1.5 font-medium text-zinc-300 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
              >
                ${escapeHtml(t("licensesFullFileBtn"))} ↗
              </button>
              <button
                id="btn-cancel-modal"
                type="button"
                class="rounded-md bg-[#ff9600] px-4 py-1.5 font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
              >
                ${escapeHtml(t("creditsCloseBtn"))}
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  `;
}

function renderAccessModalHtml(): string {
  const tier = getEffectiveTier();
  const tierLabel = formatAccessTierName(tier);

  return `
    <div id="modal-backdrop" class="fixed inset-0 z-40 flex items-center justify-center bg-black/75 backdrop-blur-[2px] p-4">
      <div class="w-full max-w-md rounded-xl border border-zinc-700 bg-[#252528] shadow-2xl overflow-hidden">
        <div class="flex items-center justify-between border-b border-zinc-800 px-5 py-3.5 bg-[#202023]">
          <h2 class="text-sm font-bold text-[#ff9600] tracking-wide uppercase">
            🔑 ${escapeHtml(t("accessModalTitle"))}
          </h2>
          <button
            id="btn-close-modal"
            type="button"
            class="text-zinc-400 hover:text-zinc-100 text-sm px-1.5 py-0.5 rounded cursor-pointer"
          >
            ✕
          </button>
        </div>

        <form id="form-access-tier" class="p-5 space-y-4 text-xs">
          <div class="flex items-center justify-between rounded-lg border border-zinc-800 bg-[#18181b] px-3.5 py-2.5">
            <span class="text-zinc-400">${escapeHtml(t("accessModalCurrent"))}</span>
            <span class="font-semibold text-[#ff9600]">${escapeHtml(tierLabel)}</span>
          </div>

          <div>
            <label for="input-access-key" class="block font-semibold text-zinc-200 mb-1">
              ${escapeHtml(t("accessModalKeyLabel"))}
            </label>
            <input
              id="input-access-key"
              type="password"
              autocomplete="off"
              placeholder="${escapeHtml(t("accessModalKeyPlaceholder"))}"
              class="w-full rounded-md border border-zinc-700 bg-[#18181b] px-3 py-2 font-mono text-zinc-100 focus:border-[#ff9600] focus:outline-none"
            />
            <p class="mt-1.5 text-[11px] text-zinc-400">${escapeHtml(t("accessModalHint"))}</p>
            ${
              state.accessKeyError
                ? `<p class="mt-1.5 text-xs font-medium text-red-400">⚠ ${escapeHtml(state.accessKeyError)}</p>`
                : ""
            }
          </div>

          <div class="flex flex-wrap items-center justify-between gap-2 pt-3 border-t border-zinc-800">
            ${
              tier !== "user"
                ? `
              <button
                id="btn-reset-access-tier"
                type="button"
                class="rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 font-medium text-amber-300 hover:bg-amber-500/20 transition cursor-pointer"
              >
                ${escapeHtml(t("accessModalResetBtn"))}
              </button>
            `
                : `<span></span>`
            }
            <div class="flex items-center gap-2 ml-auto">
              <button
                id="btn-cancel-modal"
                type="button"
                class="rounded-md border border-zinc-700 bg-zinc-800 px-3 py-1.5 font-medium text-zinc-200 hover:bg-zinc-700 transition cursor-pointer"
              >
                ${escapeHtml(t("settingsCancel"))}
              </button>
              <button
                type="submit"
                class="rounded-md bg-[#ff9600] px-4 py-1.5 font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
              >
                ${escapeHtml(t("accessModalUnlockBtn"))}
              </button>
            </div>
          </div>
        </form>
      </div>
    </div>
  `;
}

function renderActiveModalHtml(): string {
  switch (state.activeModal) {
    case "settings":
      return renderSettingsModalHtml();
    case "credits":
      return renderCreditsModalHtml();
    case "licenses":
      return renderLicensesModalHtml();
    case "access":
      return renderAccessModalHtml();
    default:
      return "";
  }
}

function normalizeLangForForm(lang: string): string {
  const lower = lang.toLowerCase();
  if (lower === "de" || lower === "en" || lower === "fr" || lower === "es" || lower === "pl") {
    return lower;
  }
  return "en";
}

function updateToastBanner(): void {
  const toastEl = document.querySelector<HTMLDivElement>("#toast-container");
  if (!toastEl) return;
  if (!state.toastMessage) {
    toastEl.innerHTML = "";
    return;
  }
  toastEl.innerHTML = `
    <div class="rounded-lg border border-[#ff9600]/60 bg-[#252528] px-3.5 py-2 text-xs font-medium text-zinc-100 shadow-lg">
      ${escapeHtml(state.toastMessage)}
    </div>
  `;
}

function renderDashboardOnly(): void {
  const dashEl = document.querySelector<HTMLDivElement>("#dashboard-root");
  if (!dashEl) return;

  const prevScrollBox = document.querySelector<HTMLDivElement>("#log-scroll-box");
  const wasNearBottom = prevScrollBox
    ? prevScrollBox.scrollHeight - prevScrollBox.scrollTop - prevScrollBox.clientHeight < 36
    : true;
  const prevScrollTop = prevScrollBox?.scrollTop ?? 0;

  dashEl.innerHTML = renderDashboardHtml();
  bindDashboardEvents();

  const nextScrollBox = document.querySelector<HTMLDivElement>("#log-scroll-box");
  if (nextScrollBox) {
    nextScrollBox.scrollTop =
      state.config.logAutoScroll && wasNearBottom
        ? nextScrollBox.scrollHeight
        : prevScrollTop;
  }
}

function renderModalOnly(): void {
  const modalEl = document.querySelector<HTMLDivElement>("#modal-root");
  if (!modalEl) return;
  modalEl.innerHTML = renderActiveModalHtml();
  bindModalEvents();
}

function openAccessModal(): void {
  versionClickCount = 0;
  state.accessKeyError = null;
  state.activeModal = "access";
  renderModalOnly();
  document.querySelector<HTMLInputElement>("#input-access-key")?.focus();
}

function handleSecretVersionClick(): void {
  versionClickCount += 1;
  if (versionClickResetTimer !== undefined) {
    window.clearTimeout(versionClickResetTimer);
  }
  versionClickResetTimer = window.setTimeout(() => {
    versionClickCount = 0;
  }, SECRET_CLICK_WINDOW_MS);

  if (versionClickCount >= SECRET_CLICK_THRESHOLD) {
    openAccessModal();
  }
}

function renderAll(): void {
  const root = document.querySelector<HTMLDivElement>("#app");
  if (!root) return;

  if (!document.querySelector("#dashboard-root")) {
    root.innerHTML = `
      <main id="dashboard-root" class="flex flex-col gap-2.5 h-screen p-4 bg-[#18181b] text-zinc-100 select-none overflow-hidden"></main>
      <div id="modal-root"></div>
      <div id="toast-container" class="fixed bottom-14 right-4 z-50 pointer-events-none"></div>
    `;
    const dashRoot = document.querySelector<HTMLElement>("#dashboard-root");
    dashRoot?.addEventListener("pointerdown", (ev) => {
      if (ev.button !== 0) return;
      const target = ev.target as HTMLElement | null;
      if (target?.closest("[data-secret-trigger='true']")) {
        handleSecretVersionClick();
      }
    });
  }

  renderDashboardOnly();
  renderModalOnly();
  updateToastBanner();
}

function appendLogInPlace(entry: LogEntry): void {
  state.logs.push(entry);
  if (state.logs.length > MAX_UI_LOGS) {
    state.logs.shift();
  }

  const scrollBox = document.querySelector<HTMLDivElement>("#log-scroll-box");
  const listEl = document.querySelector<HTMLDivElement>("#log-list");
  if (!scrollBox || !listEl) return;

  const prevScrollTop = scrollBox.scrollTop;
  const wasNearBottom =
    scrollBox.scrollHeight - prevScrollTop - scrollBox.clientHeight < 36;
  const shouldAutoScroll = state.config.logAutoScroll && wasNearBottom;

  let removedHeight = 0;
  if (state.logs.length === 1) {
    listEl.innerHTML = renderLogEntryHtml(entry);
  } else {
    listEl.insertAdjacentHTML("beforeend", renderLogEntryHtml(entry));
    while (listEl.children.length > MAX_UI_LOGS) {
      const oldest = listEl.firstElementChild as HTMLElement | null;
      if (!oldest) break;
      removedHeight += oldest.offsetHeight;
      oldest.remove();
    }
  }

  if (shouldAutoScroll) {
    scrollBox.scrollTop = scrollBox.scrollHeight;
  } else if (removedHeight > 0 && prevScrollTop > 0) {
    scrollBox.scrollTop = Math.max(0, prevScrollTop - removedHeight);
  }
}

function applyAdaptiveLogoSize(logoImg: HTMLImageElement): void {
  if (logoImg.naturalHeight <= 0 || logoImg.naturalWidth <= 0) return;
  const ratio = logoImg.naturalWidth / logoImg.naturalHeight;
  const isSquare = ratio < 1.45;
  squareLogoCache.set(getSafeHeaderLogoUrl(), isSquare);
  logoImg.classList.toggle("h-[104px]", isSquare);
  logoImg.classList.toggle("h-[72px]", !isSquare);
}

function bindDashboardEvents(): void {
  const logoImg = document.querySelector<HTMLImageElement>("#img-header-logo");
  if (logoImg) {
    if (logoImg.complete && logoImg.naturalHeight > 0) {
      applyAdaptiveLogoSize(logoImg);
    } else {
      logoImg.addEventListener(
        "load",
        () => applyAdaptiveLogoSize(logoImg),
        { once: true },
      );
    }
    logoImg.addEventListener(
      "error",
      () => {
        if (logoImg.src !== dsaLogoUrl) {
          logoImg.src = dsaLogoUrl;
          logoImg.classList.remove("h-[104px]");
          logoImg.classList.add("h-[72px]");
        }
      },
      { once: true },
    );
  }

  document
    .querySelector<HTMLButtonElement>("#btn-open-access-badge")
    ?.addEventListener("click", openAccessModal);

  document
    .querySelector<HTMLSelectElement>("#select-quick-lang")
    ?.addEventListener("change", async (ev) => {
      const target = ev.currentTarget as HTMLSelectElement;
      const nextLang = setLocale(target.value);
      state.config.language = nextLang;
      renderAll();
      try {
        const snap = await invoke<BridgeSnapshot>("update_app_config", {
          config: state.config,
        });
        applySnapshot(snap);
        renderDashboardOnly();
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-open-settings")
    ?.addEventListener("click", () => {
      state.activeModal = "settings";
      renderModalOnly();
    });

  document
    .querySelector<HTMLButtonElement>("#btn-toggle-path-help")
    ?.addEventListener("click", () => {
      state.pathHelpOpen = !state.pathHelpOpen;
      renderDashboardOnly();
    });

  document
    .querySelector<HTMLButtonElement>("#btn-close-path-help")
    ?.addEventListener("click", () => {
      state.pathHelpOpen = false;
      renderDashboardOnly();
    });

  document
    .querySelector<HTMLButtonElement>("#btn-browse-path")
    ?.addEventListener("click", async () => {
      try {
        const picked = await openFolderDialog({
          directory: true,
          multiple: false,
          title: t("pathDialogTitle"),
          defaultPath: state.snapshot.profilePath ?? state.config.fs25ProfilePath ?? undefined,
        });
        if (typeof picked === "string" && picked.trim().length > 0) {
          const snap = await invoke<BridgeSnapshot>("set_fs25_profile_path", {
            path: picked,
          });
          applySnapshot(snap);
          state.pathHelpOpen = false;
          renderDashboardOnly();
          showToast(t("toastPathUpdated"));
        }
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-open-releases")
    ?.addEventListener("click", () => void openExternalUrl(GITHUB_RELEASES_URL));

  const switchDevEnvironment = async (
    nextMode: BackendMode,
    nextUrl?: string,
  ): Promise<void> => {
    if (getEffectiveTier() !== "developer") {
      return;
    }
    const currentUrl = normalizeUrlForCompare(
      state.snapshot.backendUrl || state.config.backendUrl,
    );
    const targetUrl = nextUrl ?? state.config.backendUrl;
    if (
      getEffectiveBackendMode() === nextMode &&
      (nextMode === "mock" || currentUrl === normalizeUrlForCompare(targetUrl))
    ) {
      return;
    }
    const nextConfig: AppConfig = {
      ...state.config,
      backendMode: nextMode,
      backendUrl: targetUrl,
    };
    try {
      const snap = await invoke<BridgeSnapshot>("update_app_config", {
        config: nextConfig,
      });
      applySnapshot(snap);
      if (nextMode === "mock") {
        state.mockToolsExpanded = true;
      }
      renderDashboardOnly();
    } catch (err) {
      showToast(String(err));
    }
  };

  document
    .querySelector<HTMLButtonElement>("#btn-dev-switch-prod")
    ?.addEventListener("click", () =>
      void switchDevEnvironment("live", OFFICIAL_PROD_BACKEND_URL),
    );

  document
    .querySelector<HTMLButtonElement>("#btn-dev-switch-dev")
    ?.addEventListener("click", () =>
      void switchDevEnvironment("live", OFFICIAL_DEV_BACKEND_URL),
    );

  document
    .querySelector<HTMLButtonElement>("#btn-dev-switch-mock")
    ?.addEventListener("click", () => void switchDevEnvironment("mock"));

  document
    .querySelector<HTMLButtonElement>("#btn-toggle-mock-tools")
    ?.addEventListener("click", () => {
      state.mockToolsExpanded = !state.mockToolsExpanded;
      renderDashboardOnly();
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-gd")
    ?.addEventListener("click", async () => {
      try {
        await invoke("mock_trigger_great_demand", { fillType: null });
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-approve")
    ?.addEventListener("click", async () => {
      try {
        await invoke("mock_approve_pending");
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-offline")
    ?.addEventListener("click", async () => {
      state.mockSimulatedOffline = !state.mockSimulatedOffline;
      renderDashboardOnly();
      try {
        await invoke("mock_set_offline", { offline: state.mockSimulatedOffline });
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-minver")
    ?.addEventListener("click", async () => {
      state.mockLockedMinVersion = !state.mockLockedMinVersion;
      const targetVer = state.mockLockedMinVersion ? "9.9.9" : "1.0.0";
      renderDashboardOnly();
      try {
        await invoke("mock_set_min_version", { minVersion: targetVer });
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-logo")
    ?.addEventListener("click", async () => {
      try {
        const picked = await openFolderDialog({
          directory: false,
          multiple: false,
          title: t("mockLogoDialogTitle"),
          filters: [
            {
              name: "PNG / WebP / JPG / DDS",
              extensions: ["png", "webp", "jpg", "jpeg", "dds"],
            },
          ],
        });
        if (typeof picked === "string" && picked.trim().length > 0) {
          const snap = await invoke<BridgeSnapshot>("mock_set_community_logo", {
            path: picked,
          });
          applySnapshot(snap);
          renderDashboardOnly();
        }
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-logo-reset")
    ?.addEventListener("click", async () => {
      try {
        const snap = await invoke<BridgeSnapshot>("mock_set_community_logo", {
          path: null,
        });
        applySnapshot(snap);
        renderDashboardOnly();
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLInputElement>("#chk-log-autoscroll")
    ?.addEventListener("change", async (ev) => {
      const target = ev.currentTarget as HTMLInputElement;
      state.config.logAutoScroll = target.checked;
      if (state.config.logAutoScroll) {
        const scrollBox = document.querySelector<HTMLDivElement>("#log-scroll-box");
        if (scrollBox) {
          scrollBox.scrollTop = scrollBox.scrollHeight;
        }
      }
      try {
        const snap = await invoke<BridgeSnapshot>("update_app_config", {
          config: state.config,
        });
        applySnapshot(snap);
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-copy-diag")
    ?.addEventListener("click", async () => {
      try {
        const report = await invoke<string>("generate_diagnostic_report");
        try {
          await writeText(report);
        } catch {
          await navigator.clipboard.writeText(report);
        }
        state.copiedDiagFeedback = true;
        renderDashboardOnly();
        if (copyFeedbackTimer !== undefined) {
          window.clearTimeout(copyFeedbackTimer);
        }
        copyFeedbackTimer = window.setTimeout(() => {
          state.copiedDiagFeedback = false;
          renderDashboardOnly();
        }, 2500);
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-open-discord")
    ?.addEventListener("click", () => void openExternalUrl(DSA_DISCORD_URL));

  document
    .querySelector<HTMLButtonElement>("#btn-open-github")
    ?.addEventListener("click", () => void openExternalUrl(GITHUB_REPO_URL));

  document
    .querySelector<HTMLButtonElement>("#btn-open-credits")
    ?.addEventListener("click", () => {
      state.activeModal = "credits";
      renderModalOnly();
    });

  document
    .querySelector<HTMLButtonElement>("#btn-quit-app")
    ?.addEventListener("click", async () => {
      try {
        await invoke("quit_app");
      } catch {
        window.close();
      }
    });
}

function bindModalEvents(): void {
  const closeModal = (): void => {
    state.activeModal = "none";
    state.accessKeyError = null;
    renderModalOnly();
  };

  document
    .querySelector<HTMLButtonElement>("#btn-close-modal")
    ?.addEventListener("click", closeModal);

  document
    .querySelector<HTMLButtonElement>("#btn-cancel-modal")
    ?.addEventListener("click", closeModal);

  document
    .querySelector<HTMLDivElement>("#modal-backdrop")
    ?.addEventListener("click", (ev) => {
      if (ev.target === ev.currentTarget) {
        closeModal();
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-open-licenses")
    ?.addEventListener("click", () => {
      state.activeModal = "licenses";
      renderModalOnly();
    });

  document
    .querySelector<HTMLButtonElement>("#btn-back-credits")
    ?.addEventListener("click", () => {
      state.activeModal = "credits";
      renderModalOnly();
    });

  document.querySelectorAll<HTMLButtonElement>("[data-ext-url]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const url = btn.getAttribute("data-ext-url");
      if (url) {
        void openExternalUrl(url);
      }
    });
  });

  document.querySelectorAll<HTMLButtonElement>("[data-preset-url]").forEach((btn) => {
    btn.addEventListener("click", () => {
      const preset = btn.getAttribute("data-preset-url");
      const urlInput = document.querySelector<HTMLInputElement>("#cfg-url");
      const modeSelect = document.querySelector<HTMLSelectElement>("#cfg-mode");
      if (preset && urlInput) {
        urlInput.value = preset;
      }
      if (modeSelect) {
        modeSelect.value = "live";
      }
    });
  });

  document
    .querySelector<HTMLFormElement>("#form-settings")
    ?.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const isDev = getEffectiveTier() === "developer";
      const langVal =
        document.querySelector<HTMLSelectElement>("#cfg-lang")?.value ?? "de";
      const trayVal =
        document.querySelector<HTMLInputElement>("#cfg-tray")?.checked ?? true;
      const notifyVal =
        document.querySelector<HTMLInputElement>("#cfg-notify")?.checked ?? true;
      const autoScrollVal =
        document.querySelector<HTMLInputElement>("#cfg-autoscroll")?.checked ??
        true;
      const modeVal = isDev
        ? ((document.querySelector<HTMLSelectElement>("#cfg-mode")
            ?.value as BackendMode) ?? state.config.backendMode)
        : "live";
      const urlVal = isDev
        ? document.querySelector<HTMLInputElement>("#cfg-url")?.value.trim() ||
          state.config.backendUrl
        : state.config.backendUrl;
      const tokenRaw =
        document.querySelector<HTMLInputElement>("#cfg-token")?.value.trim() ?? "";

      const nextConfig: AppConfig = {
        ...state.config,
        language: setLocale(langVal),
        minimizeToTray: trayVal,
        notificationsEnabled: notifyVal,
        logAutoScroll: autoScrollVal,
        backendMode: modeVal,
        backendUrl: urlVal,
        marketToken: tokenRaw.length > 0 ? tokenRaw : null,
      };

      try {
        const snap = await invoke<BridgeSnapshot>("update_app_config", {
          config: nextConfig,
        });
        state.config = {
          ...nextConfig,
        };
        applySnapshot(snap);
        state.activeModal = "none";
        renderAll();
        await ensureNotificationPermissionIfEnabled();
        showToast(t("toastSettingsSaved"));
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLFormElement>("#form-access-tier")
    ?.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const rawKey =
        document.querySelector<HTMLInputElement>("#input-access-key")?.value ?? "";
      try {
        const updatedCfg = await invoke<AppConfig>("unlock_access_tier", {
          key: rawKey,
        });
        const snap = await invoke<BridgeSnapshot>("get_bridge_snapshot");
        state.config = updatedCfg;
        applySnapshot(snap);
        state.accessKeyError = null;
        state.activeModal = "none";
        renderAll();
        showToast(
          t("toastAccessUnlocked", {
            tier: formatAccessTierName(updatedCfg.accessTier),
          }),
        );
      } catch {
        state.accessKeyError = t("accessModalInvalidKey");
        renderModalOnly();
        document.querySelector<HTMLInputElement>("#input-access-key")?.focus();
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-reset-access-tier")
    ?.addEventListener("click", async () => {
      try {
        const updatedCfg = await invoke<AppConfig>("reset_access_tier");
        const snap = await invoke<BridgeSnapshot>("get_bridge_snapshot");
        state.config = updatedCfg;
        applySnapshot(snap);
        state.accessKeyError = null;
        state.activeModal = "none";
        renderAll();
        showToast(t("toastAccessReset"));
      } catch (err) {
        showToast(String(err));
      }
    });
}

async function bootstrap(): Promise<void> {
  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      if (state.activeModal !== "none") {
        state.activeModal = "none";
        state.accessKeyError = null;
        renderModalOnly();
      } else if (state.pathHelpOpen) {
        state.pathHelpOpen = false;
        renderDashboardOnly();
      }
    }
  });

  try {
    const [info, config, snapshot, logs] = await Promise.all([
      invoke<AppInfo>("get_app_info"),
      invoke<AppConfig>("get_app_config"),
      invoke<BridgeSnapshot>("get_bridge_snapshot"),
      invoke<LogEntry[]>("get_bridge_logs"),
    ]);
    state.info = info;
    state.config = config;
    applySnapshot(snapshot);
    state.logs = logs;
    setLocale(config.language);
  } catch {
    setLocale("de");
    syncDocumentTitle();
  }

  renderAll();
  const scrollBox = document.querySelector<HTMLDivElement>("#log-scroll-box");
  if (scrollBox && state.config.logAutoScroll) {
    scrollBox.scrollTop = scrollBox.scrollHeight;
  }

  void ensureNotificationPermissionIfEnabled();

  try {
    await listen<BridgeSnapshot>("bridge://status", (event) => {
      const prevTier = getEffectiveTier();
      const prevSig = JSON.stringify({
        ...state.snapshot,
        // Ignore lastSyncUnix changes when hidden in Standard User mode so DOM stays untouched on background polls
        lastSyncUnix:
          prevTier === "user" ? null : state.snapshot.lastSyncUnix,
      });
      applySnapshot(event.payload);
      if (prevTier !== event.payload.accessTier && state.activeModal === "settings") {
        renderModalOnly();
      }
      const nextTier = getEffectiveTier();
      const nextSig = JSON.stringify({
        ...state.snapshot,
        lastSyncUnix:
          nextTier === "user" ? null : state.snapshot.lastSyncUnix,
      });
      if (prevSig !== nextSig) {
        renderDashboardOnly();
      }
    });

    await listen<LogEntry>("bridge://log", (event) => {
      appendLogInPlace(event.payload);
    });

    await listen<GreatDemandNotification>("bridge://great-demand", (event) => {
      const displayTitle =
        localizeFillTypeCode(event.payload.fillType) ?? event.payload.title;
      showToast(
        t("toastGreatDemand", {
          title: displayTitle,
          bonus: event.payload.bonusPercent,
        }),
      );
    });

    // Fallback check so Extended (Diagnostic) mode expires cleanly even when in `pathMissing` state.
    window.setInterval(async () => {
      if (getEffectiveTier() === "extended") {
        try {
          const snap = await invoke<BridgeSnapshot>("get_bridge_snapshot");
          if (snap.accessTier !== "extended") {
            applySnapshot(snap);
            renderAll();
          }
        } catch {
          // Ignore transient IPC errors.
        }
      }
    }, 15_000);
  } catch {
    // Running in standalone browser preview outside Tauri IPC.
  }
}

void bootstrap();
