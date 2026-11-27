#!/usr/bin/env node
/**
 * GitHub Release -> Discord Notifier & In-Place Download Card Updater.
 *
 * Modes:
 * 1. GitHub Actions (`release: [published]`): reads release payload from `GITHUB_EVENT_PATH`.
 * 2. Local file test: `--release-file tests/fixtures/sample-release-beta.json`
 * 3. Live GitHub fetch: `--repo D-S-Agrarservice/GlobalMarketClient [--tag v0.9.16-beta]`
 *
 * Flags:
 *   --dry-run                Print payloads and routing plan without calling Discord.
 *   --init-card              Post a new download card instead of editing an existing message ID
 *                            (prints the created `message_id` for saving in `.env` / GitHub Variables).
 *   --skip-announce          Only update the download card; skip the thread announcement post.
 *   --release-file <path>    Load GitHub release JSON from a local file.
 *   --repo <owner/repo>      Fetch release JSON from GitHub REST API.
 *   --tag <tag>              Tag to fetch when using `--repo` (defaults to latest release).
 */

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { loadDotEnv } from "./lib/env-loader.mjs";
import {
  editWebhookMessage,
  maskWebhookUrl,
  sendWebhookMessage,
} from "./lib/discord-webhook.mjs";
import {
  buildAnnouncementPayload,
  buildDownloadCardPayload,
  classifyReleaseChannel,
  selectDiscordAttachmentAssets,
} from "./lib/release-formatter.mjs";

loadDotEnv();

function parseArgs(argv) {
  const args = {
    dryRun: false,
    initCard: false,
    skipAnnounce: false,
    noAttach: false,
    announceMessageId: null,
    releaseFile: null,
    repo: null,
    tag: null,
  };

  for (let i = 0; i < argv.length; i++) {
    const token = argv[i];
    if (token === "--dry-run") args.dryRun = true;
    else if (token === "--init-card") args.initCard = true;
    else if (token === "--skip-announce") args.skipAnnounce = true;
    else if (token === "--no-attach") args.noAttach = true;
    else if (token === "--announce-message-id") args.announceMessageId = argv[++i];
    else if (token === "--release-file") args.releaseFile = argv[++i];
    else if (token === "--repo") args.repo = argv[++i];
    else if (token === "--tag") args.tag = argv[++i];
    else if (token === "--help" || token === "-h") {
      console.log(
        [
          "Usage: node scripts/notify-release.mjs [options]",
          "",
          "Options:",
          "  --dry-run                Preview payloads without sending to Discord",
          "  --init-card              Create a new persistent download card and print its message ID",
          "  --skip-announce          Skip posting the changelog announcement into the thread",
          "  --release-file <path>    Load release JSON from disk",
          "  --repo <owner/repo>      Fetch release JSON from GitHub API",
          "  --tag <tag>              Specific release tag when using --repo (optional)",
        ].join("\n"),
      );
      process.exit(0);
    } else {
      throw new Error(`Unknown argument: ${token}`);
    }
  }

  return args;
}

async function fetchGitHubRelease(repo, tag) {
  const cleanRepo = String(repo).trim();
  const endpoint = tag
    ? `https://api.github.com/repos/${cleanRepo}/releases/tags/${encodeURIComponent(tag)}`
    : `https://api.github.com/repos/${cleanRepo}/releases/latest`;

  const headers = {
    Accept: "application/vnd.github+json",
    "User-Agent": "DSA-Discord-Integrations",
  };
  const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;
  if (token) {
    headers.Authorization = `Bearer ${token}`;
  }

  const res = await fetch(endpoint, { headers });
  if (res.status === 404 && !tag) {
    // `/releases/latest` ignores pre-releases; fall back to the newest published release (including betas).
    const listRes = await fetch(
      `https://api.github.com/repos/${cleanRepo}/releases?per_page=15`,
      { headers },
    );
    if (listRes.ok) {
      const releases = await listRes.json();
      const newestPublished = Array.isArray(releases)
        ? releases.find((r) => !r.draft)
        : null;
      if (newestPublished) {
        return newestPublished;
      }
    }
  }
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`GitHub API HTTP ${res.status} (${endpoint}): ${body.slice(0, 300)}`);
  }
  return res.json();
}

async function loadReleaseData(args) {
  if (args.releaseFile) {
    const raw = readFileSync(resolve(process.cwd(), args.releaseFile), "utf8");
    const parsed = JSON.parse(raw);
    return parsed.release ?? parsed;
  }

  if (args.repo) {
    return fetchGitHubRelease(args.repo, args.tag);
  }

  if (process.env.GITHUB_EVENT_PATH) {
    const raw = readFileSync(process.env.GITHUB_EVENT_PATH, "utf8");
    const event = JSON.parse(raw);
    if (event.release) {
      return event.release;
    }
    if (process.env.GITHUB_REPOSITORY) {
      return fetchGitHubRelease(
        process.env.GITHUB_REPOSITORY,
        process.env.RELEASE_TAG || args.tag || null,
      );
    }
  }

  throw new Error(
    "No release source provided. Use --release-file, --repo <owner/repo>, or run inside a GitHub Actions release workflow.",
  );
}

