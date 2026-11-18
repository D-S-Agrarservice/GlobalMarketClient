# Etappe 2 Handoff Specification: Cross-Server Supply Contracts (#25) & Direct Offers

**Mod Baseline Ready:** `FS25_GlobalMarket v0.9.9.34` (`gm_floowy`)  
**Client Baseline Ready:** `GlobalMarketClient v0.9.13` (Beta, `develop_floowy` / `main`)  
**Status (2026-11-18):** Mod side current at `v0.9.11.1` (later Etappe 2 changes: `v0.9.9.35` accepted `CANCEL_OFFER` returns unsold liters, `v0.9.9.37` cleanup on farm deletion), client at `v0.9.13`; the backend part (section 3, `GlobalMarketServer#25`) is still open. The XML formats themselves are specified in `External_Interface_Specification_FS25_GlobalMarket_Mod2EXE.md` (single source).  
**Remaining Scope:** Backend API (`GlobalMarketServer` / `gm.bydora.de`, FastAPI)  
**Tracked GitHub Issues:** Backend API & State Machine [`D-S-Agrarservice/GlobalMarketServer#25`](https://github.com/D-S-Agrarservice/GlobalMarketServer/issues/25) | Future Online NPCs [`D-S-Agrarservice/FS25_GlobalMarket#29`](https://github.com/D-S-Agrarservice/FS25_GlobalMarket/issues/29)

---

## 1. Overview & Non-Negotiable Architecture Invariants

The FS25 Lua mod has already implemented the complete Etappe 2 foundation (`GlobalMarketContractsOnline.lua`, `GlobalMarketContractsOnlineSync.lua`, `ContractOnlineBridgeEvent.lua`, `ONLINE_CONTRACTS_MODE`, and Local vs. Online UI selectors). Both `GlobalMarketClient` and `GlobalMarketServer` can now implement their side against this finalized XML contract without needing any follow-up mod patches.

### 1.1 Capability Gate (`contractsSupported`)
* Older `GlobalMarketClient` versions (`<= 0.9.11`) ignore and delete unknown `write/*.xml` actions (`IncomingPacket::IgnoredUnknownAction`).
* Therefore, `FS25_GlobalMarket` **keeps cross-server contract/offer publishing disabled** until the active Data-Host's `globalMarketOnline.xml` explicitly advertises:
  ```xml
  <globalMarketOnline timestamp="..." isConnected="true" contractsSupported="true" ...>
  ```
* `GlobalMarketClient` should only emit `contractsSupported="true"` once both the local XML bridge implementation **and** the backend handshake (`MetaStatus.contracts_supported == true`) confirm Etappe 2 support.

### 1.2 Persistent Server Identity (`serverInstanceId`)
* Each FS25 savegame generates and persists a 16-char lowercase hex `serverInstanceId` inside `<savegame>/globalMarket.xml` (for example `a3f901c48b2e7d10`).
* Every online action and board record identifies farms by `(serverId, farmId)` — **never** by Data-Host `client_id` or IP address — so Dedicated Server Data-Host failovers or restarts never sever ownership of open contracts, active deliveries, or pending settlements.

### 1.3 Anti-Arbitrage (Relative Multipliers Only)
* **Never transmit fixed Euro prices** across servers.
* **Supply Contracts:** Outgoing `CREATE_CONTRACT` carries `baseTrendFactor` (`1.0` = local Map-Spy `normalPrice` baseline) + `bonusPct` (`5..20`). Incoming `<contract>` and `<result>` carry `baseTrendFactor` + `bonusPct`. Every server multiplies `baseTrendFactor` by its own local Map-Spy `getContractReferencePricePer1000(fillType)`.
* **Direct Offers:** Outgoing `CREATE_OFFER` and incoming `<offer>` / `OFFER_SOLD` carry `priceDeltaPct` (`-10..+10` integer percent relative to local spot market price `getCurrentMarketPricePer1000(fillType)`).

### 1.4 Sandbox-Safe Real-Time Deadlines
* `os.time()` is `nil` in the FS25 Lua sandbox.
* Outgoing `CREATE_CONTRACT` (`24..168`) and `CREATE_OFFER` (`24..72`) send `durationHours` (`int`).
* Incoming `<contract>` and `<offer>` carry `remainingSeconds` (`int`, computed by backend against UTC `expires_at - now()`).
* Lua decrements `remainingSeconds` by `1` per second locally for UI display only; **all expirations, defaults, and `15%` breach penalties are triggered authoritatively by backend `<settlements>`**.

### 1.5 Conflict-Free `Pending -> Ack` Concurrency Model
* **Creating own listings (`CREATE_CONTRACT` / `CREATE_OFFER`):** Escrow (100% cash for contracts, physical stock for offers) is locked upfront locally with `status="PENDING_CREATE"`. If the backend returns `status="REJECTED"`, Lua refunds 100% escrow or moves the escrowed goods safely to the farm's `PickupInbox`.
* **Accepting / Buying foreign listings (`ACCEPT_CONTRACT` / `BUY_OFFER`):** Lua **does NOT** bind the contract or deduct buyer money/spawn goods locally on click. It records a `pendingContractActions[actionId]` lock (`30s` timeout) and waits for `<actionResults><result actionId="..." status="ACCEPTED|REJECTED" .../></actionResults>`.
  * Only one competing server receives `ACCEPTED` (`ACCEPT_CONTRACT` or exclusive `BUY_OFFER`); losing servers receive `REJECTED` and drop the pending request cleanly without ghost bookings.
  * If a buyer's target silo became full during the ~1–2s roundtrip after `BUY_OFFER` returns `ACCEPTED`, Lua deposits whatever fits and places any remainder into the buyer's `PickupInbox` (`sourceType="OFFER_PURCHASE"`, claimable at `0 EUR` shipping at a GM Terminal) so savegame and backend remain 100% consistent.

### 1.6 Cross-Market Global Contract Board & Future Online NPCs
* **Shared Global Pool across Markets:** While `/api/v1/market/*` (spot stock, trends, Great Demand) remains strictly partitioned per `X-Market-Token`, **Supply Contracts & Direct Offers (`/api/v1/contracts/*`) operate in a shared cross-market global pool** by default so farms on both the Official Market and Private Markets can trade contracts and direct offers together seamlessly (and live market switching never orphans active contracts or escrows).
  * `GlobalMarketClient` still forwards `X-Market-Token` on `/api/v1/contracts/*` so `GlobalMarketServer` retains full server-side control (e.g., to optionally add a per-token isolation flag later without any Client or Mod update).
  * Validate `CREATE_CONTRACT` and `CREATE_OFFER` `fill_type` against the **global `APPROVED` commodity pool** so unmoderated private-market test commodities never leak onto the shared board.
* NPC contracts and direct offers currently remain local per savegame (`NPC_ACTIVITY_MODE`). However, incoming `<contract>` and `<offer>` parsing already supports `isNpc="true"` (`origin="ONLINE_NPC"`) as a forward-compatible hook (tracked in [`D-S-Agrarservice/FS25_GlobalMarket#29`](https://github.com/D-S-Agrarservice/FS25_GlobalMarket#29)).

---

## 2. `GlobalMarketClient` (Rust + Tauri v2) Implementation Status (`v0.9.13` Beta — Implemented)

### 2.1 Handshake & Heartbeat (`api/types.rs` & `bridge/xml.rs`)
1. Extend `ServerHandshake` (`MetaStatus` JSON) with:
   ```rust
   #[serde(default)]
   pub contracts_supported: bool,
   ```
2. Write `contractsSupported="true"` (or `"false"`) onto `<globalMarketOnline ...>` in `write_heartbeat_xml_with_latest` when `handshake.online && handshake.contracts_supported`.

### 2.2 Outgoing Watcher: `write/contractAction_*.xml` (`bridge/xml.rs` & `bridge/watcher.rs`)
1. Add `IncomingPacket::ContractActions(Vec<ContractActionRequest>)` for `<gmDataSync action="contractAction">` in `bridge/xml.rs`.
2. Parse `<request ... />` child elements into typed DTOs with attributes:
   - Required: `subAction`, `actionId`, `serverId`
   - Optional: `farmId`, `farmName`, `remoteId`, `fillType`, `liters`, `baseTrendFactor`, `bonusPct`, `priceDeltaPct`, `allowPartial`, `durationHours`, `wasBale`, `wasPallet`, `settlementIds`
3. Forward parsed requests reliably to `POST /api/v1/contracts/actions` using the existing durable retry-queue pattern (`pending_tx.json` / `pending_contract_actions.json` with idempotency key `client_id + "_" + actionId` or `actionId` directly, since Lua already generates globally unique `<serverId>_<gTimeMs>_<counter>`).
4. Delete `write/contractAction_*.xml` strictly **after** queueing or processing, identical to `transaction_*.xml`.
5. Remember: `REQUEST_SYNC` (`subAction="REQUEST_SYNC"`) is sent when a Data-Host first promotes so the client immediately triggers a fresh `GET /api/v1/contracts/state` and writes `read/gmContractSync_*.xml`.

### 2.3 Incoming Writer: `read/gmContractSync_*.xml` (`bridge/xml.rs` & `bridge/coordinator.rs`)
1. **CRITICAL — Separate Filename Prefix from `gmDataSync_*.xml`!**
   * `bridge/xml.rs` currently prunes unread `gmDataSync_*.xml` files down to `MAX_RETAINED_MARKET_SYNC_FILES = 3`.
   * **NEVER** reuse the `gmDataSync_` filename prefix for `contractData`, and do **NOT** prune `gmContractSync_*.xml` with spot-market snapshots — otherwise spot-market updates while the game is closed or busy would evict unread `actionResults` or `settlements`!
   * Write contract sync packets via `.staging/` atomic rename as **`read/gmContractSync_<ts>_<seq>.xml`**.
2. Lua already polls all `read/*.xml` chronologically every `1000 ms`, routes `action="contractData"` via `onReadCustomSyncXml`, and deletes each processed file.
3. Clamp integer fields (`liters`, `remainingSeconds`, `totalLiters`, `deliveredLiters`, `missingLiters`) to `0..=i32::MAX` (`2_147_483_647`) and format floats (`baseTrendFactor`) with invariant dot-decimal (`format!("{factor:.4}")`).
4. Note on `actionResults`:
   * Wait-free ACK delivery: Either `POST /api/v1/contracts/actions` returns the immediate `ContractActionResult` AND/OR the next `GET /api/v1/contracts/state?server_id=<serverId>` returns recent unacknowledged `actionResults` (e.g. TTL 5 minutes or until superseded/acked) alongside unacknowledged `settlements`. Returning `actionResults` both immediately upon `POST` (writing `gmContractSync_*.xml` at once) AND keeping recent results in `GET /state` guarantees zero lost Acks across brief network hiccups!
5. Note on `server_id` Discovery for `GET /api/v1/contracts/state`:
   * The Rust client learns the active savegame's `serverId` from any outgoing `write/contractAction_*.xml` (including the automatic `REQUEST_SYNC` packet emitted when Lua becomes Data-Host!). Persist the last seen `server_id` in coordinator state (cleared/updated when a new `serverId` arrives, and reset on profile directory change). Even if `server_id` isn't known yet, `GET /api/v1/contracts/state` without `server_id` can still deliver open `<contracts>` and `<directOffers>` safely.

---

## 3. `GlobalMarketServer` (`gm.bydora.de` FastAPI) API & State Machine Proposal

All endpoints respect the `X-Market-Token` header (`official` market when absent, isolated private market when present).

### 3.1 Handshake Update (`GET /api/v1/meta/status`)
Add optional boolean field to `MetaStatus`:
```json
{
  "online": true,
  "min_exe_version": "0.9.10",
  "latest_exe_version": "0.9.12",
  "poll_interval_sec": 10,
  "market_name": "Official Global Market",
  "api_version": "2.0.0",
  "contracts_supported": true
}
```

### 3.2 Contract State Poll (`GET /api/v1/contracts/state`)
* **Query param:** `server_id` (optional string, e.g. `"a3f901c48b2e7d10"`).
* **Headers:** `X-Market-Token` (optional), `If-None-Match` (`ETag` -> `304 Not Modified`).
* **Response `200 OK`:**
  ```json
  {
    "updated_at": 1763395200,
    "contracts": [
      {
        "remote_id": "c_101",
        "origin": "ONLINE",
        "is_npc": false,
        "issuer_server_id": "99aa11bb22cc33dd",
        "issuer_farm_id": 1,
        "issuer_name": "Nordhof [Server A]",
        "contractor_server_id": "",
        "contractor_farm_id": 0,
        "fill_type": "WHEAT",
        "total_liters": 50000,
        "delivered_liters": 0,
        "base_trend_factor": 1.0,
        "bonus_pct": 12,
        "remaining_seconds": 172800,
        "status": "OPEN"
      }
    ],
    "direct_offers": [
      {
        "remote_id": "o_205",
        "origin": "ONLINE",
        "is_npc": false,
        "seller_server_id": "99aa11bb22cc33dd",
        "seller_farm_id": 2,
        "seller_name": "Südhof [Server A]",
        "fill_type": "BARLEY",
        "total_liters": 25000,
        "remaining_liters": 25000,
        "price_delta_pct": -5,
        "allow_partial": true,
        "remaining_seconds": 86400,
        "was_bale": false,
        "was_pallet": false,
        "status": "OPEN"
      }
    ],
    "action_results": [
      {
        "action_id": "a3f901c48b2e7d10_12000_1",
        "sub_action": "BUY_OFFER",
        "status": "ACCEPTED",
        "reason": "",
        "remote_id": "o_205",
        "granted_liters": 10000,
        "base_trend_factor": 1.0,
        "bonus_pct": 0,
        "remaining_seconds": 0
      }
    ],
    "settlements": [
      {
        "settlement_id": "set_901",
        "type": "CONTRACT_DELIVERY",
        "target_server_id": "a3f901c48b2e7d10",
        "target_farm_id": 1,
        "remote_id": "c_101",
        "fill_type": "WHEAT",
        "liters": 20000,
        "missing_liters": 0,
        "base_trend_factor": 1.0,
        "price_delta_pct": 0,
        "source_label": "#c_101 (Südhof)"
      }
    ]
  }
  ```
* **Visibility filtering on backend:**
  * `contracts`: return all `OPEN` contracts in the market, PLUS any `ACTIVE` contracts where `issuer_server_id == server_id` or `contractor_server_id == server_id`.
  * `direct_offers`: return all `OPEN` offers with `remaining_liters > 0`.
  * `action_results`: filter to results belonging to `server_id` within a recent window (e.g., last 10 minutes), storing them idempotently per `action_id`.
  * `settlements`: return all **unacknowledged** settlements where `target_server_id == server_id`. Keep emitting them until the client sends `ACK_SETTLEMENT`! (Lua deduplicates `settlement_id` persistently in `globalMarket.xml` across reloads).

### 3.3 Action Endpoint (`POST /api/v1/contracts/actions`)
Idempotent per `action_id` (repeating an already-processed `action_id` returns the exact stored `ActionResult` inside `action_results` without mutating state twice!).

* **Request JSON (`snake_case`):**
  ```json
  {
    "sub_action": "CREATE_CONTRACT",
    "action_id": "a3f901c48b2e7d10_12000_1",
    "server_id": "a3f901c48b2e7d10",
    "farm_id": 1,
    "farm_name": "Hof Meier [Dedi EU]",
    "remote_id": "c_101",
    "fill_type": "WHEAT",
    "liters": 50000,
    "base_trend_factor": 1.0,
    "bonus_pct": 12,
    "price_delta_pct": -5,
    "allow_partial": true,
    "duration_hours": 48,
    "was_bale": false,
    "was_pallet": false,
    "settlement_ids": "set_901,set_902"
  }
  ```
  *(Optional fields are omitted when unset.)*
* **Response `200 OK`:** Returns the updated `ContractStateResponse` (same schema as `GET /api/v1/contracts/state?server_id=<server_id>`, including the immediate `ActionResult` inside `action_results`), matching how `POST /api/v1/market/transactions` returns `MarketState`. (`GlobalMarketClient` also gracefully handles bare `ActionResult` responses by refreshing `/contracts/state` automatically.)

| `sub_action` | Backend Validation & Atomic DB Transaction | `ActionResult` & Emitted `Settlement`s |
| :--- | :--- | :--- |
| `CREATE_CONTRACT` | Verify `fill_type` is `APPROVED`, `liters >= 1000`, `5 <= bonus_pct <= 20`, `1 <= duration_hours <= 168`. Insert `OPEN` contract (`expires_at = now + duration_hours`). | `ACCEPTED` (`remote_id`, `remaining_seconds`) or `REJECTED` (e.g., unapproved commodity / spam cap). |
| `CANCEL_CONTRACT` | `UPDATE ... WHERE remote_id = ? AND issuer_server_id = ? AND issuer_farm_id = ? AND status = 'OPEN'` -> delete/mark `CANCELLED`. | `ACCEPTED` if row was still `OPEN`; `REJECTED` if already `ACTIVE` / expired. |
| `ACCEPT_CONTRACT` | **Atomic conditional update:** `UPDATE ... WHERE remote_id = ? AND status = 'OPEN' AND (issuer_server_id != ? OR issuer_farm_id != ?)` -> set `status = 'ACTIVE'`, `contractor_server_id`, `contractor_farm_id`. | Winner gets `ACCEPTED` (`base_trend_factor`); concurrent losers get `REJECTED`. |
| `DELIVER_CONTRACT` | Verify `status = 'ACTIVE'` and `(contractor_server_id, contractor_farm_id)` matches sender. Increment `delivered_liters = min(total_liters, delivered_liters + liters)`. If `delivered_liters >= total_liters`, mark `COMPLETED`. | Emit `CONTRACT_DELIVERY` (`liters`) to `(issuer_server_id, issuer_farm_id)`! If completed, also emit `CONTRACT_COMPLETED_ISSUER` to issuer. Return `ACCEPTED`. |
| `CREATE_OFFER` | Verify `fill_type` is `APPROVED`, `liters > 0`, `-10 <= price_delta_pct <= 10`, `1 <= duration_hours <= 72`. Insert `OPEN` offer. | `ACCEPTED` (`remote_id`, `remaining_seconds`) or `REJECTED`. |
| `BUY_OFFER` | **Row-locked atomic update:** `SELECT ... FOR UPDATE`. Verify `status = 'OPEN'`, not self-purchase, `remaining_liters > 0`. If `allow_partial == false`, require `remaining_liters >= liters` (else `REJECTED`); if `allow_partial == true`, grant `min(liters, remaining_liters)`. Deduct `granted_liters`; mark `SOLD_OUT` when `remaining_liters == 0`. | Return `ACCEPTED` (`granted_liters`) or `REJECTED`. On `ACCEPTED`, emit `OFFER_SOLD` (`liters = granted_liters`, `price_delta_pct`) to `(seller_server_id, seller_farm_id)`! |
| `CANCEL_OFFER` | `SELECT ... FOR UPDATE` verify owner & `remaining_liters > 0`, set `status = 'CANCELLED'`. | `ACCEPTED` (`granted_liters = remaining_liters` actually returned to the seller at cancel time) or `REJECTED` if already sold out. |
| `ACK_SETTLEMENT` | Mark all `settlement_id` values in `settlement_ids.split(",")` belonging to `server_id` as acknowledged (`acked_at = now`). | `ACCEPTED` (no XML `<result>` needed by Lua, or benign `ACCEPTED`). |
| `REQUEST_SYNC` | No-op handshake ping; returns current `ContractStateResponse` immediately so Client writes `gmContractSync_*.xml`. | Current `contracts`, `direct_offers`, `action_results`, `settlements`. |

### 3.4 Background Expiration Worker (Cron / Loop on `GlobalMarketServer`)
Every ~30–60 seconds, scan expired contracts & offers where `expires_at <= now()`:
1. **Expired `OPEN` Contract:** Mark `EXPIRED`, emit `CONTRACT_EXPIRED_OPEN` (`target_server_id = issuer_server_id`, `target_farm_id = issuer_farm_id`, `remote_id`, `fill_type`, `liters = total_liters`, `base_trend_factor`).
2. **Expired `ACTIVE` Contract (`delivered_liters < total_liters`):** Mark `DEFAULTED`, `missing = total_liters - delivered_liters`:
   * Emit `CONTRACT_PENALTY_DEBIT` (`target_server_id = contractor_server_id`, `target_farm_id = contractor_farm_id`, `missing_liters = missing`, `base_trend_factor`).
   * Emit `CONTRACT_DEFAULT_REFUND` (`target_server_id = issuer_server_id`, `target_farm_id = issuer_farm_id`, `missing_liters = missing`, `base_trend_factor`).
3. **Expired `OPEN` Direct Offer (`remaining_liters > 0`):** Mark `EXPIRED`, emit `OFFER_RETURNED` (`target_server_id = seller_server_id`, `target_farm_id = seller_farm_id`, `liters = remaining_liters`).
