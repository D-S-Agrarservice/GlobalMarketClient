import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * Loads key=value pairs from a local `.env` file into `process.env`
 * without overriding variables already present in the environment.
 * Zero external dependencies.
 */
export function loadDotEnv(filePath = ".env") {
  const fullPath = resolve(process.cwd(), filePath);
  if (!existsSync(fullPath)) {
    return false;
  }

  const raw = readFileSync(fullPath, "utf8");
  for (const rawLine of raw.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line || line.startsWith("#")) continue;

    const eqIndex = line.indexOf("=");
    if (eqIndex <= 0) continue;

    const key = line.slice(0, eqIndex).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;

    let value = line.slice(eqIndex + 1).trim();
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1);
    }

    if (process.env[key] === undefined || process.env[key] === "") {
      process.env[key] = value;
    }
  }

  return true;
}
