import fs from "node:fs/promises";

/**
 * 生成并上传 Tauri Updater 所需的 latest.json（静态更新清单）。
 *
 * 背景：GitHub Actions matrix 并行构建时，tauri-action 可能在不同 job 间互相覆盖 latest.json，
 * 导致缺少某些平台（例如 darwin-aarch64），从而在应用内更新时报错。
 *
 * 本脚本用于在所有平台构建产物上传完成后，统一从 Release 资产生成完整 platforms 映射，并上传 latest.json。
 */

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    throw new Error(`Missing env: ${name}`);
  }
  return value;
}

function parseOwnerRepo(value) {
  const [owner, repo] = value.split("/", 2);
  if (!owner || !repo) {
    throw new Error(`Invalid GITHUB_REPOSITORY: ${value}`);
  }
  return { owner, repo };
}

async function githubRequestJson(url, token) {
  const response = await fetch(url, {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/vnd.github+json",
      "User-Agent": "token-proxy-updater-json",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub API ${response.status} ${response.statusText}: ${url}`);
  }
  return response.json();
}

async function githubRequestText(url, token, accept) {
  const response = await fetch(url, {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: accept,
      "User-Agent": "token-proxy-updater-json",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!response.ok) {
    throw new Error(`GitHub API ${response.status} ${response.statusText}: ${url}`);
  }
  const buffer = Buffer.from(await response.arrayBuffer());
  return buffer.toString("utf8");
}

function findSingleAsset(assets, pattern, label) {
  const matches = assets.filter((asset) => pattern.test(asset.name));
  if (matches.length === 0) {
    throw new Error(`Missing release asset for ${label} (${pattern})`);
  }
  if (matches.length > 1) {
    const names = matches.map((asset) => asset.name).join(", ");
    throw new Error(`Multiple release assets matched for ${label}: ${names}`);
  }
  return matches[0];
}

function resolveSignatureAsset(assetsByName, updaterAssetName) {
  const sigName = `${updaterAssetName}.sig`;
  const sigAsset = assetsByName.get(sigName);
  if (!sigAsset) {
    throw new Error(`Missing signature asset: ${sigName}`);
  }
  return sigAsset;
}

function stripTagPrefix(tagName) {
  return tagName.startsWith("v") ? tagName.slice(1) : tagName;
}

function pickPrimaryBundle(os) {
  // 与 tauri-action 的默认策略保持一致：Windows 优先 MSI，Linux 优先 AppImage。
  if (os === "windows") return ["msi", "nsis"];
  if (os === "linux") return ["appimage", "deb", "rpm"];
  if (os === "darwin") return ["app"];
  return [];
}

function buildUpdaterRules(version) {
  return [
    // macOS app bundle (updater 需要 .app.tar.gz)
    {
      os: "darwin",
      arch: "aarch64",
      bundle: "app",
      label: "macOS (aarch64) app.tar.gz",
      assetPattern: /^Token\.Proxy_aarch64\.app\.tar\.gz$/,
    },
    {
      os: "darwin",
      arch: "x86_64",
      bundle: "app",
      label: "macOS (x86_64) app.tar.gz",
      assetPattern: /^Token\.Proxy_x64\.app\.tar\.gz$/,
    },
    // Windows installers
    {
      os: "windows",
      arch: "x86_64",
      bundle: "msi",
      label: "Windows (x86_64) msi",
      assetPattern: new RegExp(`^Token\\.Proxy_${version}_x64_.*\\.msi$`),
    },
    {
      os: "windows",
      arch: "x86_64",
      bundle: "nsis",
      label: "Windows (x86_64) nsis exe",
      assetPattern: new RegExp(`^Token\\.Proxy_${version}_x64-setup\\.exe$`),
    },
    // Linux (x86_64 / aarch64)
    {
      os: "linux",
      arch: "x86_64",
      bundle: "appimage",
      label: "Linux (x86_64) AppImage",
      assetPattern: new RegExp(`^Token\\.Proxy_${version}_amd64\\.AppImage$`),
    },
    {
      os: "linux",
      arch: "aarch64",
      bundle: "appimage",
      label: "Linux (aarch64) AppImage",
      assetPattern: new RegExp(`^Token\\.Proxy_${version}_aarch64\\.AppImage$`),
    },
    {
      os: "linux",
      arch: "x86_64",
      bundle: "deb",
      label: "Linux (x86_64) deb",
      assetPattern: new RegExp(`^Token\\.Proxy_${version}_amd64\\.deb$`),
    },
    // 注意：deb 的 arm64 发行包命名为 arm64，但 updater 平台键使用 aarch64。
    {
      os: "linux",
      arch: "aarch64",
      bundle: "deb",
      label: "Linux (aarch64) deb",
      assetPattern: new RegExp(`^Token\\.Proxy_${version}_arm64\\.deb$`),
    },
    {
      os: "linux",
      arch: "x86_64",
      bundle: "rpm",
      label: "Linux (x86_64) rpm",
      assetPattern: new RegExp(`^Token\\.Proxy-${version}-\\d+\\.x86_64\\.rpm$`),
    },
    {
      os: "linux",
      arch: "aarch64",
      bundle: "rpm",
      label: "Linux (aarch64) rpm",
      assetPattern: new RegExp(`^Token\\.Proxy-${version}-\\d+\\.aarch64\\.rpm$`),
    },
  ];
}

async function main() {
  const token = requireEnv("GITHUB_TOKEN");
  const tagName = requireEnv("TAG_NAME");
  const { owner, repo } = parseOwnerRepo(requireEnv("GITHUB_REPOSITORY"));

  const apiBase = process.env.GITHUB_API_URL || "https://api.github.com";
  const release = await githubRequestJson(
    `${apiBase}/repos/${owner}/${repo}/releases/tags/${encodeURIComponent(tagName)}`,
    token
  );

  const version = stripTagPrefix(release.tag_name || tagName);
  const notes = typeof release.body === "string" ? release.body : "";
  const pubDate =
    typeof release.published_at === "string" && release.published_at
      ? release.published_at
      : new Date().toISOString();

  const assets = await githubRequestJson(
    `${apiBase}/repos/${owner}/${repo}/releases/${release.id}/assets?per_page=100`,
    token
  );
  const assetsByName = new Map(assets.map((asset) => [asset.name, asset]));

  const rules = buildUpdaterRules(version);

  // 先构建 `{os}-{arch}-{bundle}`，再按优先级补齐 `{os}-{arch}` 作为主入口。
  const platforms = {};
  const candidatesByPlatform = new Map();

  for (const rule of rules) {
    const updaterAsset = findSingleAsset(assets, rule.assetPattern, rule.label);
    const signatureAsset = resolveSignatureAsset(assetsByName, updaterAsset.name);

    // 使用 API 下载 .sig（避免 browser_download_url 的重定向/权限差异）。
    const signature = (
      await githubRequestText(
        `${apiBase}/repos/${owner}/${repo}/releases/assets/${signatureAsset.id}`,
        token,
        "application/octet-stream"
      )
    ).trimEnd();

    const key = `${rule.os}-${rule.arch}-${rule.bundle}`;
    platforms[key] = {
      signature,
      url: updaterAsset.browser_download_url,
    };

    const baseKey = `${rule.os}-${rule.arch}`;
    if (!candidatesByPlatform.has(baseKey)) {
      candidatesByPlatform.set(baseKey, []);
    }
    candidatesByPlatform.get(baseKey).push({ bundle: rule.bundle, key });
  }

  for (const [baseKey, candidates] of candidatesByPlatform.entries()) {
    const [os] = baseKey.split("-", 1);
    const bundlePriority = pickPrimaryBundle(os);
    const preferred = bundlePriority
      .map((bundle) => candidates.find((item) => item.bundle === bundle))
      .find(Boolean);

    if (!preferred) {
      throw new Error(`No primary bundle candidate for ${baseKey}`);
    }

    platforms[baseKey] = platforms[preferred.key];
  }

  const latestJson = {
    version,
    notes,
    pub_date: pubDate,
    platforms,
  };

  const filePath = "latest.json";
  await fs.writeFile(filePath, `${JSON.stringify(latestJson, null, 2)}\n`);

  // 删除旧 latest.json（如果存在），再上传新版本，避免同名冲突。
  const existing = assets.find((asset) => asset.name === "latest.json");
  if (existing) {
    await fetch(`${apiBase}/repos/${owner}/${repo}/releases/assets/${existing.id}`, {
      method: "DELETE",
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: "application/vnd.github+json",
        "User-Agent": "token-proxy-updater-json",
        "X-GitHub-Api-Version": "2022-11-28",
      },
    });
  }

  const uploadUrl = String(release.upload_url || "").replace(
    "{?name,label}",
    `?name=${encodeURIComponent("latest.json")}`
  );
  if (!uploadUrl.startsWith("http")) {
    throw new Error("Invalid release.upload_url");
  }

  const fileBytes = await fs.readFile(filePath);
  const uploadResponse = await fetch(uploadUrl, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: "application/vnd.github+json",
      "Content-Type": "application/octet-stream",
      "Content-Length": String(fileBytes.length),
      "User-Agent": "token-proxy-updater-json",
      "X-GitHub-Api-Version": "2022-11-28",
    },
    body: fileBytes,
  });
  if (!uploadResponse.ok) {
    throw new Error(
      `Upload failed: ${uploadResponse.status} ${uploadResponse.statusText}`
    );
  }

  const outputPath = process.env.GITHUB_OUTPUT;
  if (outputPath) {
    await fs.appendFile(outputPath, `updater_json=latest.json\n`);
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
