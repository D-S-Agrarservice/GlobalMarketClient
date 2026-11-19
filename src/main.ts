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
import { relaunch } from "@tauri-apps/plugin-process";
import { check as checkAppUpdate, type Update } from "@tauri-apps/plugin-updater";
import dsaLogoUrl from "./assets/dsa-logo.png";
import {
  formatAccessTierName,
  formatLogEntryMessage,
  getLocale,
  LANGUAGE_OPTIONS,
  localizeFillTypeCode,
  normalizeLocale,
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
  BYDORA_URL,
  DSA_DISCORD_URL,
  GITHUB_RELEASES_URL,
  GITHUB_REPO_URL,
  type GreatDemandNotification,
  type HeartbeatTickPayload,
  LOCAL_DEV_BACKEND_URL,
  type LogEntry,
  OFFICIAL_DEV_BACKEND_URL,
  OFFICIAL_PROD_BACKEND_URL,
  OPEN_SOURCE_PACKAGES,
  type SavedTokenProfile,
  type ThemePreference,
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
  mockOptionalUpdate: boolean;
  dismissedOptionalVersion: string | null;
  copiedDiagFeedback: boolean;
  toastMessage: string | null;
  checkingForUpdate: boolean;
  availableUpdateVersion: string | null;
  updateInstallingState: "idle" | "downloading" | "installing";
  updateDownloadPercent: number;
  togglingConnection: boolean;
}

const state: UiState = {
  info: {
    appVersion: "0.9.14",
    protocolVersion: "0.9.14",
    isBeta: true,
    platform: "desktop",
  },
  config: {
    accessTier: "user",
    fs25ProfilePath: null,
    language: "de",
    theme: "dark",
    minimizeToTray: true,
    notificationsEnabled: true,
    logAutoScroll: true,
    autoConnectOnLaunch: true,
    backendMode: "live",
    backendUrl: OFFICIAL_PROD_BACKEND_URL,
    activeTokenId: null,
  },
  snapshot: {
    state: "connecting",
    connectionEnabled: true,
    gameEdition: null,
    accessTier: "user",
    backendMode: "live",
    backendUrl: OFFICIAL_PROD_BACKEND_URL,
    apiVersion: null,
    marketName: "Global Market",
    brandingTier: 0,
    isPrivateMarket: false,
    marketLogoUrl: null,
    marketLogoFile: null,
    activeTokenId: null,
    savedTokens: [],
    profilePath: null,
    bridgePath: null,
    exeVersion: "0.9.14",
    minExeVersion: "0.9.9",
    latestExeVersion: null,
    contractsSupported: false,
    openContractsCount: 0,
    openOffersCount: 0,
    approvedCount: 0,
    pendingCount: 0,
    rejectedCount: 0,
    readQueueCount: 0,
    writeQueueCount: 0,
    activeGreatDemands: [],
    activeGreatDemandCodes: [],
    heartbeatSeq: 0,
    lastHeartbeatUnix: null,
    lastSyncUnix: null,
    lastError: null,
    extendedExpiresUnix: null,
  },
  logs: [],
  activeModal: "none",
  pathHelpOpen: false,
  accessKeyError: null,
  mockToolsExpanded: true,
  mockSimulatedOffline: false,
  mockLockedMinVersion: false,
  mockOptionalUpdate: false,
  dismissedOptionalVersion: null,
  copiedDiagFeedback: false,
  toastMessage: null,
  checkingForUpdate: false,
  availableUpdateVersion: null,
  updateInstallingState: "idle",
  updateDownloadPercent: 0,
  togglingConnection: false,
};

let pendingAppUpdate: Update | null = null;

let toastTimer: number | undefined;
let copyFeedbackTimer: number | undefined;
let versionClickCount = 0;
let versionClickResetTimer: number | undefined;
let previewingUnsavedTheme = false;

const systemDarkMedia =
  typeof window !== "undefined" && typeof window.matchMedia === "function"
    ? window.matchMedia("(prefers-color-scheme: dark)")
    : null;

function normalizeTheme(raw?: string | null): ThemePreference {
  const lower = (raw ?? "").trim().toLowerCase();
  if (lower === "light") return "light";
  if (lower === "system" || lower === "auto") return "system";
  return "dark";
}

function resolveEffectiveTheme(pref: ThemePreference): "dark" | "light" {
  if (pref === "light") return "light";
  if (pref === "system") {
    return systemDarkMedia && !systemDarkMedia.matches ? "light" : "dark";
  }
  return "dark";
}

function applyThemeToDom(prefRaw?: string | null): ThemePreference {
  const pref = normalizeTheme(prefRaw ?? state.config.theme);
  const effective = resolveEffectiveTheme(pref);
  document.documentElement.setAttribute("data-theme", effective);
  document.documentElement.style.colorScheme = effective;
  return pref;
}

function revertPreviewedThemeIfNeeded(): void {
  if (!previewingUnsavedTheme) return;
  previewingUnsavedTheme = false;
  const saved = applyThemeToDom(state.config.theme);
  void invoke("preview_window_theme", { theme: saved }).catch(() => {});
}

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

async function checkForAppUpdates(manual: boolean): Promise<void> {
  if (state.checkingForUpdate || state.updateInstallingState !== "idle") {
    return;
  }
  state.checkingForUpdate = true;
  if (manual && state.activeModal === "settings") {
    renderModalOnly();
  }

  try {
    const found = await checkAppUpdate({ timeout: 8000 });
    if (pendingAppUpdate && pendingAppUpdate !== found) {
      try {
        await pendingAppUpdate.close();
      } catch {
        // Ignore cleanup errors on previous resource handle.
      }
    }
    pendingAppUpdate = found;
    state.availableUpdateVersion = found ? found.version : null;
    state.checkingForUpdate = false;

    if (state.activeModal === "settings") {
      renderModalOnly();
    }
    renderDashboardOnly();

    if (manual) {
      if (found) {
        state.activeModal = "none";
        renderModalOnly();
        showToast(t("toastUpdateFound", { version: found.version }));
      } else {
        showToast(t("toastUpToDate", { version: state.info.appVersion }));
      }
    }
  } catch {
    state.checkingForUpdate = false;
    if (state.activeModal === "settings") {
      renderModalOnly();
    }
    if (manual) {
      // Before the first public release is published (or when offline), latest.json returns 404;
      // surface a calm up-to-date notice rather than a raw HTTP error.
      showToast(t("toastUpToDate", { version: state.info.appVersion }));
    }
  }
}

