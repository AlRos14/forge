#!/usr/bin/env node
"use strict";

const childProcess = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const process = require("node:process");

const PACKAGE = require("../package.json");

const REPO = "ForgeAILab/forge";
const CACHE_ROOT = path.join(os.homedir(), ".forge", "npx");
const USER_AGENT = `${PACKAGE.name}/${PACKAGE.version}`;
const SERVER_STATE_FILE = "server.json";

function usage() {
  const version = PACKAGE.version;
  console.log(`Forge npm bootstrapper ${version}

Usage:
  npx ${PACKAGE.name} [forge-options]
  npx ${PACKAGE.name} ctl [forge-ctl-options]
  npx -p ${PACKAGE.name} forge-ctl [forge-ctl-options]

Examples:
  npx ${PACKAGE.name} --demo
  npx ${PACKAGE.name} --open
  npx ${PACKAGE.name} ctl project list

Options handled by the bootstrapper:
  --open                    Open the web UI after starting forge
  --no-open                 Accepted for compatibility; opening is off by default
  --release <tag|latest>    Download a specific GitHub release tag
  --version                 Show the npm bootstrapper version
  --help                    Show this help

All other options are passed through to the Forge binary.`);
}

function isHelp(args) {
  return args.includes("--help") || args.includes("-h");
}

function isVersion(args) {
  return args.includes("--version") || args.includes("-V");
}

function isCtlInvocation(args) {
  return (
    path.basename(process.argv[1] || "") === "forge-ctl" ||
    args[0] === "ctl" ||
    args[0] === "forge-ctl"
  );
}

function parseArgs(argv) {
  const args = [...argv];
  let command = path.basename(process.argv[1] || "") === "forge-ctl" ? "forge-ctl" : "forge";
  let openBrowser = false;
  let release =
    process.env.FORGE_NPX_TAG ||
    (PACKAGE.version === "0.0.0" ? "latest" : `v${PACKAGE.version}`);
  const passthrough = [];

  if (args[0] === "ctl" || args[0] === "forge-ctl") {
    command = "forge-ctl";
    openBrowser = false;
    args.shift();
  }

  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === "--no-open") {
      openBrowser = false;
      continue;
    }
    if (arg === "--open") {
      openBrowser = command === "forge";
      continue;
    }
    if (arg === "--release") {
      const value = args[i + 1];
      if (!value) {
        throw new Error("--release requires a tag value");
      }
      release = value;
      i += 1;
      continue;
    }
    if (arg.startsWith("--release=")) {
      release = arg.slice("--release=".length);
      continue;
    }
    passthrough.push(arg);
  }

  return { command, openBrowser, passthrough, release };
}

function platformInfo(platform = process.platform, archInput = process.arch) {
  if (platform !== "darwin" && platform !== "linux") {
    throw new Error(
      `Unsupported platform ${platform}. Forge release archives currently support macOS and Linux.`
    );
  }

  let osName = platform === "darwin" ? "macos" : "linux";
  let arch = archInput;
  if (arch === "x64") arch = "x86_64";
  if (arch === "arm64") arch = "aarch64";

  if (arch !== "x86_64" && arch !== "aarch64") {
    throw new Error(`Unsupported architecture ${archInput}`);
  }

  if (platform === "linux" && linuxLibc() === "musl") {
    osName = "linux-musl";
  }

  const artifact = `forge-${arch}-${osName}`;
  return { artifact, archiveName: `${artifact}.tar.gz` };
}

function linuxLibc() {
  const override = process.env.FORGE_NPX_LIBC;
  if (override === "gnu" || override === "musl") {
    return override;
  }

  const report = process.report?.getReport?.();
  if (report?.header?.glibcVersionRuntime) {
    return "gnu";
  }

  let output = "";
  try {
    output = childProcess.execFileSync("ldd", ["--version"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    output = `${error.stdout || ""}\n${error.stderr || ""}`;
  }

  return /musl/i.test(output) ? "musl" : "gnu";
}

function request(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    const req = https.get(
      url,
      {
        headers: {
          "user-agent": USER_AGENT,
          accept: "application/vnd.github+json, application/octet-stream, */*",
        },
      },
      (res) => {
        const location = res.headers.location;
        if (
          location &&
          [301, 302, 303, 307, 308].includes(res.statusCode || 0)
        ) {
          res.resume();
          if (redirects > 5) {
            reject(new Error(`Too many redirects for ${url}`));
            return;
          }
          request(new URL(location, url).toString(), redirects + 1)
            .then(resolve)
            .catch(reject);
          return;
        }

        if ((res.statusCode || 0) < 200 || (res.statusCode || 0) >= 300) {
          res.resume();
          reject(new Error(`HTTP ${res.statusCode} fetching ${url}`));
          return;
        }

        resolve(res);
      }
    );
    req.on("error", reject);
  });
}

