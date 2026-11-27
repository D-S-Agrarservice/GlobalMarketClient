/**
 * Zero-dependency Discord Webhook API client supporting:
 * - Posting new messages (`POST ...?wait=true`)
 * - Editing existing messages in-place (`PATCH .../messages/{id}`)
 * - Deleting webhook messages (`DELETE .../messages/{id}`)
 * - Thread routing (`?thread_id=...`)
 * - Automatic 429 rate-limit backoff and webhook token masking
 */

const DISCORD_WEBHOOK_REGEX =
  /^https:\/\/(?:(?:canary|ptb)\.)?discord(?:app)?\.com\/api\/(?:v\d+\/)?webhooks\/(\d+)\/([A-Za-z0-9._-]+)\/?$/;

/**
 * Validates that a string is a legitimate Discord Webhook URL.
 */
export function isValidWebhookUrl(url) {
  if (typeof url !== "string") return false;
  return DISCORD_WEBHOOK_REGEX.test(url.trim());
}

/**
 * Masks the secret webhook token so URLs are safe to print in logs/errors.
 */
export function maskWebhookUrl(url) {
  if (typeof url !== "string" || !url.trim()) return "<empty>";
  return url.trim().replace(
    /(webhooks\/\d+\/)([A-Za-z0-9._-]+)/,
    "$1<REDACTED>",
  );
}

/**
 * Normalizes a Discord Webhook base URL (strips trailing slash and query params).
 */
export function normalizeWebhookBaseUrl(rawUrl) {
  const trimmed = (rawUrl ?? "").trim();
  const parsed = new URL(trimmed);
  const clean = `${parsed.origin}${parsed.pathname}`.replace(/\/+$/, "");
  if (!isValidWebhookUrl(clean)) {
    throw new Error(
      `Invalid Discord Webhook URL format: ${maskWebhookUrl(trimmed)}`,
    );
  }
  return clean;
}

/**
 * Builds the final endpoint URL with optional messageId and query parameters.
 */
function buildEndpointUrl(webhookUrl, { messageId, threadId, wait } = {}) {
  const base = normalizeWebhookBaseUrl(webhookUrl);
  const path = messageId ? `${base}/messages/${encodeURIComponent(messageId)}` : base;
  const params = new URLSearchParams();
  if (wait) params.set("wait", "true");
  if (threadId && String(threadId).trim()) {
    params.set("thread_id", String(threadId).trim());
  }
  const qs = params.toString();
  return qs ? `${path}?${qs}` : path;
}

/**
 * Performs a Discord Webhook request with 429 rate-limit retry handling.
 */
async function requestWithRetry(url, init, maxRetries = 3) {
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    const response = await fetch(url, init);

    if (response.status === 429 && attempt < maxRetries) {
      let retryAfterSec = 2;
      try {
        const body = await response.json();
        if (typeof body?.retry_after === "number" && body.retry_after > 0) {
          retryAfterSec = Math.min(body.retry_after, 30);
        }
      } catch {
        // Fallback to default backoff.
      }
      await new Promise((r) => setTimeout(r, Math.ceil(retryAfterSec * 1000) + 150));
      continue;
    }

    if (!response.ok) {
      const errorText = await response.text().catch(() => "");
      throw new Error(
        `Discord Webhook HTTP ${response.status} (${maskWebhookUrl(url)}): ${errorText.slice(0, 400)}`,
      );
    }

    if (response.status === 204) {
      return null;
    }

    return response.json();
  }

  throw new Error(`Discord Webhook exceeded retry limit (${maskWebhookUrl(url)})`);
}

/**
 * Builds either a JSON or `multipart/form-data` request body depending on whether
 * binary file attachments are provided.
 */
function buildRequestInit(method, bodyObj, files = []) {
  if (Array.isArray(files) && files.length > 0) {
    const form = new FormData();
    form.append("payload_json", JSON.stringify(bodyObj));
    files.forEach((file, idx) => {
      const blob = new Blob([file.data], {
        type: file.contentType || "application/octet-stream",
      });
      form.append(`files[${idx}]`, blob, file.filename);
    });
    return { method, body: form };
  }

  return {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(bodyObj),
  };
}

/**
 * Sends a new message via webhook (with optional file attachments)
 * and returns the created Discord Message object.
 */
export async function sendWebhookMessage(
  webhookUrl,
  payload,
  { threadId, files = [] } = {},
) {
  const url = buildEndpointUrl(webhookUrl, { threadId, wait: true });
  return requestWithRetry(url, buildRequestInit("POST", payload, files));
}

/**
 * Edits an existing message created by the same webhook in-place.
 * When `files` are supplied, `attachments: []` is included so previous file
 * attachments are cleanly replaced by the new release files.
 */
export async function editWebhookMessage(
  webhookUrl,
  messageId,
  payload,
  { threadId, files = [] } = {},
) {
  if (!messageId || !String(messageId).trim()) {
    throw new Error("editWebhookMessage requires a non-empty messageId.");
  }
  const url = buildEndpointUrl(webhookUrl, { messageId, threadId });
  // PATCH /webhooks/{id}/{token}/messages/{msg} does not accept `username`, `avatar_url`, `thread_name`, or `applied_tags`.
  const {
    username: _u,
    avatar_url: _a,
    thread_name: _tn,
    applied_tags: _at,
    ...patchBody
  } = payload;
  if (Array.isArray(files) && files.length > 0) {
    patchBody.attachments = [];
  }
  return requestWithRetry(url, buildRequestInit("PATCH", patchBody, files));
}

/**
 * Deletes an existing message created by the same webhook.
 */
export async function deleteWebhookMessage(webhookUrl, messageId, { threadId } = {}) {
  if (!messageId || !String(messageId).trim()) {
    throw new Error("deleteWebhookMessage requires a non-empty messageId.");
  }
  const url = buildEndpointUrl(webhookUrl, { messageId, threadId });
  return requestWithRetry(url, { method: "DELETE" });
}
