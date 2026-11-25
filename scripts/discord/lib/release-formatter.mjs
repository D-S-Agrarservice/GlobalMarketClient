/**
 * Release classifier and bilingual (DE / EN) Discord Embed formatter
 * for D-S-Agrarservice GlobalMarketClient (and future FS25_GlobalMarket mod) releases.
 */

export const EMBED_COLOR_BETA = 0xf59e0b; // Amber / Beta
export const EMBED_COLOR_RELEASE = 0x4fa83d; // DSA Green / Stable v1.0+

const IGNORED_ASSET_PATTERNS = [
  /^latest\.json$/i,
  /\.sig$/i,
  /\.app\.tar\.gz$/i,
  /\.nsis\.zip$/i,
  /\.blockmap$/i,
];

/**
 * Parses a Git tag such as `v0.9.16-beta`, `0.9.16`, `v1.0.0`, or `v1.0.0-rc.1`.
 */
export function parseSemverTag(tagName) {
  const raw = String(tagName ?? "").trim();
  const match = raw.match(
    /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+([0-9A-Za-z.-]+))?$/,
  );
  if (!match) {
    return {
      raw,
      valid: false,
      major: 0,
      minor: 0,
      patch: 0,
      prereleaseTag: raw.includes("-") ? raw.split("-").slice(1).join("-") : null,
    };
  }
  return {
    raw,
    valid: true,
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
    prereleaseTag: match[4] ?? null,
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
 * Truncates text cleanly to Discord's field/description limits.
 */
export function truncateText(text, maxLength) {
  const str = String(text ?? "").trim();
  if (str.length <= maxLength) return str;
  return `${str.slice(0, Math.max(0, maxLength - 1)).trimEnd()}…`;
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
 * Formats a list of platform assets as clickable Markdown links.
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
 * Builds the bilingual (DE / EN) **Download Card** payload for the locked `#gm-downloads` channel.
 * Designed to be edited in-place (`PATCH`) on every release so old versions vanish automatically.
 */
export function buildDownloadCardPayload(release, options = {}) {
  const { isBeta } = classifyReleaseChannel(release);
  const tag = String(release?.tag_name ?? "unknown").trim();
  const titleName = String(release?.name || `GlobalMarketClient ${tag}`).trim();
  const releaseUrl =
    String(release?.html_url ?? "").trim() ||
    "https://github.com/D-S-Agrarservice/GlobalMarketClient/releases";
  const publishedUnix = resolvePublishedUnix(release);
  const assets = groupReleaseAssets(release?.assets);

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
      name: "🚜 Farming Simulator Mod",
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

  fields.push({
    name: "ℹ️ Hinweise / Notes",
    value: truncateText(
      [
        `🇩🇪 **Auto-Update:** Bestehende Installationen aktualisieren sich direkt in der App. Bei Erstinstallation SmartScreen („Weitere Infos → Trotzdem ausführen“) bzw. macOS Gatekeeper beachten. Hilfe & Diagnose: ${supportChannelMention}`,
        `🇬🇧 **Auto-Update:** Existing installs update right inside the app. On first install, allow Windows SmartScreen (*More info → Run anyway*) or macOS Gatekeeper (*Open Anyway*). Support & Diagnostics: ${supportChannelMention}`,
        `🔗 **[Alle Dateien & Changelog auf GitHub / Full Release on GitHub](${releaseUrl})**`,
      ].join("\n"),
      1024,
    ),
    inline: false,
  });

  return {
    username: "GlobalMarket Releases",
    embeds: [
      {
        title: `${badge} · ${truncateText(titleName, 220)}`,
        url: releaseUrl,
        color,
        description: truncateText(descriptionLines.join("\n"), 4096),
        fields,
        footer: {
          text: isBeta
            ? "D-S-Agrarservice · GlobalMarket Beta Channel · Auto-updated via GitHub Releases"
            : "D-S-Agrarservice · GlobalMarket Official Release · Auto-updated via GitHub Releases",
        },
        timestamp: new Date(publishedUnix * 1000).toISOString(),
      },
    ],
  };
}

/**
 * Builds the bilingual (DE / EN) **Announcement / Changelog** payload intended for
 * the active Beta (`< 1.0.0`) or Release (`>= 1.0.0`) channel/thread.
 */
export function buildAnnouncementPayload(release, options = {}) {
  const { isBeta } = classifyReleaseChannel(release);
  const tag = String(release?.tag_name ?? "unknown").trim();
  const titleName = String(release?.name || `GlobalMarketClient ${tag}`).trim();
  const releaseUrl =
    String(release?.html_url ?? "").trim() ||
    "https://github.com/D-S-Agrarservice/GlobalMarketClient/releases";
  const publishedUnix = resolvePublishedUnix(release);
  const changelog = extractChangelogExcerpt(release?.body, 1100);

  const badge = isBeta ? "🧪 NEW BETA" : "🚀 NEW RELEASE";
  const color = isBeta ? EMBED_COLOR_BETA : EMBED_COLOR_RELEASE;
  const downloadMention = options.downloadChannelId
    ? `<#${options.downloadChannelId}>`
    : "**#gm-downloads**";
  const supportMention = options.supportChannelId
    ? `<#${options.supportChannelId}>`
    : "**#gm-support**";

  const roleMention = options.roleId ? `<@&${options.roleId}> ` : "";
  const content = isBeta
    ? `${roleMention}🧪 **GlobalMarket Beta \`${tag}\`** ist verfügbar / is now available!`
    : `${roleMention}🚀 **GlobalMarket \`${tag}\`** ist verfügbar / is now available!`;

  const fields = [];
  if (changelog) {
    fields.push({
      name: "📝 Changelog / Änderungen",
      value: truncateText(changelog, 1024),
      inline: false,
    });
  }

  fields.push({
    name: "📥 Download & Support",
    value: [
      `• **Downloads:** ${downloadMention} · [GitHub Release (${tag})](${releaseUrl})`,
      `• **Support / Diagnostics:** ${supportMention}`,
    ].join("\n"),
    inline: false,
  });

  return {
    username: "GlobalMarket Updates",
    content,
    allowed_mentions: options.roleId ? { roles: [String(options.roleId)] } : { parse: [] },
    embeds: [
      {
        title: `${badge} · ${truncateText(titleName, 220)}`,
        url: releaseUrl,
        color,
        description: isBeta
          ? `🇩🇪 Neue **Beta-Version** veröffentlicht. Die Download-Karte in ${downloadMention} wurde automatisch aktualisiert.\n🇬🇧 A new **Beta build** has been published. The download card in ${downloadMention} was updated automatically.`
          : `🇩🇪 Neue **Release-Version** veröffentlicht. Die Download-Karte in ${downloadMention} wurde automatisch aktualisiert.\n🇬🇧 A new **Stable Release** has been published. The download card in ${downloadMention} was updated automatically.`,
        fields,
        footer: {
          text: "D-S-Agrarservice · GlobalMarket",
        },
        timestamp: new Date(publishedUnix * 1000).toISOString(),
      },
    ],
  };
}
