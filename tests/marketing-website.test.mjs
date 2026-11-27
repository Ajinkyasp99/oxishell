import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

const script = readFileSync(new URL("../marketing-website/site.js", import.meta.url), "utf8");
const page = readFileSync(new URL("../marketing-website/index.html", import.meta.url), "utf8");
const sitemap = readFileSync(new URL("../marketing-website/sitemap.xml", import.meta.url), "utf8");

test("publishes indexable product metadata and a canonical sitemap with local screenshots", () => {
  const canonical = page.match(/<link rel="canonical" href="([^"]+)">/)?.[1];
  assert.equal(canonical, "https://ajinkyasp99.github.io/oxishell/");
  assert.match(page, /<meta name="robots" content="max-image-preview:large">/);
  const app = JSON.parse(page.match(/<script type="application\/ld\+json">([\s\S]+?)<\/script>/)[1]);
  assert.equal(app["@type"], "SoftwareApplication");
  assert.equal(app.url, canonical);
  assert.equal(app.sameAs, "https://github.com/Ajinkyasp99/oxishell");
  assert.deepEqual([...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map(([, url]) => url), [canonical]);

  const images = [...sitemap.matchAll(/<image:loc>([^<]+)<\/image:loc>/g)].map(([, url]) => url);
  assert.ok(images.includes(app.screenshot));
  for (const url of images) {
    assert.ok(url.startsWith(canonical + "assets/"), `image is outside the Pages project: ${url}`);
    const file = url.slice(canonical.length);
    assert.ok(page.includes(`src="./${file}"`), `image is not visible on the landing page: ${file}`);
    assert.ok(existsSync(new URL(`../marketing-website/${file}`, import.meta.url)), `missing sitemap image: ${file}`);
  }
});

test("links to the Medium article and identifies release-dependent features", () => {
  assert.match(page, /href="https:\/\/medium\.com\/@ajinkya_Patil\/linux-terminal-for-claude-code-codex-gemini-221546a50632"/);
  assert.match(page, /for availability in downloadable packages/);
  assert.match(page, /docs\/RELEASE_NOTES\.md/);
});

test("preserves published anchors and installer links", () => {
  const ids = new Set(Array.from(page.matchAll(/\bid="([^"]+)"/g), ([, id]) => id));
  for (const id of ["top", "main", "whats-new", "features", "quick-start", "download", "faq", "docs"]) {
    assert.ok(ids.has(id), `missing published anchor ${id}`);
  }
  for (const [, anchor] of page.matchAll(/href="#([^"]+)"/g)) {
    assert.ok(ids.has(anchor), `missing anchor ${anchor}`);
  }
  for (const name of ["oxishell_amd64.deb", "oxishell_macos_arm64.dmg", "oxishell_macos_x86_64.dmg"]) {
    assert.ok(page.includes(`href="https://github.com/Ajinkyasp99/oxishell/releases/latest/download/${name}"`));
  }
});

test("shows the Compose and agent usage release highlights without JavaScript", () => {
  const highlights = page.match(/<section\b[^>]+id="whats-new"[^>]*>([\s\S]+?)<\/section>/)?.[1];
  assert.ok(highlights, "missing release highlights");
  assert.match(page, /<nav\b[^>]+id="main-nav"[\s\S]+?href="#whats-new"/);
  assert.match(highlights, /v0\.6\.0[\s\S]+?compose-current\.webp/);
  assert.match(highlights, /v0\.5\.0[\s\S]+?usage-current\.webp/);
  assert.doesNotMatch(highlights, /\shidden\b/);
});

test("keeps the walkthrough and FAQ answers in the source HTML", () => {
  const steps = [...page.matchAll(/data-panel="(workflow-[^"]+)"/g)].map(([, id]) => id);
  assert.equal(steps.length, 5);
  for (const id of steps) {
    assert.ok(page.includes(`class="workflow-panel feature-panel" id="${id}">`), `step is missing or hidden: ${id}`);
  }
  const faq = page.match(/<section\b[^>]+id="faq"[^>]*>([\s\S]+?)<\/section>/)?.[1];
  assert.ok(faq, "missing FAQ");
  assert.equal([...faq.matchAll(/<summary>[^<]+<\/summary>\s*<p>[^<]/g)].length, 4);
});

