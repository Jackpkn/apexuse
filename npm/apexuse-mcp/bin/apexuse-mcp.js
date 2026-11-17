#!/usr/bin/env node
"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const http = require("node:http");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { execFileSync, spawn } = require("node:child_process");

const PKG = require("../package.json");
const VERSION = PKG.version || "0.3.0";
const DEFAULT_REPO = "Jackpkn/apexuse";

const TARGET_MAP = {
  "darwin:arm64": { target: "macos-arm64", archive: "tar.gz", binName: "apexuse-mcp" },
  "darwin:x64": { target: "macos-x86_64", archive: "tar.gz", binName: "apexuse-mcp" },
  "linux:x64": { target: "linux-x86_64", archive: "tar.gz", binName: "apexuse-mcp" },
  "win32:x64": { target: "windows-x86_64", archive: "zip", binName: "apexuse-mcp.exe" },
};

function defaultLog(msg) {
  process.stderr.write(`[apexuse-mcp] ${msg}\n`);
}

function resolveTarget(platform = process.platform, arch = process.arch) {
  const key = `${platform}:${arch}`;
  const spec = TARGET_MAP[key];
  if (!spec) {
    throw new Error(
      `Unsupported platform/architecture: ${platform}/${arch}. ` +
        `Prebuilt binaries cover darwin/arm64, darwin/x64, linux/x64, and win32/x64. ` +
        `Build from source with: cargo install apexuse-mcp`
    );
  }
  return { ...spec };
}

function buildReleaseUrls(targetInfo, version = VERSION, repo = DEFAULT_REPO) {
  const tag = version.startsWith("v") ? version : `v${version}`;
  const base = `apexuse-mcp-${tag}-${targetInfo.target}`;
  const assetName = `${base}.${targetInfo.archive}`;
  const sha256AssetName = `${assetName}.sha256`;
  const prefix = `https://github.com/${repo}/releases/download/${tag}`;
  return {
    tag,
    base,
    assetName,
    sha256AssetName,
    assetUrl: `${prefix}/${assetName}`,
    sha256Url: `${prefix}/${sha256AssetName}`,
  };
}

function stripWrapperFlags(args = []) {
  let useSystem = false;
  let stopParsing = false;
  const forwardedArgs = [];
  for (const arg of args) {
    if (!stopParsing && arg === "--") {
      stopParsing = true;
      forwardedArgs.push(arg);
      continue;
    }
    if (!stopParsing && arg === "--use-system") {
      useSystem = true;
      continue;
    }
    forwardedArgs.push(arg);
  }
  return { useSystem, forwardedArgs };
}

function parseSha256(text) {
  if (typeof text !== "string" || !text.trim()) {
    throw new Error("Empty SHA-256 checksum payload");
  }
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  if (lines.length === 0) {
    throw new Error("Empty SHA-256 checksum payload");
  }

  if (lines.length === 1) {
    const unixMatch = lines[0].match(/^([0-9a-fA-F]{64})(?:[\s*]+.+)?$/);
    if (unixMatch) {
      return unixMatch[1].toLowerCase();
    }
    const bsdMatch = lines[0].match(/^SHA256\s*\(.+\)\s*=\s*([0-9a-fA-F]{64})$/i);
    if (bsdMatch) {
      return bsdMatch[1].toLowerCase();
    }
    const psTableMatch = lines[0].match(/^SHA256\s+([0-9a-fA-F]{64})(?:\s+.*)?$/i);
    if (psTableMatch) {
      return psTableMatch[1].toLowerCase();
    }
  } else {
    for (const line of lines) {
      const psListMatch = line.match(/^Hash\s*:\s*([0-9a-fA-F]{64})$/i);
      if (psListMatch) {
        return psListMatch[1].toLowerCase();
      }
      const psTableMatch = line.match(/^SHA256\s+([0-9a-fA-F]{64})(?:\s+.*)?$/i);
      if (psTableMatch) {
        return psTableMatch[1].toLowerCase();
      }
    }
  }

  throw new Error("Invalid or corrupted SHA-256 checksum payload");
}

