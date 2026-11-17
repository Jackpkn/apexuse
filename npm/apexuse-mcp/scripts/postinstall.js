#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const FALLBACK_VERSION = "0.3.0";

function readPackageVersion(pkgPath = path.join(__dirname, "..", "package.json")) {
  try {
    const raw = fs.readFileSync(pkgPath, "utf8");
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed.version === "string" && parsed.version.trim()) {
      return parsed.version.trim();
    }
  } catch {
    // Fall back cleanly if package.json is missing, unreadable, or malformed.
  }
  return FALLBACK_VERSION;
}

function renderPostinstallMessage({
  platform = process.platform,
  version,
  pkgPath,
} = {}) {
  const resolvedPlatform = platform || process.platform;
  const resolvedVersion =
    (typeof version === "string" && version.trim()) ||
    readPackageVersion(pkgPath);

  const lines = [
    `Installed apexuse-mcp@${resolvedVersion}`,
    "",
    "Quick setup:",
    "  1. Auto-wire detected AI coding agents & verify MCP handshake:",
    "     npx -y apexuse-mcp install all --write --npx --with-skill --probe",
    "",
    "     Codex Desktop App plugin:",
    "     npx -y apexuse-mcp install codex-plugin --write --npx --probe",
    "",
  ];

  if (resolvedPlatform === "darwin") {
    lines.push(
      "  2. Grant macOS permissions & verify readiness:",
      "     npx -y apexuse-mcp doctor --onboard",
      "     npx -y apexuse-mcp doctor --check",
      ""
    );
  } else {
    lines.push(
      "  2. Verify OS readiness:",
      "     npx -y apexuse-mcp doctor --check",
      ""
    );
  }

  lines.push(
    "  3. Or paste into mcpServers:",
    '     {"mcpServers":{"apexuse":{"command":"npx","args":["-y","apexuse-mcp"]}}}',
    ""
  );

  return lines.join("\n");
}

if (require.main === module) {
  try {
    process.stdout.write(renderPostinstallMessage());
  } catch {
    // Never fail npm install if postinstall banner output fails.
  }
}

module.exports = {
  FALLBACK_VERSION,
  readPackageVersion,
  renderPostinstallMessage,
};