async function installPendingAppUpdate(): Promise<void> {
  if (!pendingAppUpdate || state.updateInstallingState !== "idle") {
    void openExternalUrl(GITHUB_RELEASES_URL);
    return;
  }

  state.updateInstallingState = "downloading";
  state.updateDownloadPercent = 0;
  renderDashboardOnly();

  let totalBytes = 0;
  let receivedBytes = 0;

  try {
    await pendingAppUpdate.download((ev) => {
      if (ev.event === "Started") {
        totalBytes = ev.data.contentLength ?? 0;
        receivedBytes = 0;
      } else if (ev.event === "Progress") {
        receivedBytes += ev.data.chunkLength;
        if (totalBytes > 0) {
          const pct = Math.min(99, Math.round((receivedBytes / totalBytes) * 100));
          if (pct !== state.updateDownloadPercent) {
            state.updateDownloadPercent = pct;
            const labelEl = document.querySelector<HTMLSpanElement>("#update-btn-label");
            const barEl = document.querySelector<HTMLDivElement>("#update-progress-bar");
            if (labelEl) {
              labelEl.textContent = t("updateDownloading", { percent: pct });
            }
            if (barEl) {
              barEl.style.width = `${pct}%`;
            }
          }
        }
      } else if (ev.event === "Finished") {
        state.updateDownloadPercent = 100;
      }
    });

    state.updateInstallingState = "installing";
    state.updateDownloadPercent = 100;
    renderDashboardOnly();

    try {
      await invoke("prepare_for_update");
    } catch {
      // Proceed with installation even if worker was already stopped.
    }

    await pendingAppUpdate.install({ restartAfterInstall: true });
    await relaunch();
  } catch (err) {
    state.updateInstallingState = "idle";
    state.updateDownloadPercent = 0;
    renderDashboardOnly();
    showToast(t("toastUpdateFailed", { error: String(err) }));
    void openExternalUrl(GITHUB_RELEASES_URL);
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

function isOfficialMarketLabel(raw: string): boolean {
  const lower = raw.trim().toLowerCase();
  return (
    !lower ||
    lower === "global market" ||
    lower === "global market official" ||
    lower === "official global market" ||
    lower === "local simulation (mock)"
  );
}

function getDisplayMarketName(isMock: boolean): string {
  const raw = state.snapshot.marketName?.trim() ?? "";
  if (isMock) {
    if (state.snapshot.isPrivateMarket && !isOfficialMarketLabel(raw)) {
      return raw;
    }
    return t("marketNameMock");
  }
  if (!state.snapshot.isPrivateMarket || isOfficialMarketLabel(raw)) {
    return t("officialMarketName");
  }
  return raw;
}

function normalizeUrlForCompare(raw: string): string {
  return raw.trim().replace(/\/+$/, "").toLowerCase();
}

function getActiveTokenId(): string | null {
  return state.snapshot.activeTokenId ?? state.config.activeTokenId ?? null;
}

function getSavedTokens(): SavedTokenProfile[] {
  return state.snapshot.savedTokens ?? [];
}

function formatSwitcherOptionLabel(prof: SavedTokenProfile): string {
  if (prof.brandingTier === 1) {
    return prof.customLabel
      ? `🖼 ${prof.customLabel} · Global (${prof.maskedHint})`
      : `🖼 ${t("switcherOfficialWithLogo")} (${prof.maskedHint})`;
  }
  if (prof.brandingTier === 2 || prof.brandingTier === 3 || prof.isPrivateMarket) {
    const title =
      prof.customLabel || prof.detectedMarketName || t("switcherUnverifiedProfile");
    return `🔒 ${title} (${prof.maskedHint})`;
  }
  const fallback =
    prof.customLabel || prof.detectedMarketName || t("switcherUnverifiedProfile");
  return `🔑 ${fallback} (${prof.maskedHint})`;
}

function formatProfileCardTitle(prof: SavedTokenProfile): string {
  if (prof.brandingTier === 1) {
    return prof.customLabel || t("switcherOfficialWithLogo");
  }
  if (
    prof.customLabel &&
    prof.detectedMarketName &&
    prof.customLabel !== prof.detectedMarketName
  ) {
    return `${prof.customLabel} (${prof.detectedMarketName})`;
  }
  return (
    prof.customLabel || prof.detectedMarketName || t("switcherUnverifiedProfile")
  );
}

function formatProfileTierBadge(prof: SavedTokenProfile): {
  text: string;
  classes: string;
} {
  switch (prof.brandingTier) {
    case 1:
      return {
        text: t("settingsProfileTier1Badge"),
        classes: "border-sky-500/35 bg-sky-500/10 text-sky-300",
      };
    case 2:
      return {
        text: t("settingsProfileTier2Badge"),
        classes: "border-[#ff9600]/35 bg-[#ff9600]/10 text-[#ff9600]",
      };
    case 3:
      return {
        text: t("settingsProfileTier3Badge"),
        classes: "border-[#ff9600]/45 bg-[#ff9600]/15 text-amber-200",
      };
    default:
      return {
        text: t("settingsProfilePendingBadge"),
        classes: "border-zinc-700 bg-zinc-800 text-zinc-400",
      };
  }
}

function getSafeHeaderLogoUrl(): string {
  const isMock =
    getEffectiveTier() === "developer" && getEffectiveBackendMode() === "mock";
  const hasToken = Boolean(getActiveTokenId()?.trim());
  const custom = state.snapshot.marketLogoUrl?.trim();
  const tierAllowsLogo =
    state.snapshot.brandingTier === 1 || state.snapshot.brandingTier === 3;
  if ((!hasToken && !isMock) || !tierAllowsLogo || !custom) {
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

function parseVersionParts(raw: string): number[] {
  return raw
    .split(/[^0-9]+/)
    .filter((part) => part.length > 0)
    .map((part) => Number.parseInt(part, 10))
    .filter((num) => Number.isFinite(num));
}

function isSemverSufficient(currentVersion: string, requiredVersion: string): boolean {
  const cur = parseVersionParts(currentVersion);
  const req = parseVersionParts(requiredVersion);
  if (cur.length === 0) return false;
  for (let i = 0; i < 4; i += 1) {
    const c = cur[i] ?? 0;
    const r = req[i] ?? 0;
    if (c > r) return true;
    if (c < r) return false;
  }
  return true;
}

function getServerRecommendedUpdate(): string | null {
  const latest = state.snapshot.latestExeVersion?.trim();
  if (!latest) return null;
  return isSemverSufficient(state.snapshot.exeVersion, latest) ? null : latest;
}

function syncDocumentTitle(): void {
  const suffix = state.info.isBeta ? " (Beta)" : "";
  document.title = `${getBrandTitle()} – D-S-Agrarservice${suffix}`;
}

function applySnapshot(snap: BridgeSnapshot): void {
  state.snapshot = snap;
  state.config.accessTier = snap.accessTier;
  state.config.backendMode = snap.backendMode;
  state.config.backendUrl = snap.backendUrl;
  state.config.activeTokenId = snap.activeTokenId;
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
    case "disconnected":
      return {
        label: t("stateDisconnected"),
        dotClass: "bg-zinc-500",
        pillClass: "border-zinc-600/80 bg-zinc-800 text-zinc-300",
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

function formatLifebitTooltip(seq: number, lastUnix: number | null): string {
  if (seq > 0 && lastUnix !== null) {
    return t("lifebitTooltip", {
      seq,
      time: formatLocalTime(lastUnix * 1000),
    });
  }
  return t("lifebitWaitingTooltip");
}

function formatSnapshotError(rawError: string | null): string | null {
  if (!rawError) return null;
  const slotMatch = /^Key slot limit reached \(max\. (\d+) active client slots\)$/i.exec(
    rawError.trim(),
  );
  if (slotMatch) {
    return t("logTokenSlotLimitReached", { maxSlots: slotMatch[1] });
  }
  if (rawError.startsWith("Unauthorized (")) {
    return t("logTokenInvalid");
  }
  return rawError;
}

function syncLifebitLedsInPlace(seq: number, lastUnix: number | null): void {
  const pill = document.querySelector<HTMLDivElement>("#status-pill");
  if (pill && state.snapshot.state === "online") {
    pill.title = formatLifebitTooltip(seq, lastUnix);
  }

  const ledA = document.querySelector<HTMLSpanElement>("#lifebit-led-a");
  const ledB = document.querySelector<HTMLSpanElement>("#lifebit-led-b");
  if (ledA && ledB) {
    const bitOne = seq > 0 && seq % 2 === 1;
    const bitZeroActive = seq > 0 && seq % 2 === 0;
    ledA.className = bitOne
      ? "inline-block h-1.5 w-1.5 rounded-full bg-emerald-300 shadow-[0_0_6px_rgba(110,231,183,0.95)] transition-all duration-300"
      : "inline-block h-1.5 w-1.5 rounded-full bg-emerald-500/25 transition-all duration-300";
    ledB.className = bitZeroActive
      ? "inline-block h-1.5 w-1.5 rounded-full bg-emerald-300 shadow-[0_0_6px_rgba(110,231,183,0.95)] transition-all duration-300"
      : "inline-block h-1.5 w-1.5 rounded-full bg-emerald-500/25 transition-all duration-300";
  }

  const debugVal = document.querySelector<HTMLSpanElement>("#debug-lifebit-val");
  if (debugVal) {
    debugVal.textContent = seq > 0 ? `#${seq} (${seq % 2})` : "—";
  }
}

function triggerLifebitPulseInPlace(seq: number, timestampUnix: number): void {
  state.snapshot.heartbeatSeq = seq;
  state.snapshot.lastHeartbeatUnix = timestampUnix;
  if (state.snapshot.state !== "online") {
    return;
  }

  syncLifebitLedsInPlace(seq, timestampUnix);

  const ring = document.querySelector<HTMLSpanElement>("#status-pulse-ring");
  if (ring) {
    ring.classList.remove("dsa-lifebit-ring-active");
    void ring.offsetWidth;
    ring.classList.add("dsa-lifebit-ring-active");
  }

  const dot = document.querySelector<HTMLSpanElement>("#status-dot");
  if (dot) {
    dot.classList.remove("dsa-lifebit-dot-active");
    void dot.offsetWidth;
    dot.classList.add("dsa-lifebit-dot-active");
  }
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

/** Minutes left in the temporary Extended mode (rounded up), or `null` when unknown. */
function getExtendedRemainingMinutes(): number | null {
  const expires = state.snapshot.extendedExpiresUnix;
  if (expires == null) {
    return null;
  }
  const secondsLeft = Math.max(0, expires - Math.floor(Date.now() / 1000));
  return Math.ceil(secondsLeft / 60);
}

/** Extended badge label including the remaining time, e.g. "EXTENDED DEBUG · 23 min". */
function formatExtendedBadgeText(): string {
  const minutes = getExtendedRemainingMinutes();
  if (minutes === null) {
    return t("tierBadgeExtended");
  }
  return `${t("tierBadgeExtended")} · ${t("tierBadgeRemaining", { min: minutes })}`;
}

/** Updates the Extended badge countdown without re-rendering the dashboard. */
function syncExtendedBadgeInPlace(): void {
  if (getEffectiveTier() !== "extended") {
    return;
  }
  const badge = document.querySelector<HTMLButtonElement>("#btn-open-access-badge");
  if (badge) {
    badge.textContent = formatExtendedBadgeText();
  }
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

  const betaBadgeHtml = state.info.isBeta
    ? `<span class="rounded px-1.5 py-0.5 text-[10px] font-bold tracking-wider uppercase bg-amber-500/15 text-amber-300 border border-amber-500/40">${escapeHtml(t("betaBadge"))}</span>`
    : "";

  const tierBadgeHtml =
    tier === "developer"
      ? `<button id="btn-open-access-badge" type="button" class="rounded px-1.5 py-0.5 text-[10px] font-bold tracking-wider uppercase bg-purple-500/20 text-purple-300 border border-purple-500/40 hover:bg-purple-500/35 hover:border-purple-400 transition cursor-pointer">${escapeHtml(t("tierBadgeDeveloper"))}</button>`
      : tier === "extended"
        ? `<button id="btn-open-access-badge" type="button" class="rounded px-1.5 py-0.5 text-[10px] font-bold tracking-wider uppercase bg-cyan-500/20 text-cyan-300 border border-cyan-500/40 hover:bg-cyan-500/35 hover:border-cyan-400 transition cursor-pointer">${escapeHtml(formatExtendedBadgeText())}</button>`
        : "";

  const serverRecommendedVersion = getServerRecommendedUpdate();
  const activeOptionalVersion = state.availableUpdateVersion ?? serverRecommendedVersion;
  const showOptionalBanner =
    Boolean(activeOptionalVersion) &&
    (state.updateInstallingState !== "idle" ||
      state.dismissedOptionalVersion !== activeOptionalVersion);

  const updateBtnLabel =
    state.updateInstallingState === "downloading"
      ? t("updateDownloading", { percent: state.updateDownloadPercent })
      : state.updateInstallingState === "installing"
        ? t("updateInstalling")
        : state.availableUpdateVersion
          ? t("updateInstallBtn", { version: state.availableUpdateVersion })
          : t("outdatedAction");

  const updateProgressBarHtml =
    state.updateInstallingState !== "idle"
      ? `
        <div class="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-zinc-900/90">
          <div id="update-progress-bar" class="h-full bg-[#ff9600] transition-all duration-150" style="width: ${state.updateDownloadPercent}%"></div>
        </div>
      `
      : "";

  const outdatedBannerHtml =
    state.snapshot.state === "outdated"
      ? `
      <div class="rounded-lg border border-red-500/50 bg-red-950/40 px-3.5 py-2.5 text-xs">
        <div class="flex items-center justify-between gap-3">
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
          <div class="flex items-center gap-2 shrink-0">
            ${
              state.availableUpdateVersion
                ? `
              <button
                id="btn-install-update"
                type="button"
                ${state.updateInstallingState !== "idle" ? "disabled" : ""}
                class="rounded-md bg-[#ff9600] px-3 py-1.5 text-xs font-semibold text-zinc-950 hover:bg-[#ffa726] disabled:opacity-70 transition cursor-pointer"
              >
                ⬇ <span id="update-btn-label">${escapeHtml(updateBtnLabel)}</span>
              </button>
              <button
                id="btn-open-releases"
                type="button"
                class="rounded-md border border-zinc-700 bg-zinc-900/70 px-2.5 py-1.5 text-xs font-medium text-zinc-200 hover:border-[#ff9600] transition cursor-pointer"
              >
                GitHub ↗
              </button>
            `
                : `
              <button
                id="btn-open-releases"
                type="button"
                class="rounded-md bg-[#ff9600] px-3 py-1.5 text-xs font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
              >
                ${escapeHtml(t("outdatedAction"))}
              </button>
            `
            }
          </div>
        </div>
        ${updateProgressBarHtml}
      </div>
    `
      : showOptionalBanner && activeOptionalVersion
        ? `
      <div class="rounded-lg border border-[#ff9600]/45 bg-[#ff9600]/10 px-3.5 py-2.5 text-xs">
        <div class="flex items-center justify-between gap-3">
          <div class="min-w-0">
            <p class="font-semibold text-[#ff9600]">
              ⬆ ${escapeHtml(
                state.availableUpdateVersion
                  ? t("updateAvailableTitle", { version: activeOptionalVersion })
                  : t("optionalUpdateTitle", { version: activeOptionalVersion }),
              )}
            </p>
            <p class="text-zinc-300 mt-0.5">
              ${escapeHtml(
                state.availableUpdateVersion
                  ? t("updateAvailableDesc", {
                      version: activeOptionalVersion,
                      currentVersion: state.info.appVersion,
                    })
                  : t("optionalUpdateDesc", {
                      version: activeOptionalVersion,
                      currentVersion: state.snapshot.exeVersion,
                    }),
              )}
            </p>
          </div>
          <div class="flex items-center gap-2 shrink-0">
            ${
              state.availableUpdateVersion
                ? `
            <button
              id="btn-install-update"
              type="button"
              ${state.updateInstallingState !== "idle" ? "disabled" : ""}
              class="rounded-md bg-[#ff9600] px-3 py-1.5 text-xs font-semibold text-zinc-950 hover:bg-[#ffa726] disabled:opacity-70 transition cursor-pointer"
            >
              ⬇ <span id="update-btn-label">${escapeHtml(updateBtnLabel)}</span>
            </button>
            `
                : `
            <button
              id="btn-open-releases"
              type="button"
              class="rounded-md bg-[#ff9600] px-3 py-1.5 text-xs font-semibold text-zinc-950 hover:bg-[#ffa726] transition cursor-pointer"
            >
              ${escapeHtml(t("outdatedAction"))} ↗
            </button>
            `
            }
            ${
              state.updateInstallingState === "idle"
                ? `
            <button
              id="btn-dismiss-optional-update"
              type="button"
              title="${escapeHtml(t("optionalUpdateDismissTitle"))}"
              class="rounded px-1.5 py-1 text-zinc-400 hover:text-zinc-100 transition cursor-pointer"
            >
              ✕
            </button>
            `
                : ""
            }
          </div>
        </div>
        ${updateProgressBarHtml}
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
  const hbSeq = state.snapshot.heartbeatSeq ?? 0;
  const lifebitDebugFormatted = hbSeq > 0 ? `#${hbSeq} (${hbSeq % 2})` : "—";

  const extendedDebugStripHtml = isExtendedOrDev
    ? `
      <div class="flex flex-wrap items-center justify-between gap-2 pt-1.5 border-t border-zinc-800/80 text-[11px] text-zinc-400 font-mono">
        <div class="truncate max-w-[52%]" title="${escapeHtml(state.snapshot.bridgePath ?? "—")}">
          <span class="font-sans text-zinc-500">${escapeHtml(t("debugBridgeLabel"))}</span>
          <span class="text-zinc-300">${escapeHtml(state.snapshot.bridgePath ?? "—")}</span>
        </div>
        <div class="flex items-center gap-3 shrink-0">
          <span>
            <span class="font-sans text-zinc-500">${escapeHtml(t("debugLifebitLabel"))}</span>
            <span id="debug-lifebit-val" class="text-emerald-300">${lifebitDebugFormatted}</span>
          </span>
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
            <span class="text-[10px] font-mono uppercase tracking-wider text-zinc-400">v${escapeHtml(state.snapshot.exeVersion)}${state.info.isBeta ? " BETA" : ""}</span>
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
              id="btn-mock-contracts"
              type="button"
              class="rounded bg-cyan-500/15 border border-cyan-500/40 px-2.5 py-1 text-xs font-medium text-cyan-300 hover:bg-cyan-500 hover:text-zinc-950 transition cursor-pointer"
            >
              📋 ${escapeHtml(t("mockBtnContractsSim"))}
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
              id="btn-mock-optver"
              type="button"
              class="rounded border px-2.5 py-1 text-xs font-medium transition cursor-pointer ${
                state.mockOptionalUpdate
                  ? "bg-[#ff9600] text-zinc-950 border-amber-300 font-semibold"
                  : "bg-zinc-800 border-zinc-700 text-zinc-300 hover:border-zinc-500"
              }"
            >
              🔔 ${escapeHtml(state.mockOptionalUpdate ? t("mockBtnOptVerOff") : t("mockBtnOptVerOn"))}
            </button>
            <select
              id="sel-mock-tier"
              aria-label="Mock Branding Tier"
              class="rounded border border-purple-500/40 bg-zinc-900 px-2 py-1 text-xs font-medium text-purple-200 hover:border-purple-400 focus:border-[#ff9600] focus:outline-none cursor-pointer"
            >
              <option value="0" ${state.snapshot.brandingTier === 0 ? "selected" : ""} class="bg-[#252528] text-zinc-100">${escapeHtml(t("mockTier0"))}</option>
              <option value="1" ${state.snapshot.brandingTier === 1 ? "selected" : ""} class="bg-[#252528] text-zinc-100">${escapeHtml(t("mockTier1"))}</option>
              <option value="2" ${state.snapshot.brandingTier === 2 ? "selected" : ""} class="bg-[#252528] text-zinc-100">${escapeHtml(t("mockTier2"))}</option>
              <option value="3" ${state.snapshot.brandingTier === 3 ? "selected" : ""} class="bg-[#252528] text-zinc-100">${escapeHtml(t("mockTier3"))}</option>
            </select>
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

  const activeTokenId = getActiveTokenId();
  const savedTokens = getSavedTokens();

  const marketScopeBadgeHtml =
    state.snapshot.brandingTier === 1
      ? `<span class="rounded border border-sky-500/35 bg-sky-500/10 px-1.5 py-0.2 text-[10px] font-semibold text-sky-300 shrink-0">🖼 ${escapeHtml(t("headerBadgeTier1Logo"))}</span>`
      : state.snapshot.isPrivateMarket
        ? `<span class="rounded border border-[#ff9600]/35 bg-[#ff9600]/10 px-1.5 py-0.2 text-[10px] font-semibold text-[#ff9600] shrink-0">🔒 ${escapeHtml(t("headerBadgePrivateMarket"))}</span>`
        : "";

  const quickMarketOptionsHtml = [
    `<option value="" ${!activeTokenId ? "selected" : ""} class="bg-[#252528] text-zinc-100">🌐 ${escapeHtml(t("switcherOfficialStandard"))}</option>`,
    ...savedTokens.map(
      (prof) =>
        `<option value="${escapeHtml(prof.id)}" ${prof.id === activeTokenId ? "selected" : ""} class="bg-[#252528] text-zinc-100">${escapeHtml(formatSwitcherOptionLabel(prof))}</option>`,
    ),
  ].join("");

  const hasQuickMarketSwitcher = !isMock && savedTokens.length > 0;
  const rightControlWidthClass = hasQuickMarketSwitcher ? "w-48" : "w-40";
  const rightControlPyClass = hasQuickMarketSwitcher ? "py-1" : "py-1.5";

  const quickMarketSwitcherHtml =
    hasQuickMarketSwitcher
      ? `
        <select
          id="select-quick-market"
          aria-label="${escapeHtml(t("switcherAriaLabel"))}"
          title="${escapeHtml(t("switcherAriaLabel"))}"
          class="w-48 truncate rounded-md border ${
            activeTokenId
              ? state.snapshot.brandingTier === 1
                ? "border-sky-500/50 bg-sky-500/10 text-sky-100"
                : "border-[#ff9600]/55 bg-[#ff9600]/10 text-amber-100"
              : "border-zinc-700 bg-[#252528] text-zinc-200"
          } px-2.5 ${rightControlPyClass} text-xs font-semibold hover:border-[#ff9600] focus:border-[#ff9600] focus:outline-none cursor-pointer"
        >
          ${quickMarketOptionsHtml}
        </select>
      `
      : "";

  return `
    <!-- 3-Column Hero Header: Left Brand/Version | Center Logo + Hidden 5x Trigger + Status | Right Settings + Language + Market Switcher -->
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
          ${betaBadgeHtml}
          ${tierBadgeHtml}
        </div>
        <p class="text-xs text-zinc-300 truncate">${escapeHtml(t("appSubtitle"))}</p>
        <p class="text-[11px] text-zinc-500 truncate">
          D-S-Agrarservice &bull; <span data-secret-trigger="true" class="font-mono select-none">v${escapeHtml(state.info.protocolVersion)}${state.info.isBeta ? " Beta" : ""}</span>
        </p>
        <p class="text-[11px] text-zinc-500 truncate">
          powered by <button id="btn-open-bydora" type="button" title="${escapeHtml(BYDORA_URL)}" class="text-zinc-400 hover:text-sky-300 hover:underline transition cursor-pointer">bydora.de</button>
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
        <div class="mt-0.5 flex items-center justify-center gap-1.5 max-w-[280px]">
          <button
            id="btn-secret-version"
            data-secret-trigger="true"
            type="button"
            class="text-xs font-semibold text-zinc-200 focus:outline-none cursor-default select-none truncate"
          >${escapeHtml(displayMarketName)}</button>
          ${marketScopeBadgeHtml}
        </div>
        <div
          id="status-pill"
          title="${
            state.snapshot.state === "online"
              ? escapeHtml(
                  formatLifebitTooltip(
                    hbSeq,
                    state.snapshot.lastHeartbeatUnix ?? null,
                  ),
                )
              : state.snapshot.state === "disconnected"
                ? escapeHtml(t("stateDisconnected"))
                : ""
          }"
          class="inline-flex items-center gap-2 rounded-full border px-3.5 py-1 text-xs font-semibold ${conn.pillClass}"
        >
          <span class="relative inline-flex h-2.5 w-2.5 items-center justify-center">
            <span id="status-pulse-ring" class="pointer-events-none absolute inset-0 rounded-full ${
              state.snapshot.state === "online" ? "bg-emerald-400/75 opacity-0" : "opacity-0"
            }"></span>
            <span id="status-dot" class="relative inline-block h-2.5 w-2.5 rounded-full ${conn.dotClass}"></span>
          </span>
          <span>${escapeHtml(conn.label)}</span>
          ${
            state.snapshot.state === "online"
              ? `<span class="ml-0.5 inline-flex items-center gap-1 rounded-full border border-emerald-500/30 bg-emerald-950/75 px-1.5 py-0.5" aria-hidden="true">
                  <span id="lifebit-led-a" class="${
                    hbSeq > 0 && hbSeq % 2 === 1
                      ? "inline-block h-1.5 w-1.5 rounded-full bg-emerald-300 shadow-[0_0_6px_rgba(110,231,183,0.95)] transition-all duration-300"
                      : "inline-block h-1.5 w-1.5 rounded-full bg-emerald-500/25 transition-all duration-300"
                  }"></span>
                  <span id="lifebit-led-b" class="${
                    hbSeq > 0 && hbSeq % 2 === 0
                      ? "inline-block h-1.5 w-1.5 rounded-full bg-emerald-300 shadow-[0_0_6px_rgba(110,231,183,0.95)] transition-all duration-300"
                      : "inline-block h-1.5 w-1.5 rounded-full bg-emerald-500/25 transition-all duration-300"
                  }"></span>
                </span>`
              : ""
          }
        </div>
      </div>

      <!-- Right Column: Connection Switch + Settings Button + Quick Language Selector + Quick Market Switcher -->
      <div class="flex flex-col items-end gap-1.5">
        <button
          id="btn-toggle-connection"
          type="button"
          ${state.togglingConnection ? "disabled" : ""}
          title="${escapeHtml(
            state.snapshot.connectionEnabled
              ? t("btnDisconnectTooltip")
              : t("btnConnectTooltip"),
          )}"
          class="${rightControlWidthClass} flex items-center justify-center gap-1.5 rounded-md border ${
            state.snapshot.connectionEnabled
              ? "border-red-500/35 bg-red-500/10 text-red-200 hover:border-red-400 hover:text-red-300"
              : "border-emerald-500/45 bg-emerald-500/15 text-emerald-200 hover:border-emerald-400 hover:bg-emerald-500/20"
          } px-3 ${rightControlPyClass} text-xs font-semibold transition ${
            state.togglingConnection ? "opacity-60 cursor-wait" : "cursor-pointer"
          }"
        >
          <svg class="h-3.5 w-3.5 shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M18.36 6.64a9 9 0 1 1-12.73 0"></path>
            <line x1="12" y1="2" x2="12" y2="12"></line>
          </svg>
          <span class="truncate">${escapeHtml(
            state.snapshot.connectionEnabled
              ? t("btnDisconnect")
              : t("btnConnect"),
          )}</span>
        </button>

        <button
          id="btn-open-settings"
          type="button"
          title="${escapeHtml(t("btnSettings"))}"
          class="${rightControlWidthClass} flex items-center justify-center gap-1.5 rounded-md border border-zinc-700 bg-[#252528] px-3 ${rightControlPyClass} text-xs font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer"
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
          class="${rightControlWidthClass} rounded-md border border-zinc-700 bg-[#252528] px-2.5 ${rightControlPyClass} text-xs font-semibold text-zinc-200 hover:border-[#ff9600]/60 focus:border-[#ff9600] focus:outline-none cursor-pointer"
        >
          ${langOptionsHtml}
        </select>

        ${quickMarketSwitcherHtml}
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
          ${
            state.snapshot.contractsSupported
              ? `
          <span class="rounded bg-sky-500/15 border border-sky-500/30 px-2 py-0.5 text-[11px] font-medium text-sky-300">
            📋 ${escapeHtml(t("contractsBadge", { count: state.snapshot.openContractsCount }))}
          </span>
          <span class="rounded bg-cyan-500/15 border border-cyan-500/30 px-2 py-0.5 text-[11px] font-medium text-cyan-300">
            🏷 ${escapeHtml(t("offersBadge", { count: state.snapshot.openOffersCount }))}
          </span>
          `
              : ""
          }
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

      ${(() => {
        const displayErr = formatSnapshotError(state.snapshot.lastError);
        return displayErr
          ? `<div class="text-red-400 text-[11px] truncate" title="${escapeHtml(displayErr)}">⚠ ${escapeHtml(displayErr)}</div>`
          : "";
      })()}

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

function renderSavedProfilesListHtml(): string {
  const activeTokenId = getActiveTokenId();
  const savedTokens = getSavedTokens();
  const isOfficialActive = !activeTokenId;

  const officialCardHtml = `
    <div class="flex items-center justify-between gap-3 rounded-lg border ${
      isOfficialActive
        ? "border-emerald-500/45 bg-emerald-500/10"
        : "border-zinc-800 bg-[#18181b]/90"
    } px-3 py-2">
      <div class="min-w-0">
        <div class="flex items-center gap-1.5 flex-wrap">
          <span class="font-semibold text-zinc-100">🌐 ${escapeHtml(t("settingsOfficialMarketOption"))}</span>
        </div>
        <p class="text-[11px] text-zinc-400 truncate">${escapeHtml(t("settingsOfficialMarketOptionDesc"))}</p>
      </div>
      <div class="flex items-center gap-1.5 shrink-0">
        ${
          isOfficialActive
            ? `<span class="rounded border border-emerald-500/40 bg-emerald-500/15 px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-emerald-300">${escapeHtml(t("settingsProfileActiveBadge"))}</span>`
            : `<button type="button" data-select-token="" class="rounded border border-zinc-700 bg-[#252528] px-2.5 py-1 text-[11px] font-semibold text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer">${escapeHtml(t("settingsProfileActivateBtn"))}</button>`
        }
      </div>
    </div>
  `;

  const savedCardsHtml = savedTokens
    .map((prof) => {
      const isActive = prof.id === activeTokenId;
      const icon =
        prof.brandingTier === 1
          ? "🖼"
          : prof.brandingTier === 2 || prof.brandingTier === 3 || prof.isPrivateMarket
            ? "🔒"
            : "🔑";
      const badge = formatProfileTierBadge(prof);
      return `
        <div class="flex items-center justify-between gap-3 rounded-lg border ${
          isActive
            ? prof.brandingTier === 1
              ? "border-sky-500/45 bg-sky-500/10"
              : "border-[#ff9600]/50 bg-[#ff9600]/10"
            : "border-zinc-800 bg-[#18181b]/90"
        } px-3 py-2">
          <div class="min-w-0 space-y-0.5">
            <div class="flex items-center gap-1.5 flex-wrap">
              <span class="font-semibold text-zinc-100 truncate">${icon} ${escapeHtml(formatProfileCardTitle(prof))}</span>
              <span class="rounded border border-zinc-700/80 bg-zinc-900/90 px-1.5 py-0.2 font-mono text-[10px] text-zinc-400">${escapeHtml(prof.maskedHint)}</span>
            </div>
            <div class="flex items-center gap-1.5 flex-wrap">
              <span class="rounded border px-1.5 py-0.2 text-[10px] font-medium ${badge.classes}">${escapeHtml(badge.text)}</span>
            </div>
          </div>
          <div class="flex items-center gap-1.5 shrink-0">
            ${
              isActive
                ? `<span class="rounded border border-emerald-500/40 bg-emerald-500/15 px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-emerald-300">${escapeHtml(t("settingsProfileActiveBadge"))}</span>`
                : `<button type="button" data-select-token="${escapeHtml(prof.id)}" class="rounded border border-zinc-700 bg-[#252528] px-2.5 py-1 text-[11px] font-semibold text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] transition cursor-pointer">${escapeHtml(t("settingsProfileActivateBtn"))}</button>`
            }
            <button
              type="button"
              data-remove-token="${escapeHtml(prof.id)}"
              class="rounded border border-red-500/30 bg-red-500/10 px-2 py-1 text-[11px] font-medium text-red-300 hover:bg-red-500/20 hover:border-red-400 transition cursor-pointer"
            >
              ${escapeHtml(t("settingsProfileRemoveBtn"))}
            </button>
          </div>
        </div>
      `;
    })
    .join("");

  return `${officialCardHtml}${savedCardsHtml}`;
}

function bindSavedProfilesListEvents(): void {
  const container = document.querySelector<HTMLDivElement>(
    "#settings-saved-profiles-list",
  );
  if (!container) return;

  container
    .querySelectorAll<HTMLButtonElement>("[data-select-token]")
    .forEach((btn) => {
      btn.addEventListener("click", async () => {
        const rawId = btn.getAttribute("data-select-token") ?? "";
        const tokenId = rawId.trim().length > 0 ? rawId.trim() : null;
        try {
          const snap = await invoke<BridgeSnapshot>(
            "select_active_market_token",
            { tokenId },
          );
          applySnapshot(snap);
          refreshSavedProfilesListInModal();
          renderDashboardOnly();
          showToast(t("toastMarketSwitched"));
        } catch (err) {
          showToast(String(err));
        }
      });
    });

  container
    .querySelectorAll<HTMLButtonElement>("[data-remove-token]")
    .forEach((btn) => {
      btn.addEventListener("click", async () => {
        const tokenId = btn.getAttribute("data-remove-token")?.trim();
        if (!tokenId) return;
        try {
          const snap = await invoke<BridgeSnapshot>("remove_market_token", {
            tokenId,
          });
          applySnapshot(snap);
          refreshSavedProfilesListInModal();
          renderDashboardOnly();
          showToast(t("toastTokenRemoved"));
        } catch (err) {
          showToast(String(err));
        }
      });
    });
}

function refreshSavedProfilesListInModal(): void {
  const container = document.querySelector<HTMLDivElement>(
    "#settings-saved-profiles-list",
  );
  if (!container) return;
  container.innerHTML = renderSavedProfilesListHtml();
  bindSavedProfilesListEvents();
}

function renderSettingsModalHtml(): string {
  const currentLang = normalizeLangForForm(state.config.language);
  const currentTheme = normalizeTheme(state.config.theme);
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
          <!-- Language & Appearance -->
          <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
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

            <div>
              <label for="cfg-theme" class="block font-semibold text-zinc-200 mb-1">
                ${escapeHtml(t("settingsTheme"))}
              </label>
              <select
                id="cfg-theme"
                class="w-full rounded-md border border-zinc-700 bg-[#18181b] px-3 py-2 text-zinc-100 focus:border-[#ff9600] focus:outline-none"
              >
                <option value="dark" ${currentTheme === "dark" ? "selected" : ""} class="bg-[#252528] text-zinc-100">
                  🌙 ${escapeHtml(t("settingsThemeDark"))}
                </option>
                <option value="light" ${currentTheme === "light" ? "selected" : ""} class="bg-[#252528] text-zinc-100">
                  ☀️ ${escapeHtml(t("settingsThemeLight"))}
                </option>
                <option value="system" ${currentTheme === "system" ? "selected" : ""} class="bg-[#252528] text-zinc-100">
                  🖥 ${escapeHtml(t("settingsThemeSystem"))}
                </option>
              </select>
            </div>
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

            <div>
              <label for="cfg-startup-connect" class="block font-semibold text-zinc-200 mb-1">
                ${escapeHtml(t("settingsStartupConnectLabel"))}
              </label>
              <select
                id="cfg-startup-connect"
                class="w-full rounded-md border border-zinc-700 bg-[#18181b] px-3 py-2 text-zinc-100 focus:border-[#ff9600] focus:outline-none"
              >
                <option value="auto" ${state.config.autoConnectOnLaunch !== false ? "selected" : ""} class="bg-[#252528] text-zinc-100">
                  ${escapeHtml(t("settingsStartupConnectAuto"))}
                </option>
                <option value="disconnected" ${state.config.autoConnectOnLaunch === false ? "selected" : ""} class="bg-[#252528] text-zinc-100">
                  ${escapeHtml(t("settingsStartupConnectManual"))}
                </option>
              </select>
              <p class="mt-1 text-[11px] text-zinc-400">
                ${escapeHtml(t("settingsStartupConnectHint"))}
              </p>
            </div>

            ${devServerFieldsHtml}

            <div class="space-y-2">
              <label class="block font-semibold text-zinc-200">
                ${escapeHtml(t("settingsSavedMarketsLabel"))}
              </label>
              <div id="settings-saved-profiles-list" class="space-y-1.5">
                ${renderSavedProfilesListHtml()}
              </div>
            </div>

            <div class="rounded-lg border border-zinc-800 bg-[#18181b]/80 p-3 space-y-2">
              <p class="text-[11px] font-semibold text-zinc-200">
                🔑 ${escapeHtml(t("settingsAddTokenSection"))}
              </p>
              <input
                id="cfg-token-label"
                type="text"
                maxlength="36"
                autocomplete="off"
                placeholder="${escapeHtml(t("settingsTokenLabelPlaceholder"))}"
                class="w-full rounded-md border border-zinc-700 bg-[#202023] px-2.5 py-1.5 text-xs text-zinc-100 focus:border-[#ff9600] focus:outline-none"
              />
              <div class="flex items-center gap-2">
                <input
                  id="cfg-new-token"
                  type="password"
                  value=""
                  autocomplete="new-password"
                  spellcheck="false"
                  placeholder="${escapeHtml(t("settingsNewTokenPlaceholder"))}"
                  class="flex-1 min-w-0 rounded-md border border-zinc-700 bg-[#202023] px-2.5 py-1.5 font-mono text-xs text-zinc-100 focus:border-[#ff9600] focus:outline-none"
                />
                <button
                  id="btn-add-token"
                  type="button"
                  class="shrink-0 rounded-md border border-[#ff9600]/60 bg-[#ff9600]/15 px-3 py-1.5 text-xs font-semibold text-[#ff9600] hover:bg-[#ff9600] hover:text-zinc-950 transition cursor-pointer"
                >
                  + ${escapeHtml(t("settingsAddTokenBtn"))}
                </button>
              </div>
              <p class="text-[11px] text-zinc-500 leading-snug">
                ${escapeHtml(t("settingsMarketTokenHint"))}
              </p>
              <p class="text-[11px] text-emerald-400/80 leading-snug">
                🛡 ${escapeHtml(t("settingsVaultSecurityHint"))}
              </p>
            </div>
          </div>

          <div class="flex flex-wrap items-center justify-between gap-2 pt-3 border-t border-zinc-800">
            <button
              id="btn-check-updates"
              type="button"
              ${state.checkingForUpdate || state.updateInstallingState !== "idle" ? "disabled" : ""}
              class="rounded-md border border-zinc-700 bg-[#18181b] px-3 py-1.5 font-medium text-zinc-200 hover:border-[#ff9600] hover:text-[#ff9600] disabled:opacity-60 transition cursor-pointer"
            >
              🔄 ${escapeHtml(
                state.checkingForUpdate
                  ? t("settingsCheckingUpdates")
                  : t("settingsCheckUpdatesBtn"),
              )}
            </button>

            <div class="flex items-center gap-2 ml-auto">
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

          <!-- Historic Roots & Pioneers: Susi & Wingi (FS15) + kevink98 (FS17/FS19) -->
          <div class="space-y-1.5">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-[#ff9600]">
              ★ ${escapeHtml(t("creditsSpecialSection"))}
            </h3>
            <div class="space-y-2">
              <div class="rounded-lg border border-amber-500/25 bg-[#18181b] p-3">
                <p class="font-bold text-xs text-amber-200">${escapeHtml(t("creditsPioneersFs15Title"))}</p>
                <p class="text-zinc-300 mt-0.5 leading-relaxed">
                  ${escapeHtml(t("creditsPioneersFs15"))}
                </p>
              </div>
              <div class="rounded-lg border border-[#ff9600]/40 bg-[#ff9600]/10 p-3">
                <p class="font-bold text-xs text-amber-200">${escapeHtml(t("creditsSpecialKevinkTitle"))}</p>
                <p class="text-zinc-200 mt-0.5 leading-relaxed">
                  ${escapeHtml(t("creditsSpecialKevink"))}
                </p>
              </div>
            </div>
          </div>

          <!-- Ground-Up Redevelopment & AI Transparency -->
          <div class="space-y-1.5">
            <h3 class="text-[11px] font-semibold uppercase tracking-wider text-sky-300">
              ⚡ ${escapeHtml(t("creditsAiSection"))}
            </h3>
            <div class="rounded-lg border border-sky-500/30 bg-sky-500/10 p-3">
              <div class="flex items-center justify-between gap-2 flex-wrap">
                <p class="font-bold text-xs text-sky-200">${escapeHtml(t("creditsAiTitle"))}</p>
                <span class="rounded border border-sky-400/30 bg-zinc-900/70 px-1.5 py-0.2 font-mono text-[10px] text-sky-300">Rust + Tauri v2 + TypeScript</span>
              </div>
              <p class="text-zinc-200 mt-1 leading-relaxed">
                ${escapeHtml(t("creditsAiBody"))}
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
              ${escapeHtml(getBrandTitle())} v${escapeHtml(state.info.appVersion)}${state.info.isBeta ? " Beta" : ""} (${escapeHtml(state.info.platform)})
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
  return normalizeLocale(lang);
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
    .querySelector<HTMLSelectElement>("#select-quick-market")
    ?.addEventListener("change", async (ev) => {
      const target = ev.currentTarget as HTMLSelectElement;
      const tokenId = target.value.trim().length > 0 ? target.value.trim() : null;
      try {
        const snap = await invoke<BridgeSnapshot>(
          "select_active_market_token",
          { tokenId },
        );
        applySnapshot(snap);
        renderDashboardOnly();
        showToast(t("toastMarketSwitched"));
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-toggle-connection")
    ?.addEventListener("click", async () => {
      if (state.togglingConnection) return;
      const nextConnected = !state.snapshot.connectionEnabled;
      state.togglingConnection = true;
      renderDashboardOnly();
      try {
        const snap = await invoke<BridgeSnapshot>("set_bridge_connected", {
          connected: nextConnected,
        });
        applySnapshot(snap);
        showToast(t(nextConnected ? "toastConnected" : "toastDisconnected"));
      } catch (err) {
        showToast(String(err));
      } finally {
        state.togglingConnection = false;
        renderDashboardOnly();
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
    .querySelector<HTMLButtonElement>("#btn-install-update")
    ?.addEventListener("click", () => void installPendingAppUpdate());

  document
    .querySelector<HTMLButtonElement>("#btn-open-releases")
    ?.addEventListener("click", () => void openExternalUrl(GITHUB_RELEASES_URL));

  document
    .querySelector<HTMLButtonElement>("#btn-dismiss-optional-update")
    ?.addEventListener("click", () => {
      const target =
        state.availableUpdateVersion ?? getServerRecommendedUpdate();
      if (target) {
        state.dismissedOptionalVersion = target;
        renderDashboardOnly();
      }
    });

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
    .querySelector<HTMLButtonElement>("#btn-mock-contracts")
    ?.addEventListener("click", async () => {
      try {
        await invoke("mock_simulate_contract_activity");
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
      const targetVer = state.mockLockedMinVersion
        ? "9.9.9"
        : state.info.protocolVersion || "0.9.14";
      renderDashboardOnly();
      try {
        await invoke("mock_set_min_version", { minVersion: targetVer });
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLButtonElement>("#btn-mock-optver")
    ?.addEventListener("click", async () => {
      state.mockOptionalUpdate = !state.mockOptionalUpdate;
      if (state.mockOptionalUpdate) {
        state.dismissedOptionalVersion = null;
      }
      const targetOpt = state.mockOptionalUpdate ? "0.9.15" : null;
      renderDashboardOnly();
      try {
        await invoke("mock_set_latest_version", { latestVersion: targetOpt });
      } catch (err) {
        showToast(String(err));
      }
    });

  document
    .querySelector<HTMLSelectElement>("#sel-mock-tier")
    ?.addEventListener("change", async (ev) => {
      const target = ev.currentTarget as HTMLSelectElement;
      const tierNum = Math.max(0, Math.min(3, Number.parseInt(target.value, 10) || 0));
      try {
        const snap = await invoke<BridgeSnapshot>("mock_set_branding_tier", {
          tier: tierNum,
          marketName: null,
        });
        applySnapshot(snap);
        renderDashboardOnly();
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
    .querySelector<HTMLButtonElement>("#btn-open-bydora")
    ?.addEventListener("click", () => void openExternalUrl(BYDORA_URL));

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
    revertPreviewedThemeIfNeeded();
    state.activeModal = "none";
    state.accessKeyError = null;
    renderModalOnly();
  };

  document
    .querySelector<HTMLSelectElement>("#cfg-theme")
    ?.addEventListener("change", (ev) => {
      const target = ev.currentTarget as HTMLSelectElement;
      const previewPref = applyThemeToDom(target.value);
      previewingUnsavedTheme = previewPref !== normalizeTheme(state.config.theme);
      void invoke("preview_window_theme", { theme: previewPref }).catch(() => {});
    });

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
    .querySelector<HTMLButtonElement>("#btn-check-updates")
    ?.addEventListener("click", () => void checkForAppUpdates(true));

  bindSavedProfilesListEvents();

  const submitPendingTokenInputs = async (): Promise<boolean> => {
    const tokenInput = document.querySelector<HTMLInputElement>("#cfg-new-token");
    const labelInput =
      document.querySelector<HTMLInputElement>("#cfg-token-label");
    const rawToken = tokenInput?.value.trim() ?? "";
    if (rawToken.length === 0) {
      return false;
    }
    const rawLabel = labelInput?.value.trim() ?? "";
    const snap = await invoke<BridgeSnapshot>("add_market_token", {
      token: rawToken,
      customLabel: rawLabel.length > 0 ? rawLabel : null,
    });
    if (tokenInput) {
      tokenInput.value = "";
    }
    if (labelInput) {
      labelInput.value = "";
    }
    applySnapshot(snap);
    refreshSavedProfilesListInModal();
    renderDashboardOnly();
    return true;
  };

  const handleAddTokenClick = async (): Promise<void> => {
    const tokenInput = document.querySelector<HTMLInputElement>("#cfg-new-token");
    if (!tokenInput || tokenInput.value.trim().length === 0) {
      tokenInput?.focus();
      return;
    }
    try {
      await submitPendingTokenInputs();
      showToast(t("toastTokenAdded"));
    } catch (err) {
      showToast(String(err));
    }
  };

  document
    .querySelector<HTMLButtonElement>("#btn-add-token")
    ?.addEventListener("click", () => void handleAddTokenClick());

  document
    .querySelector<HTMLInputElement>("#cfg-new-token")
    ?.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        ev.preventDefault();
        void handleAddTokenClick();
      }
    });

  document
    .querySelector<HTMLInputElement>("#cfg-token-label")
    ?.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") {
        const tokenInput =
          document.querySelector<HTMLInputElement>("#cfg-new-token");
        if (tokenInput && tokenInput.value.trim().length > 0) {
          ev.preventDefault();
          void handleAddTokenClick();
        }
      }
    });

  document
    .querySelector<HTMLFormElement>("#form-settings")
    ?.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const isDev = getEffectiveTier() === "developer";
      const langVal =
        document.querySelector<HTMLSelectElement>("#cfg-lang")?.value ?? "de";
      const themeVal = normalizeTheme(
        document.querySelector<HTMLSelectElement>("#cfg-theme")?.value ??
          state.config.theme,
      );
      const trayVal =
        document.querySelector<HTMLInputElement>("#cfg-tray")?.checked ?? true;
      const notifyVal =
        document.querySelector<HTMLInputElement>("#cfg-notify")?.checked ?? true;
      const autoScrollVal =
        document.querySelector<HTMLInputElement>("#cfg-autoscroll")?.checked ??
        true;
      const startupConnectVal =
        document.querySelector<HTMLSelectElement>("#cfg-startup-connect")
          ?.value ?? "auto";
      const modeVal = isDev
        ? ((document.querySelector<HTMLSelectElement>("#cfg-mode")
            ?.value as BackendMode) ?? state.config.backendMode)
        : "live";
      const urlVal = isDev
        ? document.querySelector<HTMLInputElement>("#cfg-url")?.value.trim() ||
          state.config.backendUrl
        : state.config.backendUrl;

      try {
        await submitPendingTokenInputs();

        const nextConfig: AppConfig = {
          ...state.config,
          language: setLocale(langVal),
          theme: themeVal,
          minimizeToTray: trayVal,
          notificationsEnabled: notifyVal,
          logAutoScroll: autoScrollVal,
          autoConnectOnLaunch: startupConnectVal !== "disconnected",
          backendMode: modeVal,
          backendUrl: urlVal,
        };

        const snap = await invoke<BridgeSnapshot>("update_app_config", {
          config: nextConfig,
        });
        previewingUnsavedTheme = false;
        state.config = {
          ...nextConfig,
          activeTokenId: snap.activeTokenId ?? nextConfig.activeTokenId,
        };
        applyThemeToDom(state.config.theme);
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
        state.config = {
          ...updatedCfg,
          theme: normalizeTheme(updatedCfg.theme),
        };
        applyThemeToDom(state.config.theme);
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
        state.config = {
          ...updatedCfg,
          theme: normalizeTheme(updatedCfg.theme),
        };
        applyThemeToDom(state.config.theme);
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
  systemDarkMedia?.addEventListener("change", () => {
    if (!previewingUnsavedTheme && normalizeTheme(state.config.theme) === "system") {
      applyThemeToDom("system");
    }
  });

  window.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      if (state.activeModal !== "none") {
        revertPreviewedThemeIfNeeded();
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
    state.config = {
      ...config,
      theme: normalizeTheme(config.theme),
      autoConnectOnLaunch: config.autoConnectOnLaunch !== false,
    };
    applyThemeToDom(state.config.theme);
    applySnapshot(snapshot);
    state.logs = logs;
    setLocale(config.language);
  } catch {
    applyThemeToDom("dark");
    setLocale("de");
    syncDocumentTitle();
  }

  renderAll();
  const scrollBox = document.querySelector<HTMLDivElement>("#log-scroll-box");
  if (scrollBox && state.config.logAutoScroll) {
    scrollBox.scrollTop = scrollBox.scrollHeight;
  }

  void ensureNotificationPermissionIfEnabled();
  void checkForAppUpdates(false);

  try {
    await listen<BridgeSnapshot>("bridge://status", (event) => {
      const wasOutdated = state.snapshot.state === "outdated";
      const prevOptionalVer = getServerRecommendedUpdate();
      const prevTier = getEffectiveTier();
      const prevSig = JSON.stringify({
        ...state.snapshot,
        heartbeatSeq: 0,
        lastHeartbeatUnix: null,
        // Ignore lastSyncUnix changes when hidden in Standard User mode so DOM stays untouched on background polls
        lastSyncUnix:
          prevTier === "user" ? null : state.snapshot.lastSyncUnix,
      });
      applySnapshot(event.payload);
      const nextOptionalVer = getServerRecommendedUpdate();
      if (prevTier !== event.payload.accessTier && state.activeModal === "settings") {
        renderModalOnly();
      }
      const nextTier = getEffectiveTier();
      const nextSig = JSON.stringify({
        ...state.snapshot,
        heartbeatSeq: 0,
        lastHeartbeatUnix: null,
        lastSyncUnix:
          nextTier === "user" ? null : state.snapshot.lastSyncUnix,
      });
      if (
        ((!wasOutdated && event.payload.state === "outdated") ||
          (nextOptionalVer !== null && nextOptionalVer !== prevOptionalVer)) &&
        !state.availableUpdateVersion
      ) {
        void checkForAppUpdates(false);
      }
      if (prevSig !== nextSig) {
        renderDashboardOnly();
        if (state.activeModal === "settings") {
          refreshSavedProfilesListInModal();
        }
      } else {
        syncLifebitLedsInPlace(
          state.snapshot.heartbeatSeq ?? 0,
          state.snapshot.lastHeartbeatUnix ?? null,
        );
      }
    });

    await listen<HeartbeatTickPayload>("bridge://heartbeat", (event) => {
      triggerLifebitPulseInPlace(event.payload.seq, event.payload.timestampUnix);
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
          } else {
            state.snapshot.extendedExpiresUnix = snap.extendedExpiresUnix;
            syncExtendedBadgeInPlace();
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
