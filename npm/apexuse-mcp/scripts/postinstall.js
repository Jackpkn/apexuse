#!/usr/bin/env node
"use strict";

const PKG = require("../package.json");
const DEFAULT_VERSION = (PKG && PKG.version) || "0.3.0";

function renderPostinstallMessage({
  platform = process.platform,
  version = DEFAULT_VERSION,
} = {}) {
  const resolvedPlatform = platform || process.platform;
  const resolvedVersion = version || DEFAULT_VERSION;

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
  renderPostinstallMessage,
};
