/**
 * Release classifier and bilingual (DE / EN) Discord Embed formatter
 * for D-S-Agrarservice GlobalMarketClient (and future FS25_GlobalMarket mod) releases.
 */

export const EMBED_COLOR_BETA = 0xf59e0b; // Amber / Beta
export const EMBED_COLOR_RELEASE = 0x4fa83d; // DSA Green / Stable v1.0+
/** Safe per-message attachment budget under Discord's 10 MiB standard upload cap. */
export const MAX_DISCORD_ATTACHMENT_BYTES = Math.floor(9.5 * 1024 * 1024);
/** Public GitHub Pages download bridge (prevents Opera GX from closing external direct-download tabs). */
export const DEFAULT_DOWNLOAD_PORTAL_URL =
  "https://d-s-agrarservice.github.io/GlobalMarketClient/";

const IGNORED_ASSET_PATTERNS = [
  /^latest\.json$/i,
  /\.sig$/i,
  /\.app\.tar\.gz$/i,
  /\.nsis\.zip$/i,
  /\.blockmap$/i,
];

/**
 * Parses a Git tag such as `v0.9.16-beta`, `v1.0.0`, `v0.11.0.41`, or `v1.0.0.0`.
 * Supports both 3-part SemVer and 4-part Farming Simulator ModHub version strings.
 */
