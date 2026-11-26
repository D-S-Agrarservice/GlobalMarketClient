# Support & Diagnostic Mode (Extended)

How support cases are handled in `GlobalMarketClient`: the access tiers, the temporary **Extended** mode and the **Copy Diagnostics** report.

## 1. Access tiers

| Tier | Badge | Unlock | Lifetime | Adds |
| :--- | :--- | :--- | :--- | :--- |
| `user` | – | default | permanent | standard player UI, official backend URL only |
| `extended` | `EXTENDED DEBUG · NN min` (cyan) | support key | **30 minutes**, never saved | debug strip and extended diagnostic report |
| `developer` | `DEV / MAINTENANCE` (purple) | developer key | saved across restarts | everything from `extended`, plus mock tools and custom backend URLs |

The tier is a **local UI gate only**. The backend never trusts it: nothing on the server may depend on a client claiming to be in Extended or Developer mode (see D-S-Agrarservice/GlobalMarketClient#6).

**Keys:** the source contains neither the keys nor their digests. A build gets the digests from the environment variables `GM_EXTENDED_KEY_DIGEST` and `GM_DEVELOPER_KEY_DIGEST` (format `<hex16>:<hex16>`). The release workflow takes them from the repository secrets of the same name and stops if one is missing. A local build without them unlocks no tier. To change a key, run `cargo run --example access_key_digest` in `src-tauri`, type the key and store the printed digest as the secret. Older builds keep accepting the keys they were built with.

* **Support key (Extended):** short and easy to type, because support hands it out to players. It is guessable from the public digest; that is acceptable, since Extended only shows diagnostics, ends after 30 minutes and always uses the official backend.
* **Developer key:** only for the maintainers. Use a random key with at least 24 characters, so it can't be guessed.

**Developer token (development server):** since API 2.10 the development server (`SERVER_ENV=development`) needs an active `X-Dev-Token` on every request and answers `403 DEV_TOKEN_REQUIRED` / `DEV_TOKEN_INVALID` otherwise. The tokens are created in the backend admin (page *Protection*). In Developer mode, enter the token under **Settings → Developer token** and save. It is stored in the OS-protected key vault next to the market keys, never returned to the WebView and redacted from diagnostic reports, which show only `dev_token: "set"` or `"none"`. The client sends it only to `https://dev.gm.bydora.de` and to local loopback backends (`127.0.0.1`, `localhost`), never to production or another custom URL. Unlike the developer key, this token is what actually protects the dev server.

## 2. Unlocking and leaving Extended mode

1. Click the version number below the app title (or the market logo in the header) **5 times within 4 seconds**. The "Support & Diagnostic Mode" dialog opens.
2. Enter the support key given by support and click **Unlock Mode**.
3. The cyan badge in the header shows the time left, e.g. `EXTENDED DEBUG · 23 min`. Clicking the badge reopens the dialog.

Extended mode ends:
* automatically after **30 minutes** (log entry "Diagnostic mode automatically ended after 30 minutes."),
* with **Reset to Standard User Mode** in the dialog,
* on every app restart (Extended is never written to `settings.json`).

Unlocking Extended always switches to the **live official backend** (`DEFAULT_OFFICIAL_BACKEND_URL`); a custom URL or the mock backend is only available in Developer mode. Leaving Extended therefore never needs a URL reset.

## 3. What Extended mode shows in the app

* **Header line:** active server URL and backend API version.
* **Debug strip** below the status card: bridge folder, lifebit sequence (`#seq (bit)`), XML queue counters (`R:` read / `W:` write) and the time of the last sync.

## 4. Copy Diagnostics report

The **Copy Diagnostics** button copies a Markdown/YAML block for Discord or GitHub issues. It is available in every tier.

**Always included:** app version and protocol, game edition, OS, UI settings (incl. tier), backend mode and URL, market name and branding, market key state (redacted), client slot prefix (first 6 characters only), connection state with version limits, profile and bridge folders, bridge file counters, whitelist counters, contract bridge counters, active great demands, last error and the **last 15 log lines**.

**Additionally in Extended and Developer mode:**
* `api_version` reported by the backend,
* `timing`: last sync, last heartbeat and the time left in Extended mode (UTC times),
* `recent_errors`: the last **10 warnings/errors** from the whole log buffer (250 entries),
* `recent_logs` with the **last 50** instead of 15 lines.

**Redaction (all tiers):** local user names in paths become `<USER>` / `%USERPROFILE%`, the full client ID becomes `<CLIENT>`, and every market key and the developer token from the OS-protected key vault become `<REDACTED>`, in the last error, the error list and all log lines.

## 5. Support workflow

1. Ask the player to reproduce the problem.
2. Give them the support key and let them unlock Extended mode (section 2).
3. Ask them to reproduce once more, then click **Copy Diagnostics** and paste the report into the issue or Discord thread.
4. Extended mode ends by itself after 30 minutes; no further action is needed.
