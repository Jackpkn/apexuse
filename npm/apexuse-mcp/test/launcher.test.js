const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const { EventEmitter } = require("node:events");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const {
  VERSION,
  resolveTarget,
  buildReleaseUrls,
  parseSha256,
  stripWrapperFlags,
  resolveCacheBinaryPath,
  defaultPathLookup,
  downloadBuffer,
  resolveBinary,
} = require("../bin/apexuse-mcp.js");

function makeTempDir(prefix = "apexuse-launcher-test-") {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function sha256Hex(buf) {
  return crypto.createHash("sha256").update(buf).digest("hex");
}

test("resolveTarget maps supported OS/arch pairs and rejects unsupported ones", () => {
  assert.deepEqual(resolveTarget("darwin", "arm64"), {
    target: "macos-arm64",
    archive: "tar.gz",
    binName: "apexuse-mcp",
  });
  assert.deepEqual(resolveTarget("darwin", "x64"), {
    target: "macos-x86_64",
    archive: "tar.gz",
    binName: "apexuse-mcp",
  });
  assert.deepEqual(resolveTarget("linux", "x64"), {
    target: "linux-x86_64",
    archive: "tar.gz",
    binName: "apexuse-mcp",
  });
  assert.deepEqual(resolveTarget("win32", "x64"), {
    target: "windows-x86_64",
    archive: "zip",
    binName: "apexuse-mcp.exe",
  });

  assert.throws(
    () => resolveTarget("linux", "arm64"),
    /unsupported platform\/architecture: linux\/arm64/i
  );
  assert.throws(
    () => resolveTarget("freebsd", "x64"),
    /unsupported platform\/architecture: freebsd\/x64/i
  );
});

test("buildReleaseUrls matches GitHub Release workflow asset names", () => {
  const macX64 = buildReleaseUrls(resolveTarget("darwin", "x64"), "0.3.0");
  assert.equal(macX64.assetName, "apexuse-mcp-v0.3.0-macos-x86_64.tar.gz");
  assert.equal(macX64.sha256AssetName, "apexuse-mcp-v0.3.0-macos-x86_64.tar.gz.sha256");
  assert.equal(
    macX64.assetUrl,
    "https://github.com/Jackpkn/apexuse/releases/download/v0.3.0/apexuse-mcp-v0.3.0-macos-x86_64.tar.gz"
  );
  assert.equal(
    macX64.sha256Url,
    "https://github.com/Jackpkn/apexuse/releases/download/v0.3.0/apexuse-mcp-v0.3.0-macos-x86_64.tar.gz.sha256"
  );

  const winX64 = buildReleaseUrls(resolveTarget("win32", "x64"), "0.3.0");
  assert.equal(winX64.assetName, "apexuse-mcp-v0.3.0-windows-x86_64.zip");
  assert.equal(winX64.sha256AssetName, "apexuse-mcp-v0.3.0-windows-x86_64.zip.sha256");
});

test("stripWrapperFlags removes --use-system and preserves forwarded CLI flags", () => {
  assert.deepEqual(stripWrapperFlags(["--use-system", "doctor", "--check"]), {
    useSystem: true,
    forwardedArgs: ["doctor", "--check"],
  });
  assert.deepEqual(stripWrapperFlags(["install", "cursor", "--use-system", "--write", "--npx"]), {
    useSystem: true,
    forwardedArgs: ["install", "cursor", "--write", "--npx"],
  });
  assert.deepEqual(stripWrapperFlags(["--mock", "--version"]), {
    useSystem: false,
    forwardedArgs: ["--mock", "--version"],
  });
  assert.deepEqual(stripWrapperFlags([]), {
    useSystem: false,
    forwardedArgs: [],
  });
});

test("parseSha256 supports Unix, Windows/PowerShell, and uppercase checksums while rejecting corrupt payloads", () => {
  const digest = "a".repeat(64);
  const upperDigest = "ABCDEF0123456789".repeat(4);

  assert.equal(
    parseSha256(`${digest}  dist/apexuse-mcp-v0.3.0-macos-arm64.tar.gz\n`),
    digest
  );
  assert.equal(
    parseSha256(`${upperDigest} *dist/apexuse-mcp-v0.3.0-windows-x86_64.zip\r\n`),
    upperDigest.toLowerCase()
  );
  assert.equal(parseSha256(`  ${upperDigest}  `), upperDigest.toLowerCase());
  assert.equal(
    parseSha256(
      `Algorithm : SHA256\r\nHash      : ${upperDigest}\r\nPath      : C:\\temp\\apexuse.zip\r\n`
    ),
    upperDigest.toLowerCase()
  );
  assert.equal(
    parseSha256(`SHA256          ${upperDigest}  C:\\temp\\apexuse.zip\n`),
    upperDigest.toLowerCase()
  );

  assert.throws(() => parseSha256(""), /empty/i);
  assert.throws(() => parseSha256("   \n\t "), /empty/i);
  assert.throws(() => parseSha256("a".repeat(63) + "  file.tar.gz"), /invalid|corrupted/i);
  assert.throws(() => parseSha256("a".repeat(65) + "  file.tar.gz"), /invalid|corrupted/i);
  assert.throws(() => parseSha256("z".repeat(64) + "  file.tar.gz"), /invalid|corrupted/i);
  assert.throws(() => parseSha256("<html>404 Not Found</html>"), /invalid|corrupted/i);
});

test("resolveCacheBinaryPath honors APEXUSE_STATE_DIR, HOME, and USERPROFILE", () => {
  const targetMac = resolveTarget("darwin", "arm64");
  const targetWin = resolveTarget("win32", "x64");

  assert.equal(
    resolveCacheBinaryPath({ APEXUSE_STATE_DIR: "/tmp/custom-state", HOME: "/tmp/h" }, targetMac, "0.3.0", "darwin"),
    path.join("/tmp/custom-state", "bin", "v0.3.0", "apexuse-mcp")
  );
  assert.equal(
    resolveCacheBinaryPath({ HOME: "/tmp/user-home" }, targetMac, "0.3.0", "darwin"),
    path.join("/tmp/user-home", ".apexuse", "bin", "v0.3.0", "apexuse-mcp")
  );
  assert.equal(
    resolveCacheBinaryPath({ USERPROFILE: "C:\\Users\\alice" }, targetWin, "0.3.0", "win32"),
    path.join("C:\\Users\\alice", ".apexuse", "bin", "v0.3.0", "apexuse-mcp.exe")
  );
});

test("resolveBinary prioritizes APEXUSE_BIN and rejects missing APEXUSE_BIN", async () => {
  const tmpDir = makeTempDir();
  try {
    const customBin = path.join(tmpDir, "custom-apexuse-mcp");
    fs.writeFileSync(customBin, "#!/bin/sh\necho ok\n");
    fs.chmodSync(customBin, 0o755);

    let lookupCalled = false;
    let downloadCalled = false;
    const res = await resolveBinary({
      env: { APEXUSE_BIN: customBin, HOME: tmpDir },
      useSystem: true,
      platform: "darwin",
      arch: "arm64",
      pathLookup: () => {
        lookupCalled = true;
        return "/usr/local/bin/apexuse-mcp";
      },
      downloader: async () => {
        downloadCalled = true;
        throw new Error("should not download");
      },
      log: () => {},
    });

    assert.equal(res.path, customBin);
    assert.equal(res.source, "env");
    assert.equal(lookupCalled, false);
    assert.equal(downloadCalled, false);

    await assert.rejects(
      () =>
        resolveBinary({
          env: { APEXUSE_BIN: path.join(tmpDir, "missing-bin"), HOME: tmpDir },
          platform: "darwin",
          arch: "arm64",
          downloader: async () => {
            throw new Error("should not download");
          },
          log: () => {},
        }),
      /APEXUSE_BIN/
    );
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("resolveBinary uses cached version binary before PATH or network", async () => {
  const tmpDir = makeTempDir();
  try {
    const target = resolveTarget("darwin", "arm64");
    const cachedBin = resolveCacheBinaryPath({ HOME: tmpDir }, target, VERSION, "darwin");
    fs.mkdirSync(path.dirname(cachedBin), { recursive: true });
    fs.writeFileSync(cachedBin, "cached-binary");
    fs.chmodSync(cachedBin, 0o755);

    let lookupCalled = false;
    let downloadCalled = false;
    const res = await resolveBinary({
      env: { HOME: tmpDir },
      useSystem: true,
      platform: "darwin",
      arch: "arm64",
      pathLookup: () => {
        lookupCalled = true;
        return "/usr/local/bin/apexuse-mcp";
      },
      downloader: async () => {
        downloadCalled = true;
        throw new Error("should not download");
      },
      log: () => {},
    });

    assert.equal(res.path, cachedBin);
    assert.equal(res.source, "cache");
    assert.equal(lookupCalled, false);
    assert.equal(downloadCalled, false);
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("resolveBinary looks up PATH only when useSystem is true", async () => {
  const tmpDir = makeTempDir();
  try {
    const sysBin = path.join(tmpDir, "sys-apexuse-mcp");
    fs.writeFileSync(sysBin, "system-binary");
    fs.chmodSync(sysBin, 0o755);

    let lookupCalls = 0;
    const resSys = await resolveBinary({
      env: { HOME: tmpDir, PATH: tmpDir },
      useSystem: true,
      platform: "linux",
      arch: "x64",
      pathLookup: () => {
        lookupCalls += 1;
        return sysBin;
      },
      downloader: async () => {
        throw new Error("should not download when system binary is found");
      },
      log: () => {},
    });
    assert.equal(resSys.path, sysBin);
    assert.equal(resSys.source, "system");
    assert.equal(lookupCalls, 1);

    lookupCalls = 0;
    await assert.rejects(
      () =>
        resolveBinary({
          env: { HOME: tmpDir, PATH: tmpDir },
          useSystem: false,
          platform: "linux",
          arch: "x64",
          pathLookup: () => {
            lookupCalls += 1;
            return sysBin;
          },
          downloader: async () => {
            throw new Error("network blocked in test");
          },
          log: () => {},
        }),
      /network blocked in test/
    );
    assert.equal(lookupCalls, 0);
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("resolveBinary rejects SHA-256 mismatch before extraction and leaves cache clean", async () => {
  const tmpDir = makeTempDir();
  try {
    const target = resolveTarget("darwin", "x64");
    const cachedBin = resolveCacheBinaryPath({ HOME: tmpDir }, target, VERSION, "darwin");
    const fakeArchive = Buffer.from("corrupted-or-tampered-tarball");
    const wrongSha = "0".repeat(64);
    let extractorCalled = false;
    const logs = [];

    await assert.rejects(
      () =>
        resolveBinary({
          env: { HOME: tmpDir },
          platform: "darwin",
          arch: "x64",
          downloader: async (url) => {
            if (url.endsWith(".sha256")) {
              return Buffer.from(`${wrongSha}  apexuse-mcp-v0.3.0-macos-x86_64.tar.gz\n`);
            }
            return fakeArchive;
          },
          extractor: () => {
            extractorCalled = true;
          },
          log: (msg) => logs.push(msg),
        }),
      /SHA-256 mismatch/i
    );

    assert.equal(extractorCalled, false);
    assert.equal(fs.existsSync(cachedBin), false);
    assert.ok(logs.length > 0);
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("resolveBinary verifies SHA-256, extracts tar.gz, atomically caches binary, and sets 0o755", async () => {
  const tmpDir = makeTempDir();
  try {
    // Build a real .tar.gz matching .github/workflows/release.yml layout (-C pkg .)
    const pkgDir = path.join(tmpDir, "pkg");
    fs.mkdirSync(pkgDir, { recursive: true });
    const fakeBinContent = "#!/bin/sh\necho apexuse-mcp 0.3.0\n";
    fs.writeFileSync(path.join(pkgDir, "apexuse-mcp"), fakeBinContent);
    fs.chmodSync(path.join(pkgDir, "apexuse-mcp"), 0o644);
    fs.writeFileSync(path.join(pkgDir, "README.md"), "readme\n");
    fs.writeFileSync(path.join(pkgDir, "LICENSE"), "MIT\n");

    const tarPath = path.join(tmpDir, "test.tar.gz");
    execFileSync("tar", ["-czf", tarPath, "-C", pkgDir, "."]);
    const archiveBuf = fs.readFileSync(tarPath);
    const digest = sha256Hex(archiveBuf);

    const stateDir = path.join(tmpDir, "state");
    const logs = [];
    let downloadCalls = 0;

    const first = await resolveBinary({
      env: { APEXUSE_STATE_DIR: stateDir },
      platform: "darwin",
      arch: "arm64",
      downloader: async (url) => {
        downloadCalls += 1;
        if (url.endsWith(".sha256")) {
          return Buffer.from(`${digest}  dist/apexuse-mcp-v0.3.0-macos-arm64.tar.gz\n`);
        }
        return archiveBuf;
      },
      log: (msg) => logs.push(msg),
    });

    assert.equal(first.source, "download");
    assert.equal(downloadCalls, 2);
    assert.equal(fs.readFileSync(first.path, "utf8"), fakeBinContent);
    const mode = fs.statSync(first.path).mode & 0o777;
    assert.equal(mode, 0o755);
    assert.ok(logs.some((l) => l.includes("Downloading")));

    // Second invocation must hit cache without downloading again
    const second = await resolveBinary({
      env: { APEXUSE_STATE_DIR: stateDir },
      platform: "darwin",
      arch: "arm64",
      downloader: async () => {
        throw new Error("cache should have been used");
      },
      log: () => {},
    });
    assert.equal(second.path, first.path);
    assert.equal(second.source, "cache");
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});

test("downloadBuffer follows redirects and rejects redirect loops and HTTP errors", async () => {
  function mockRequester(routes) {
    return (urlStr, _opts, cb) => {
      const req = new EventEmitter();
      req.setTimeout = () => {};
      req.destroy = () => {};
      req.end = () => {
        const spec = routes[urlStr];
        if (!spec) {
          req.emit("error", new Error(`Unexpected URL: ${urlStr}`));
          return;
        }
        const res = new EventEmitter();
        res.statusCode = spec.status;
        res.headers = spec.headers || {};
        res.resume = () => {};
        cb(res);
        process.nextTick(() => {
          if (spec.body !== undefined) {
            res.emit("data", Buffer.from(spec.body));
          }
          res.emit("end");
        });
      };
      return req;
    };
  }

  const okBuf = await downloadBuffer("https://github.com/start", {
    requestImpl: mockRequester({
      "https://github.com/start": {
        status: 302,
        headers: { location: "https://objects.githubusercontent.com/final" },
      },
      "https://objects.githubusercontent.com/final": {
        status: 200,
        body: "payload-ok",
      },
    }),
  });
  assert.equal(okBuf.toString("utf8"), "payload-ok");

  await assert.rejects(
    () =>
      downloadBuffer("https://github.com/missing", {
        requestImpl: mockRequester({
          "https://github.com/missing": { status: 404, body: "Not Found" },
        }),
      }),
    /HTTP 404/
  );

  await assert.rejects(
    () =>
      downloadBuffer("https://github.com/loop1", {
        maxRedirects: 2,
        requestImpl: mockRequester({
          "https://github.com/loop1": { status: 302, headers: { location: "/loop2" } },
          "https://github.com/loop2": { status: 302, headers: { location: "/loop3" } },
          "https://github.com/loop3": { status: 302, headers: { location: "/loop1" } },
        }),
      }),
    /Too many redirects/i
  );
});

test("defaultPathLookup skips symlink to launcher itself and resolveBinary fails cleanly if archive lacks binary", async () => {
  const tmpDir = makeTempDir();
  try {
    const dir1 = path.join(tmpDir, "bin1");
    const dir2 = path.join(tmpDir, "bin2");
    fs.mkdirSync(dir1, { recursive: true });
    fs.mkdirSync(dir2, { recursive: true });

    const selfLink = path.join(dir1, "apexuse-mcp");
    const launcherJs = path.resolve(__dirname, "../bin/apexuse-mcp.js");
    try {
      fs.symlinkSync(launcherJs, selfLink);
    } catch {
      // Ignore symlink permission error on restricted Windows hosts
    }

    const realBin = path.join(dir2, "apexuse-mcp");
    fs.writeFileSync(realBin, "#!/bin/sh\necho real\n");
    fs.chmodSync(realBin, 0o755);

    const found = defaultPathLookup("apexuse-mcp", { PATH: `${dir1}${path.delimiter}${dir2}` }, "darwin");
    assert.equal(found, realBin);

    const pkgDir = path.join(tmpDir, "empty-pkg");
    fs.mkdirSync(pkgDir, { recursive: true });
    fs.writeFileSync(path.join(pkgDir, "README.md"), "only readme\n");
    const tarPath = path.join(tmpDir, "missing-bin.tar.gz");
    execFileSync("tar", ["-czf", tarPath, "-C", pkgDir, "."]);
    const archiveBuf = fs.readFileSync(tarPath);
    const digest = sha256Hex(archiveBuf);
    const stateDir = path.join(tmpDir, "state");

    await assert.rejects(
      () =>
        resolveBinary({
          env: { APEXUSE_STATE_DIR: stateDir },
          platform: "darwin",
          arch: "arm64",
          downloader: async (url) => (url.endsWith(".sha256") ? Buffer.from(digest) : archiveBuf),
          log: () => {},
        }),
      /did not contain apexuse-mcp/i
    );
    const cachePath = resolveCacheBinaryPath({ APEXUSE_STATE_DIR: stateDir }, resolveTarget("darwin", "arm64"), VERSION, "darwin");
    assert.equal(fs.existsSync(cachePath), false);
  } finally {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});