export function parseSemverTag(tagName) {
  const raw = String(tagName ?? "").trim();
  const match = raw.match(
    /^v?(\d+)\.(\d+)\.(\d+)(?:\.(\d+))?(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?$/,
  );
  if (!match) {
    return {
      raw,
      valid: false,
      major: 0,
      minor: 0,
      patch: 0,
      build: null,
      prereleaseTag: raw.includes("-") ? raw.split("-").slice(1).join("-") : null,
    };
  }
  return {
    raw,
    valid: true,
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    build: match[4] !== undefined ? Number(match[4]) : null,
    prereleaseTag: match[5] ?? null,
  };
}

/**
 * Classifies a GitHub Release into `beta` or `release` (`>= 1.0.0` non-prerelease).
 */
export function classifyReleaseChannel(release) {
  const parsed = parseSemverTag(release?.tag_name);
  const isPrereleaseFlag = Boolean(release?.prerelease);
  const hasPrereleaseSuffix = Boolean(parsed.prereleaseTag);
  const isBelowV1 = parsed.valid ? parsed.major < 1 : true;

  const isBeta = isPrereleaseFlag || hasPrereleaseSuffix || isBelowV1;
  return {
    channel: isBeta ? "beta" : "release",
    isBeta,
    semver: parsed,
  };
}

/**
 * Formats byte count into a compact human-readable string (e.g. `14.2 MB`).
 */
export function formatBytes(bytes) {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes <= 0) {
    return "";
  }
  if (bytes >= 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  }
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/**
 * Truncates text cleanly to Discord's field/description limits, preferring line boundaries.
 */
export function truncateText(text, maxLength, suffix = "…") {
  const str = String(text ?? "").trim();
  if (str.length <= maxLength) return str;
  const budget = Math.max(0, maxLength - suffix.length);
  const slice = str.slice(0, budget);
  const lastNewline = slice.lastIndexOf("\n");
  const cleanCut =
    lastNewline > Math.floor(budget * 0.6) ? slice.slice(0, lastNewline) : slice;
  return `${cleanCut.trimEnd()}${suffix}`;
}

/**
 * Filters out internal Tauri updater/signature files and groups user-facing
 * downloads by platform.
 */
export function groupReleaseAssets(assets) {
  const groups = {
    windows: [],
    macos: [],
    linux: [],
    mod: [],
    other: [],
  };

  if (!Array.isArray(assets)) return groups;

  for (const asset of assets) {
    const name = String(asset?.name ?? "").trim();
    const url = String(asset?.browser_download_url ?? "").trim();
    if (!name || !url) continue;

    if (IGNORED_ASSET_PATTERNS.some((re) => re.test(name))) {
      continue;
    }

    const entry = {
      name,
      url,
      sizeLabel: formatBytes(asset?.size),
    };

    const lower = name.toLowerCase();
    if (lower.endsWith(".exe") || lower.endsWith(".msi")) {
      groups.windows.push(entry);
    } else if (lower.endsWith(".dmg")) {
      groups.macos.push(entry);
    } else if (
      lower.endsWith(".appimage") ||
      lower.endsWith(".deb") ||
      lower.endsWith(".rpm")
    ) {
      groups.linux.push(entry);
    } else if (lower.endsWith(".zip") && /fs\d+_globalmarket/i.test(lower)) {
      groups.mod.push(entry);
    } else {
      groups.other.push(entry);
    }
  }

  return groups;
}

/**
 * Wraps a D-S-Agrarservice GitHub Release asset URL through the HTML download
 * bridge page when `portalBaseUrl` is enabled, so browsers like Opera GX do not
 * auto-close external direct-download tabs.
 */
export function buildPortalDownloadUrl(assetUrl, portalBaseUrl = DEFAULT_DOWNLOAD_PORTAL_URL) {
  const raw = String(assetUrl ?? "").trim();
  const base = String(portalBaseUrl ?? "").trim();
  if (!raw || !base) return raw;

  const match = raw.match(
    /^https:\/\/github\.com\/D-S-Agrarservice\/(GlobalMarketClient|FS25_GlobalMarket)\/releases\/download\/([^/]+)\/([^/?#]+)$/i,
  );
  if (!match) return raw;

  const normalizedBase = base.endsWith("/") ? base : `${base}/`;
  const params = new URLSearchParams({
    repo: match[1],
    tag: decodeURIComponent(match[2]),
    file: decodeURIComponent(match[3]),
  });
  return `${normalizedBase}?${params.toString()}`;
}

/**
 * Formats a list of platform assets as clickable direct GitHub download links.
 */
function formatAssetLinks(items) {
  return items
    .map((item) => {
      const sizeSuffix = item.sizeLabel ? ` (${item.sizeLabel})` : "";
      return `• [\`${item.name}\`](${item.url})${sizeSuffix}`;
    })
    .join("\n");
}

/**
 * Extracts a clean, compact changelog excerpt from a GitHub Release body,
 * removing redundant default download boilerplate if present.
 */
export function extractChangelogExcerpt(body, maxLength = 950) {
  const raw = String(body ?? "").trim();
  if (!raw) return "";

  // Strip the auto-generated `### Downloads` block from release.yml to avoid duplication.
  const withoutDownloads = raw
    .replace(/^###\s+Downloads[\s\S]*?(?=^#{1,3}\s+|(?![\s\S]))/gim, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();

  return truncateText(withoutDownloads || raw, maxLength);
}

/**
 * Computes Unix timestamp (seconds) for Discord `<t:UNIX:R>` relative timestamps.
 */
function resolvePublishedUnix(release) {
  const rawDate = release?.published_at || release?.created_at;
  const parsed = rawDate ? Date.parse(rawDate) : NaN;
  return Number.isFinite(parsed)
    ? Math.floor(parsed / 1000)
    : Math.floor(Date.now() / 1000);
}

/**
 * Detects whether a GitHub Release belongs to the Farming Simulator Mod (`FS25_GlobalMarket`)
 * rather than the desktop companion client (`GlobalMarketClient`).
 */
export function isModRelease(release, groupedAssets) {
  const hasDesktop =
    groupedAssets.windows.length > 0 ||
    groupedAssets.macos.length > 0 ||
    groupedAssets.linux.length > 0;
  if (groupedAssets.mod.length > 0 && !hasDesktop) return true;
  const hay = `${release?.html_url ?? ""} ${release?.name ?? ""}`.toLowerCase();
  return !hasDesktop && /fs\d+_globalmarket/i.test(hay);
}

/**
 * Selects the primary user-facing asset(s) to upload directly as native Discord
 * message attachments (e.g. `FS25_GlobalMarket.zip` for mod releases or the
 * Windows `-setup.exe` installer for client releases), staying safely below
 * Discord's 10 MiB standard upload limit.
 */
export function selectDiscordAttachmentAssets(
  release,
  maxTotalBytes = MAX_DISCORD_ATTACHMENT_BYTES,
) {
  const assets = Array.isArray(release?.assets) ? release.assets : [];
  const grouped = groupReleaseAssets(assets);
  const modRelease = isModRelease(release, grouped);

  const byName = new Map(
    assets
      .filter((a) => a?.name && a?.browser_download_url)
      .map((a) => [String(a.name).trim(), a]),
  );

  const candidates = [];
  if (modRelease) {
    for (const entry of grouped.mod) {
      const raw = byName.get(entry.name);
      if (raw) candidates.push(raw);
    }
  } else {
    // Prefer the standard Windows NSIS `-setup.exe` installer (~3-4 MB).
    const setupExe = grouped.windows.find((e) => /setup\.exe$/i.test(e.name));
    const anyExe = grouped.windows.find((e) => /\.exe$/i.test(e.name));
    const chosen = setupExe || anyExe || grouped.windows[0];
    if (chosen) {
      const raw = byName.get(chosen.name);
      if (raw) candidates.push(raw);
    }
  }

  const selected = [];
  let totalBytes = 0;
  for (const raw of candidates) {
    const size = Number(raw.size ?? 0);
    if (!Number.isFinite(size) || size <= 0) continue;
    if (totalBytes + size > maxTotalBytes) continue;
    selected.push({
      name: String(raw.name).trim(),
      url: String(raw.browser_download_url).trim(),
      apiUrl: raw.url ? String(raw.url).trim() : null,
      size,
      sizeLabel: formatBytes(size),
    });
    totalBytes += size;
  }

  return selected;
}

/**
 * Builds the bilingual (DE / EN) **Download Card** payload for the locked `#gm-downloads` channel.
 * Designed to be edited in-place (`PATCH`) on every release so old versions vanish automatically.
 */
export function buildDownloadCardPayload(release, options = {}) {
  const { isBeta } = classifyReleaseChannel(release);
  const tag = String(release?.tag_name ?? "unknown").trim();
  const assets = groupReleaseAssets(release?.assets);
  const modRelease = isModRelease(release, assets);

  const defaultName = modRelease ? `FS25_GlobalMarket ${tag}` : `GlobalMarketClient ${tag}`;
  const defaultUrl = modRelease
    ? "https://github.com/D-S-Agrarservice/FS25_GlobalMarket/releases"
    : "https://github.com/D-S-Agrarservice/GlobalMarketClient/releases";

  const titleName = String(release?.name || defaultName).trim();
  const releaseUrl = String(release?.html_url ?? "").trim() || defaultUrl;
  const publishedUnix = resolvePublishedUnix(release);

  const badge = isBeta ? "🧪 BETA" : "🚀 RELEASE";
  const color = isBeta ? EMBED_COLOR_BETA : EMBED_COLOR_RELEASE;
  const supportChannelMention = options.supportChannelId
    ? `<#${options.supportChannelId}>`
    : "**#gm-support**";

  const descriptionLines = isBeta
    ? [
        `🇩🇪 **Aktuelle Beta-Version** — dieser Beitrag wird automatisch bei jedem Update ersetzt.`,
        `🇬🇧 **Latest Beta Build** — this card is automatically replaced on every update.`,
        "",
        `📌 **Version:** \`${tag}\` · **Status:** ${badge} · **Update:** <t:${publishedUnix}:R>`,
      ]
    : [
        `🇩🇪 **Aktuelle Release-Version** — dieser Beitrag wird automatisch bei jedem Update ersetzt.`,
        `🇬🇧 **Latest Stable Release** — this card is automatically replaced on every update.`,
        "",
        `📌 **Version:** \`${tag}\` · **Status:** ${badge} · **Update:** <t:${publishedUnix}:R>`,
      ];

  const fields = [];

  if (assets.windows.length > 0) {
    fields.push({
      name: "🪟 Windows (10 / 11 x64)",
      value: truncateText(formatAssetLinks(assets.windows), 1024),
      inline: false,
    });
  }

  if (assets.macos.length > 0) {
    fields.push({
      name: "🍎 macOS (Universal — Apple Silicon & Intel)",
      value: truncateText(formatAssetLinks(assets.macos), 1024),
      inline: false,
    });
  }

  if (assets.linux.length > 0) {
    fields.push({
      name: "🐧 Linux / Steam Deck (x64)",
      value: truncateText(formatAssetLinks(assets.linux), 1024),
      inline: false,
    });
  }

  if (assets.mod.length > 0) {
    fields.push({
      name: "🚜 Farming Simulator Mod (.zip)",
      value: truncateText(formatAssetLinks(assets.mod), 1024),
      inline: false,
    });
  }

  if (fields.length === 0) {
    fields.push({
      name: "📥 Downloads",
      value: `[GitHub Release — ${tag}](${releaseUrl})`,
      inline: false,
    });
  }

  const portalUrl =
    options.downloadPortalUrl !== undefined
      ? options.downloadPortalUrl
      : DEFAULT_DOWNLOAD_PORTAL_URL;

  const fallbackLine = portalUrl
    ? `🌐 **Download startet nicht (z. B. Opera GX)? / Not starting?** → **[DSA Download-Seite / Download Page](${portalUrl})** · [GitHub Release](${releaseUrl})`
    : `🔗 **[Alle Dateien & Changelog auf GitHub / Full Release on GitHub](${releaseUrl})**`;

  const notesLines = modRelease
    ? [
        `🇩🇪 **Installation:** Datei direkt laden (Link oben oder Anhang unten) und unverändert in deinen FS25-\`mods\`-Ordner legen *(nicht entpacken)*. Hilfe & Support: ${supportChannelMention}`,
        `🇬🇧 **Installation:** Download via the link above or attachment below and drop the \`.zip\` directly into your FS25 \`mods\` folder *(do not extract)*. Support: ${supportChannelMention}`,
        fallbackLine,
      ]
    : [
        `🇩🇪 **Auto-Update / Windows:** Windows-\`.exe\` auch direkt als Anhang unten *(SmartScreen: „Weitere Infos → Trotzdem ausführen“)*. Bestehende Installationen aktualisieren sich in der App. Hilfe & Diagnose: ${supportChannelMention}`,
        `🇬🇧 **Auto-Update / Windows:** Windows \`.exe\` also attached below *(SmartScreen: More info → Run anyway)*. Existing installs update in-app. Support & Diagnostics: ${supportChannelMention}`,
        fallbackLine,
      ];

  fields.push({
    name: "ℹ️ Hinweise / Notes",
    value: truncateText(notesLines.join("\n"), 1024),
    inline: false,
  });

  const footerLabel = modRelease
    ? isBeta
      ? "D-S-Agrarservice · FS25_GlobalMarket Mod (Beta) · Auto-updated via GitHub Releases"
      : "D-S-Agrarservice · FS25_GlobalMarket Mod · Auto-updated via GitHub Releases"
    : isBeta
      ? "D-S-Agrarservice · GlobalMarket Beta Channel · Auto-updated via GitHub Releases"
      : "D-S-Agrarservice · GlobalMarket Official Release · Auto-updated via GitHub Releases";

  return {
    username: modRelease ? "GlobalMarket Mod Releases" : "GlobalMarket Releases",
    embeds: [
      {
        title: `${badge} · ${truncateText(titleName, 220)}`,
        url: releaseUrl,
        color,
        description: truncateText(descriptionLines.join("\n"), 4096),
        fields,
        footer: {
          text: footerLabel,
        },
        timestamp: new Date(publishedUnix * 1000).toISOString(),
      },
    ],
  };
}

/**
 * Builds the bilingual (DE / EN) **Announcement / Changelog** payload intended for
 * the active Beta (`< 1.0.0`) or Release (`>= 1.0.0`) thread in `#globalmarket`.
 */
export function buildAnnouncementPayload(release, options = {}) {
  const { isBeta } = classifyReleaseChannel(release);
  const tag = String(release?.tag_name ?? "unknown").trim();
  const assets = groupReleaseAssets(release?.assets);
  const modRelease = isModRelease(release, assets);

  const defaultName = modRelease ? `FS25_GlobalMarket ${tag}` : `GlobalMarketClient ${tag}`;
  const defaultUrl = modRelease
    ? "https://github.com/D-S-Agrarservice/FS25_GlobalMarket/releases"
    : "https://github.com/D-S-Agrarservice/GlobalMarketClient/releases";

  const titleName = String(release?.name || defaultName).trim();
  const releaseUrl = String(release?.html_url ?? "").trim() || defaultUrl;
  const publishedUnix = resolvePublishedUnix(release);
  const rawChangelog = extractChangelogExcerpt(release?.body, 100000);

  const badge = isBeta ? "🧪 NEW BETA" : "🚀 NEW RELEASE";
  const color = isBeta ? EMBED_COLOR_BETA : EMBED_COLOR_RELEASE;
  const downloadMention = options.downloadChannelId
    ? `<#${options.downloadChannelId}>`
    : "**#gm-downloads**";
  const supportMention = options.supportChannelId
    ? `<#${options.supportChannelId}>`
    : "**#gm-support**";

  const roleMention = options.roleId ? `<@&${options.roleId}> ` : "";
  const productLabel = modRelease ? "FS25_GlobalMarket Mod" : "GlobalMarketClient";
  const content = isBeta
    ? `${roleMention}🧪 **${productLabel} Beta \`${tag}\`** ist verfügbar / is now available!`
    : `${roleMention}🚀 **${productLabel} \`${tag}\`** ist verfügbar / is now available!`;

  const introLine = isBeta
    ? `🇩🇪 Neue **Beta-Version** veröffentlicht – Download in ${downloadMention}.\n🇬🇧 New **Beta build** published – download in ${downloadMention}.`
    : `🇩🇪 Neue **Release-Version** veröffentlicht – Download in ${downloadMention}.\n🇬🇧 New **Stable Release** published – download in ${downloadMention}.`;

  // Place changelog in embed `description` (4096-char Discord limit instead of 1024-char field limit).
  let description = introLine;
  if (rawChangelog) {
    const moreSuffix = `\n…\n*(Gekürzt / Truncated — [komplettes Changelog auf GitHub](${releaseUrl}))*`;
    const changelogBudget = Math.max(500, 3950 - introLine.length - 40);
    const changelogBody = truncateText(rawChangelog, changelogBudget, moreSuffix);
    description = `${introLine}\n\n${changelogBody}`;
  }

  const fields = [
    {
      name: "📥 Download & Support",
      value: [
        `• **Downloads:** ${downloadMention} · [GitHub Release (${tag})](${releaseUrl})`,
        `• **Support / Diagnostics:** ${supportMention}`,
      ].join("\n"),
      inline: false,
    },
  ];

  return {
    username: "GlobalMarket Updates",
    content,
    allowed_mentions: options.roleId ? { roles: [String(options.roleId)] } : { parse: [] },
    embeds: [
      {
        title: `${badge} · ${truncateText(titleName, 220)}`,
        url: releaseUrl,
        color,
        description: truncateText(description, 4096),
        fields,
        footer: {
          text: "D-S-Agrarservice · GlobalMarket",
        },
        timestamp: new Date(publishedUnix * 1000).toISOString(),
      },
    ],
  };
}
