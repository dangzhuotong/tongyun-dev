import { describe, expect, it } from "vitest";
import pkgRaw from "../package.json?raw";
import tauriRaw from "../src-tauri/tauri.conf.json?raw";
import cargoRaw from "../../Cargo.toml?raw";
import releaseYml from "../../.github/workflows/release.yml?raw";
import buildYml from "../../.github/workflows/build.yml?raw";
import { resolveDisplayedAppVersion } from "./updater";

describe("resolveDisplayedAppVersion", () => {
  it("prefers the installed Tauri version and strips a leading v", () => {
    expect(resolveDisplayedAppVersion("v0.3.0", "0.2.0")).toBe("0.3.0");
    expect(resolveDisplayedAppVersion("0.3.0", "0.2.0")).toBe("0.3.0");
  });

  it("falls back to the bundled package version when Tauri has not reported one", () => {
    expect(resolveDisplayedAppVersion(null, "0.2.0")).toBe("0.2.0");
    expect(resolveDisplayedAppVersion("  ", "0.2.0")).toBe("0.2.0");
  });
});

describe("release versions stay aligned", () => {
  it("keeps package.json, tauri.conf.json, and workspace Cargo.toml on the same version", () => {
    const pkg = JSON.parse(pkgRaw) as { version: string };
    const tauri = JSON.parse(tauriRaw) as {
      version: string;
      bundle: { createUpdaterArtifacts?: boolean };
      plugins: { updater: { pubkey: string; endpoints: string[] } };
    };
    const cargoVersion = cargoRaw.match(/^version = "([^"]+)"/m)?.[1];
    expect(pkg.version).toBe("0.2.1");
    expect(tauri.version).toBe(pkg.version);
    expect(cargoVersion).toBe(pkg.version);
    expect(tauri.bundle.createUpdaterArtifacts).toBe(true);
    expect(tauri.plugins.updater.endpoints).toContain(
      "https://github.com/dangzhuotong/tongyun-dev/releases/latest/download/latest.json",
    );
    expect(tauri.plugins.updater.pubkey.length).toBeGreaterThan(40);
    expect(tauri.plugins.updater.pubkey).not.toContain("REPLACE_WITH_GENERATED_PUBKEY");
  });
});

describe("release workflow", () => {
  it("publishes on v* tags and signs updater artifacts in CI", () => {
    expect(releaseYml).toMatch(/tags:\s*\n\s*- ["']v\*["']/);
    expect(releaseYml).toContain(
      "TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}",
    );
    expect(releaseYml).toContain(
      "TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}",
    );
    expect(releaseYml).toContain("includeUpdaterJson:");
    expect(releaseYml).toContain("projectPath: apps/desktop");
  });

  it("does not package installers on every push to main", () => {
    expect(buildYml).not.toMatch(/branches:\s*\n\s*- main/);
    expect(buildYml).toContain("workflow_dispatch");
  });
});