async function fetchText(url) {
  const res = await request(url);
  return new Promise((resolve, reject) => {
    let body = "";
    res.setEncoding("utf8");
    res.on("data", (chunk) => {
      body += chunk;
    });
    res.on("end", () => resolve(body));
    res.on("error", reject);
  });
}

async function resolveReleaseTag(release) {
  if (release !== "latest") {
    return release;
  }

  const body = await fetchText(`https://api.github.com/repos/${REPO}/releases/latest`);
  const json = JSON.parse(body);
  if (!json.tag_name) {
    throw new Error("GitHub latest release response did not include tag_name");
  }
  return json.tag_name;
}

function parseChecksum(sums, archiveName) {
  for (const line of sums.split(/\r?\n/)) {
    const match = line.match(/^([a-fA-F0-9]{64})\s+\*?(.+)$/);
    if (match && match[2].trim() === archiveName) {
      return match[1].toLowerCase();
    }
  }
  return null;
}

function downloadFile(url, dest, expectedSha256) {
  const temp = `${dest}.tmp-${process.pid}`;
  fs.mkdirSync(path.dirname(dest), { recursive: true });

  return new Promise((resolve, reject) => {
    request(url)
      .then((res) => {
        const total = Number(res.headers["content-length"] || 0);
        let downloaded = 0;
        const hash = crypto.createHash("sha256");
        const file = fs.createWriteStream(temp);

        const cleanup = (error) => {
          file.destroy();
          try {
            fs.unlinkSync(temp);
          } catch {}
          reject(error);
        };

        res.on("data", (chunk) => {
          downloaded += chunk.length;
          hash.update(chunk);
          if (total > 0) {
            const pct = Math.round((downloaded / total) * 100);
            process.stderr.write(`\rDownloading Forge release: ${pct}%`);
          }
        });

        res.on("error", cleanup);
        file.on("error", cleanup);
        file.on("finish", () => {
          const actual = hash.digest("hex");
          if (expectedSha256 && actual !== expectedSha256) {
            cleanup(
              new Error(
                `Checksum mismatch for ${path.basename(dest)}: expected ${expectedSha256}, got ${actual}`
              )
            );
            return;
          }
          fs.renameSync(temp, dest);
          if (total > 0) {
            process.stderr.write("\n");
          }
          resolve();
        });

        res.pipe(file);
      })
      .catch((error) => {
        try {
          fs.unlinkSync(temp);
        } catch {}
        reject(error);
      });
  });
}

function runTar(archive, dest) {
  fs.mkdirSync(dest, { recursive: true });
  childProcess.execFileSync("tar", ["-xzf", archive, "-C", dest], {
    stdio: "pipe",
  });
}

async function ensureRelease(release) {
  const { artifact, archiveName } = platformInfo();
  const tag = await resolveReleaseTag(release);
  const installDir = path.join(CACHE_ROOT, "releases", tag, artifact);
  const readyFile = path.join(installDir, ".ready");
  const binaryPath = path.join(installDir, "forge");
  const ctlPath = path.join(installDir, "forge-ctl");

  if (
    fs.existsSync(readyFile) &&
    fs.existsSync(binaryPath) &&
    fs.existsSync(ctlPath) &&
    fs.existsSync(path.join(installDir, "web", "dist", "index.html"))
  ) {
    return { binaryPath, ctlPath, installDir, tag };
  }

  const archive = path.join(CACHE_ROOT, "archives", tag, archiveName);
  const releaseBase = `https://github.com/${REPO}/releases/download/${tag}`;

  let expectedSha256 = null;
  try {
    const sums = await fetchText(`${releaseBase}/SHA256SUMS`);
    expectedSha256 = parseChecksum(sums, archiveName);
  } catch {}

  if (!fs.existsSync(archive)) {
    console.error(`Fetching Forge ${tag} for ${artifact}...`);
    await downloadFile(`${releaseBase}/${archiveName}`, archive, expectedSha256);
  } else if (expectedSha256) {
    const hash = crypto.createHash("sha256");
    hash.update(fs.readFileSync(archive));
    const actual = hash.digest("hex");
    if (actual !== expectedSha256) {
      fs.unlinkSync(archive);
      console.error(`Cached Forge archive checksum changed; refetching ${archiveName}...`);
      await downloadFile(`${releaseBase}/${archiveName}`, archive, expectedSha256);
    }
  }

  const tempDir = `${installDir}.tmp-${process.pid}`;
  fs.rmSync(tempDir, { recursive: true, force: true });
  fs.mkdirSync(tempDir, { recursive: true });
  runTar(archive, tempDir);

  for (const executable of ["forge", "forge-ctl"]) {
    const executablePath = path.join(tempDir, executable);
    if (!fs.existsSync(executablePath)) {
      throw new Error(`Release archive did not include ${executable}`);
    }
    fs.chmodSync(executablePath, 0o755);
  }

  if (!fs.existsSync(path.join(tempDir, "web", "dist", "index.html"))) {
    throw new Error("Release archive did not include web/dist assets");
  }

  fs.rmSync(installDir, { recursive: true, force: true });
  fs.renameSync(tempDir, installDir);
  fs.writeFileSync(readyFile, `${new Date().toISOString()}\n`);

  return { binaryPath, ctlPath, installDir, tag };
}