function resolveStateDir(env = process.env, platform = process.platform) {
  const explicit =
    (env.APEXUSE_STATE_DIR && env.APEXUSE_STATE_DIR.trim()) ||
    (env.HANDSMCP_STATE_DIR && env.HANDSMCP_STATE_DIR.trim());
  if (explicit) {
    return explicit;
  }
  const home =
    platform === "win32"
      ? (env.USERPROFILE && env.USERPROFILE.trim()) || (env.HOME && env.HOME.trim())
      : (env.HOME && env.HOME.trim()) || (env.USERPROFILE && env.USERPROFILE.trim());
  return path.join(home || os.homedir(), ".apexuse");
}

function resolveCacheBinaryPath(
  env = process.env,
  targetInfo = resolveTarget(),
  version = VERSION,
  platform = process.platform
) {
  const tag = version.startsWith("v") ? version : `v${version}`;
  return path.join(resolveStateDir(env, platform), "bin", tag, targetInfo.binName);
}

function isAccessibleFile(filePath, requireExecutable, platform) {
  try {
    const st = fs.statSync(filePath);
    if (!st.isFile()) {
      return false;
    }
    const mode =
      requireExecutable && platform !== "win32"
        ? fs.constants.R_OK | fs.constants.X_OK
        : fs.constants.R_OK;
    fs.accessSync(filePath, mode);
    return true;
  } catch {
    return false;
  }
}

function defaultPathLookup(binName, env = process.env, platform = process.platform) {
  const pathVal = env.PATH || env.Path || "";
  if (!pathVal) {
    return null;
  }
  let selfRealPath = null;
  try {
    selfRealPath = fs.realpathSync(__filename);
  } catch {
    selfRealPath = null;
  }

  const sep = platform === "win32" ? ";" : ":";
  for (const dir of pathVal.split(sep)) {
    if (!dir) {
      continue;
    }
    const candidate = path.join(dir, binName);
    if (!isAccessibleFile(candidate, true, platform)) {
      continue;
    }
    if (selfRealPath) {
      try {
        if (fs.realpathSync(candidate) === selfRealPath) {
          continue;
        }
      } catch {
        continue;
      }
    }
    return candidate;
  }
  return null;
}

function downloadBuffer(url, options = {}) {
  const {
    maxRedirects = 5,
    timeoutMs = 30000,
    requestImpl = null,
    redirects = 0,
  } = options;

  return new Promise((resolve, reject) => {
    let parsed;
    try {
      parsed = new URL(url);
    } catch (err) {
      reject(new Error(`Invalid URL '${url}': ${err.message}`));
      return;
    }

    let reqFn = requestImpl;
    if (!reqFn) {
      if (parsed.protocol === "https:") {
        reqFn = https.request;
      } else if (parsed.protocol === "http:") {
        reqFn = http.request;
      } else {
        reject(new Error(`Unsupported URL protocol '${parsed.protocol}' for ${url}`));
        return;
      }
    }

    let settled = false;
    const fail = (err) => {
      if (!settled) {
        settled = true;
        reject(err);
      }
    };

    const req = reqFn(
      url,
      {
        method: "GET",
        headers: {
          "User-Agent": `apexuse-mcp/${VERSION}`,
          Accept: "application/octet-stream, text/plain, */*",
        },
      },
      (res) => {
        const status = res.statusCode || 0;
        if ([301, 302, 303, 307, 308].includes(status)) {
          const location = res.headers && res.headers.location;
          if (typeof res.resume === "function") {
            res.resume();
          }
          if (!location) {
            fail(new Error(`HTTP ${status} redirect missing Location header for ${url}`));
            return;
          }
          if (redirects >= maxRedirects) {
            fail(new Error(`Too many redirects (>${maxRedirects}) fetching ${url}`));
            return;
          }
          let nextUrl;
          try {
            nextUrl = new URL(location, url).toString();
          } catch (err) {
            fail(new Error(`Invalid redirect location '${location}': ${err.message}`));
            return;
          }
          settled = true;
          downloadBuffer(nextUrl, {
            maxRedirects,
            timeoutMs,
            requestImpl,
            redirects: redirects + 1,
          }).then(resolve, reject);
          return;
        }

        if (status !== 200) {
          if (typeof res.resume === "function") {
            res.resume();
          }
          fail(new Error(`HTTP ${status} fetching ${url}`));
          return;
        }

        const chunks = [];
        res.on("data", (chunk) => chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)));
        res.on("error", (err) => fail(err));
        res.on("end", () => {
          if (!settled) {
            settled = true;
            resolve(Buffer.concat(chunks));
          }
        });
      }
    );

    if (typeof req.setTimeout === "function") {
      req.setTimeout(timeoutMs, () => {
        if (typeof req.destroy === "function") {
          req.destroy();
        }
        fail(new Error(`Timed out after ${timeoutMs}ms fetching ${url}`));
      });
    }
    req.on("error", (err) => fail(err));
    req.end();
  });
}