test("ships local assets under the Pages path", () => {
  const root = new URL("../marketing-website/", import.meta.url);
  const files = new Set(Array.from(page.matchAll(/(?:\.\/|https:\/\/ajinkyasp99\.github\.io\/oxishell\/)(assets\/[^"\s,]+)/g), ([, file]) => file));
  for (const file of files) {
    assert.ok(existsSync(new URL(file, root)), `missing ${file}`);
  }
  assert.ok(files.has("assets/social-preview.jpg"));
});

function downloadBadge(...pages) {
  const filter = fileURLToPath(new URL("../scripts/package-downloads.jq", import.meta.url));
  return JSON.parse(execFileSync("jq", ["--slurp", "--from-file", filter], {
    input: pages.map((page) => JSON.stringify(page)).join("\n"),
    encoding: "utf8",
  }));
}

test("counts package downloads across release pages and excludes checksums and drafts", () => {
  const badge = downloadBadge([{
    draft: false,
    assets: [
      { name: "oxishell_amd64.deb", download_count: 1001 },
      { name: "oxishell_macos_arm64.dmg", download_count: 25 },
      { name: "SHA256SUMS", download_count: 200 },
      { name: "oxishell_macos_arm64.dmg.sha256", download_count: 100 },
    ],
  }], [{
    draft: false,
    assets: [{ name: "cmux-linux_0.1.0_amd64.deb", download_count: 9 }],
  }, {
    draft: true,
    assets: [{ name: "oxishell_amd64.deb", download_count: 50 }],
  }]);

  assert.deepEqual(badge, {
    schemaVersion: 1,
    label: "package downloads",
    message: "1035",
    color: "blue",
  });
});

test("shows zero when there are no published packages", () => {
  assert.equal(downloadBadge([]).message, "0");
});

function loadCopyControl(clipboard) {
  const listeners = new Map();
  const command = { textContent: " sudo apt install ./oxishell_amd64.deb " };
  const status = { textContent: "" };
  const button = {
    dataset: { copy: "linux-command" },
    hidden: true,
    textContent: "Copy",
    addEventListener: (event, listener) => listeners.set(event, listener),
  };
  let selected;

  runInNewContext(script, {
    navigator: { clipboard },
    document: {
      querySelector: () => null,
      querySelectorAll: (selector) => selector === "[data-copy]" ? [button] : [],
      getElementById: (id) => ({ "linux-command": command, "copy-status": status })[id],
    },
    window: {
      getSelection: () => ({
        selectAllChildren: (element) => { selected = element; },
      }),
    },
  });

  return { button, command, listeners, status, selected: () => selected };
}

test("leaves the copy control hidden when the Clipboard API is unavailable", () => {
  const { button } = loadCopyControl(undefined);
  assert.equal(button.hidden, true);
});

test("copies the install command and announces success", async () => {
  let copied;
  const { button, listeners, status } = loadCopyControl({
    writeText: async (text) => { copied = text; },
  });

  assert.equal(button.hidden, false);
  await listeners.get("click")();
  assert.equal(copied, "sudo apt install ./oxishell_amd64.deb");
  assert.equal(button.textContent, "Copied");
  assert.match(status.textContent, /copied to clipboard/);

  listeners.get("blur")();
  assert.equal(button.textContent, "Copy");
});

test("selects the command for manual copying when clipboard access fails", async () => {
  const { button, command, listeners, selected, status } = loadCopyControl({
    writeText: async () => { throw new Error("Clipboard access denied"); },
  });

  await listeners.get("click")();
  assert.equal(selected(), command);
  assert.equal(button.textContent, "Copy");
  assert.match(status.textContent, /selected for manual copying/);
});

test("only the selected preview is visible and arrow keys move selection and focus", () => {
  let focused;
  function element(id, dataset = {}) {
    const attributes = new Map();
    const listeners = new Map();
    return {
      id,
      dataset,
      hidden: false,
      attributes,
      listeners,
      setAttribute: (key, value) => attributes.set(key, value),
      addEventListener: (event, callback) => listeners.set(event, callback),
      focus: () => { focused = id; },
    };
  }
  const panels = [element("workspaces"), element("alerts")];
  const tabs = panels.map(({ id }) => element(`tab-${id}`, { panel: id }));
  const list = element("tabs", { tabList: "Feature previews" });
  list.hidden = true;
  list.querySelectorAll = () => tabs;
  const group = element("showcase");
  group.querySelector = () => list;

  runInNewContext(script, {
    navigator: {},
    document: {
      querySelector: () => null,
      querySelectorAll: () => [group],
      getElementById: (id) => panels.find((panel) => panel.id === id),
    },
  });

  assert.equal(list.hidden, false);
  assert.equal(list.attributes.get("role"), "tablist");
  assert.deepEqual(panels.map((panel) => panel.hidden), [false, true]);
  assert.deepEqual(tabs.map((tab) => tab.tabIndex), [0, -1]);
  assert.equal(panels[1].attributes.get("aria-labelledby"), tabs[1].id);

  let defaultPrevented = false;
  tabs[0].listeners.get("keydown")({ key: "ArrowLeft", preventDefault: () => { defaultPrevented = true; } });
  assert.equal(defaultPrevented, true);
  assert.equal(focused, tabs[1].id);
  assert.deepEqual(panels.map((panel) => panel.hidden), [true, false]);
  assert.deepEqual(tabs.map((tab) => tab.attributes.get("aria-selected")), ["false", "true"]);

  tabs[0].listeners.get("click")();
  assert.deepEqual(panels.map((panel) => panel.hidden), [false, true]);
});