async function downloadReleaseAttachments(selectedAssets) {
  const files = [];
  const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;

  for (const asset of selectedAssets) {
    try {
      const targetUrl = token && asset.apiUrl ? asset.apiUrl : asset.url;
      const headers = {
        "User-Agent": "DSA-Discord-Integrations",
        Accept: "application/octet-stream",
      };
      if (token && asset.apiUrl) {
        headers.Authorization = `Bearer ${token}`;
      }
      const res = await fetch(targetUrl, { headers });
      if (!res.ok) {
        console.warn(
          `Warning: could not fetch attachment '${asset.name}' (HTTP ${res.status}) — posting embed links only.`,
        );
        continue;
      }
      const data = new Uint8Array(await res.arrayBuffer());
      files.push({
        filename: asset.name,
        data,
        contentType: "application/octet-stream",
      });
      console.log(`Prepared Discord attachment: ${asset.name} (${asset.sizeLabel})`);
    } catch (err) {
      console.warn(
        `Warning: failed to prepare attachment '${asset.name}' (${err.message}) — continuing with links only.`,
      );
    }
  }

  return files;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const release = await loadReleaseData(args);

  if (release.draft) {
    console.log(
      `Release '${release.tag_name}' is still a draft — skipping Discord notification.`,
    );
    return;
  }

  const classification = classifyReleaseChannel(release);
  const { channel, isBeta } = classification;

  const downloadWebhookUrl = (process.env.DISCORD_DOWNLOAD_WEBHOOK_URL ?? "").trim();
  const announceWebhookUrl = (process.env.DISCORD_ANNOUNCE_WEBHOOK_URL ?? "").trim();

  const targetMessageId = (
    isBeta
      ? process.env.DISCORD_BETA_MESSAGE_ID
      : process.env.DISCORD_RELEASE_MESSAGE_ID
  )?.trim();

  const targetThreadId = (
    isBeta
      ? process.env.DISCORD_BETA_THREAD_ID || process.env.GM_THREAD_BETA_ID
      : process.env.DISCORD_RELEASE_THREAD_ID || process.env.GM_THREAD_RELEASE_ID
  )?.trim();

  const targetRoleId = (
    isBeta ? process.env.DISCORD_BETA_ROLE_ID : process.env.DISCORD_RELEASE_ROLE_ID
  )?.trim();

  const downloadChannelId = (
    process.env.DISCORD_DOWNLOAD_CHANNEL_ID || process.env.GM_CHANNEL_DOWNLOADS_ID
  )?.trim();

  const supportChannelId = (
    process.env.DISCORD_SUPPORT_CHANNEL_ID || process.env.GM_CHANNEL_SUPPORT_ID
  )?.trim();

  const downloadCardPayload = buildDownloadCardPayload(release, {
    supportChannelId,
  });

  const announcementPayload = buildAnnouncementPayload(release, {
    downloadChannelId,
    supportChannelId,
    roleId: targetRoleId,
  });

  const attachmentCandidates = args.noAttach
    ? []
    : selectDiscordAttachmentAssets(release);

  console.log(`Release tag:       ${release.tag_name}`);
  console.log(`Detected channel:  ${channel.toUpperCase()}`);
  console.log(
    `Download action:   ${
      !args.initCard && targetMessageId
        ? `PATCH existing card (message_id=${targetMessageId})`
        : "POST new download card"
    }`,
  );
  console.log(
    `Attachments:       ${
      attachmentCandidates.length > 0
        ? attachmentCandidates.map((a) => `${a.name} (${a.sizeLabel})`).join(", ")
        : "none"
    }`,
  );
  console.log(
    `Announce action:   ${
      args.skipAnnounce
        ? "SKIPPED (--skip-announce)"
        : announceWebhookUrl
          ? `POST to ${maskWebhookUrl(announceWebhookUrl)}${targetThreadId ? ` (thread_id=${targetThreadId})` : ""}`
          : "SKIPPED (DISCORD_ANNOUNCE_WEBHOOK_URL not set)"
    }`,
  );

  if (args.dryRun) {
    console.log("\n--- Download Card Payload (DRY RUN) ---");
    console.log(JSON.stringify(downloadCardPayload, null, 2));
    if (!args.skipAnnounce) {
      console.log("\n--- Announcement Payload (DRY RUN) ---");
      console.log(JSON.stringify(announcementPayload, null, 2));
    }
    return;
  }

  if (!downloadWebhookUrl) {
    throw new Error(
      "DISCORD_DOWNLOAD_WEBHOOK_URL is required (or pass --dry-run to preview).",
    );
  }

  const files = await downloadReleaseAttachments(attachmentCandidates);

  // 1. Update (or create) the persistent download card in `#gm-downloads`.
  let cardMessage;
  if (!args.initCard && targetMessageId) {
    cardMessage = await editWebhookMessage(
      downloadWebhookUrl,
      targetMessageId,
      downloadCardPayload,
      { files },
    );
    console.log(`Updated download card in-place (message_id=${cardMessage.id}).`);
  } else {
    cardMessage = await sendWebhookMessage(downloadWebhookUrl, downloadCardPayload, {
      files,
    });
    const varName = isBeta ? "DISCORD_BETA_MESSAGE_ID" : "DISCORD_RELEASE_MESSAGE_ID";
    console.log(`Created new download card (message_id=${cardMessage.id}).`);
    console.log(`Save this ID as ${varName}=${cardMessage.id} so future updates replace this card in-place.`);
  }

  // 2. Optionally post (or edit in-place) a changelog announcement.
  if (!args.skipAnnounce && announceWebhookUrl) {
    if (args.announceMessageId) {
      const updatedMsg = await editWebhookMessage(
        announceWebhookUrl,
        args.announceMessageId,
        announcementPayload,
        { threadId: targetThreadId || undefined },
      );
      console.log(
        `Updated release announcement in-place (message_id=${updatedMsg.id}).`,
      );
    } else {
      const announceMsg = await sendWebhookMessage(
        announceWebhookUrl,
        announcementPayload,
        { threadId: targetThreadId || undefined },
      );
      console.log(
        `Posted release announcement (message_id=${announceMsg.id}${targetThreadId ? `, thread_id=${targetThreadId}` : ""}).`,
      );
    }
  }
}

main().catch((err) => {
  console.error(`Error: ${err.message}`);
  process.exit(1);
});