function defaultExtractor(archivePath, extractDir, targetInfo) {
  if (targetInfo.archive === "tar.gz") {
    execFileSync("tar", ["-xzf", archivePath, "-C", extractDir], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    return;
  }
  if (targetInfo.archive === "zip") {
    execFileSync(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Expand-Archive -Force -Path $env:APEXUSE_ARCHIVE -DestinationPath $env:APEXUSE_DEST",
      ],
      {
        env: {
          ...process.env,
          APEXUSE_ARCHIVE: archivePath,
          APEXUSE_DEST: extractDir,
        },
        stdio: ["ignore", "pipe", "pipe"],
      }
    );
    return;
  }
  throw new Error(`Unsupported archive format: ${targetInfo.archive}`);
}

function findExtractedBinary(dir, binName, maxDepth = 3) {
  const direct = path.join(dir, binName);
  try {
    if (fs.statSync(direct).isFile()) {
      return direct;
    }
  } catch {
    // Fall through to breadth-first scan
  }

  const queue = [{ dir, depth: 0 }];
  while (queue.length > 0) {
    const current = queue.shift();
    if (current.depth > maxDepth) {
      continue;
    }
    let entries;
    try {
      entries = fs.readdirSync(current.dir, { withFileTypes: true });
    } catch {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(current.dir, entry.name);
      if (entry.isFile() && entry.name === binName) {
        return full;
      }
      if (entry.isDirectory() && current.depth < maxDepth) {
        queue.push({ dir: full, depth: current.depth + 1 });
      }
    }
  }
  return null;
}