function openBrowserWhenReady(child, dataDir) {
  const started = Date.now();
  const deadlineMs = 30000;
  let stopped = false;

  child.once("exit", () => {
    stopped = true;
  });

  const tick = () => {
    if (stopped) return;
    const url = readServerUrl(dataDir);
    if (!url) {
      if (Date.now() - started < deadlineMs) {
        setTimeout(tick, 500);
      }
      return;
    }
    httpGet(`${url}/healthz`)
      .then(() => {
        openUrl(url);
      })
      .catch(() => {
        if (Date.now() - started < deadlineMs) {
          setTimeout(tick, 500);
        }
      });
  };

  setTimeout(tick, 500);
}

function readServerUrl(dataDir) {
  try {
    const state = JSON.parse(
      fs.readFileSync(path.join(dataDir, SERVER_STATE_FILE), "utf8")
    );
    if (typeof state.server_url === "string" && state.server_url.trim()) {
      return state.server_url.trim().replace(/\/+$/, "");
    }
  } catch {}
  return null;
}

function httpGet(url) {
  return new Promise((resolve, reject) => {
    const req = require("node:http").get(url, (res) => {
      res.resume();
      if ((res.statusCode || 0) >= 200 && (res.statusCode || 0) < 500) {
        resolve();
      } else {
        reject(new Error(`HTTP ${res.statusCode}`));
      }
    });
    req.setTimeout(1000, () => {
      req.destroy(new Error("timeout"));
    });
    req.on("error", reject);
  });
}

function openUrl(url) {
  const opener =
    process.platform === "darwin"
      ? ["open", [url]]
      : ["xdg-open", [url]];

  try {
    childProcess.spawn(opener[0], opener[1], {
      detached: true,
      stdio: "ignore",
    }).unref();
  } catch {}
}

function forgeDataDir(env, args) {
  if (env.FORGE_DATA_DIR) {
    return path.resolve(env.FORGE_DATA_DIR);
  }
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--data-dir" && args[i + 1]) {
      return path.resolve(args[i + 1]);
    }
    if (args[i].startsWith("--data-dir=")) {
      return path.resolve(args[i].slice("--data-dir=".length));
    }
  }
  return path.join(os.homedir(), ".forge");
}

function runBinary(binary, args, env, openBrowser) {
  const child = childProcess.spawn(binary, args, {
    env,
    stdio: "inherit",
  });

  if (openBrowser) {
    openBrowserWhenReady(child, forgeDataDir(env, args));
  }

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code || 0);
  });
  child.on("error", (error) => {
    console.error(`Failed to start ${path.basename(binary)}: ${error.message}`);
    process.exit(1);
  });

  process.on("SIGINT", () => child.kill("SIGINT"));
  process.on("SIGTERM", () => child.kill("SIGTERM"));
}

async function main() {
  const rawArgs = process.argv.slice(2);
  const ctlInvocation = isCtlInvocation(rawArgs);
  if (isHelp(rawArgs) && !ctlInvocation) {
    usage();
    return;
  }
  if (isVersion(rawArgs) && !ctlInvocation) {
    console.log(PACKAGE.version);
    return;
  }

  const options = parseArgs(rawArgs);
  const release = await ensureRelease(options.release);
  const env = {
    ...process.env,
    FORGE_WEB_DIST_DIR: path.join(release.installDir, "web", "dist"),
  };
  const binary = options.command === "forge-ctl" ? release.ctlPath : release.binaryPath;
  runBinary(binary, options.passthrough, env, options.openBrowser);
}

module.exports = { linuxLibc, platformInfo };

if (require.main === module) {
  main().catch((error) => {
    console.error(`forge npm bootstrap failed: ${error.message}`);
    if (process.env.FORGE_NPX_DEBUG && error.stack) {
      console.error(error.stack);
    }
    process.exit(1);
  });
}