async function resolveBinary({
  env = process.env,
  useSystem = false,
  platform = process.platform,
  arch = process.arch,
  version = VERSION,
  repo = DEFAULT_REPO,
  pathLookup = defaultPathLookup,
  downloader = downloadBuffer,
  extractor = defaultExtractor,
  log = defaultLog,
} = {}) {
  const targetInfo = resolveTarget(platform, arch);

  const envBin = env.APEXUSE_BIN && env.APEXUSE_BIN.trim();
  if (envBin) {
    if (!isAccessibleFile(envBin, false, platform)) {
      throw new Error(`APEXUSE_BIN does not point to a readable file: ${envBin}`);
    }
    return { path: envBin, source: "env", target: targetInfo };
  }

  const cacheBinPath = resolveCacheBinaryPath(env, targetInfo, version, platform);
  if (isAccessibleFile(cacheBinPath, true, platform)) {
    return { path: cacheBinPath, source: "cache", target: targetInfo };
  }

  if (useSystem) {
    const sysPath = pathLookup(targetInfo.binName, env, platform);
    if (sysPath) {
      return { path: sysPath, source: "system", target: targetInfo };
    }
  }

  const urls = buildReleaseUrls(targetInfo, version, repo);
  log(`Downloading ${urls.assetUrl} ...`);
  const shaBuf = await downloader(urls.sha256Url);
  const expectedSha = parseSha256(shaBuf.toString("utf8"));
  const archiveBuf = await downloader(urls.assetUrl);

  const actualSha = crypto.createHash("sha256").update(archiveBuf).digest("hex");
  if (actualSha !== expectedSha) {
    throw new Error(
      `SHA-256 mismatch for ${urls.assetName}: expected ${expectedSha}, got ${actualSha}`
    );
  }
  log(`Verified SHA-256 (${actualSha.slice(0, 12)}...)`);

  const cacheDir = path.dirname(cacheBinPath);
  fs.mkdirSync(cacheDir, { recursive: true });
  const tmpDir = fs.mkdtempSync(path.join(cacheDir, ".dl-"));
  const stagedPath = path.join(
    cacheDir,
    `.${targetInfo.binName}.tmp.${process.pid}.${crypto.randomBytes(4).toString("hex")}`
  );

  try {
    const archivePath = path.join(tmpDir, urls.assetName);
    const extractDir = path.join(tmpDir, "extract");
    fs.mkdirSync(extractDir, { recursive: true });
    fs.writeFileSync(archivePath, archiveBuf);

    await extractor(archivePath, extractDir, targetInfo);

    const extractedBin = findExtractedBinary(extractDir, targetInfo.binName);
    if (!extractedBin) {
      throw new Error(
        `Extracted archive ${urls.assetName} did not contain ${targetInfo.binName}`
      );
    }

    fs.renameSync(extractedBin, stagedPath);
    if (platform !== "win32") {
      fs.chmodSync(stagedPath, 0o755);
    }
    fs.renameSync(stagedPath, cacheBinPath);
    if (platform !== "win32") {
      fs.chmodSync(cacheBinPath, 0o755);
    }
  } catch (err) {
    try {
      fs.rmSync(stagedPath, { force: true });
    } catch {
      // Ignore cleanup errors
    }
    throw err;
  } finally {
    try {
      fs.rmSync(tmpDir, { recursive: true, force: true });
    } catch {
      // Ignore cleanup errors
    }
  }

  if (!isAccessibleFile(cacheBinPath, true, platform)) {
    throw new Error(`Cached binary missing or not executable after install: ${cacheBinPath}`);
  }
  log(`Cached binary at ${cacheBinPath}`);

  return { path: cacheBinPath, source: "download", target: targetInfo };
}

async function main() {
  const { useSystem, forwardedArgs } = stripWrapperFlags(process.argv.slice(2));
  let resolved;
  try {
    resolved = await resolveBinary({
      env: process.env,
      useSystem,
      platform: process.platform,
      arch: process.arch,
    });
  } catch (err) {
    defaultLog(`Error: ${err && err.message ? err.message : String(err)}`);
    process.exit(1);
    return;
  }

  const child = spawn(resolved.path, forwardedArgs, { stdio: "inherit" });

  const forwardSignal = (sig) => {
    if (!child.killed) {
      try {
        child.kill(sig);
      } catch {
        // Child may already have exited
      }
    }
  };

  process.on("SIGINT", () => forwardSignal("SIGINT"));
  process.on("SIGTERM", () => forwardSignal("SIGTERM"));

  child.on("error", (err) => {
    defaultLog(`Failed to launch ${resolved.path}: ${err.message}`);
    process.exit(1);
  });

  child.on("exit", (code, signal) => {
    if (code !== null) {
      process.exit(code);
    } else if (signal) {
      process.exit(1);
    } else {
      process.exit(0);
    }
  });
}

if (require.main === module) {
  main();
}

module.exports = {
  VERSION,
  DEFAULT_REPO,
  TARGET_MAP,
  resolveTarget,
  buildReleaseUrls,
  stripWrapperFlags,
  parseSha256,
  resolveStateDir,
  resolveCacheBinaryPath,
  defaultPathLookup,
  downloadBuffer,
  defaultExtractor,
  findExtractedBinary,
  resolveBinary,
};
