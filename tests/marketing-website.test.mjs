import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";

const script = readFileSync(new URL("../marketing-website/site.js", import.meta.url), "utf8");
const page = readFileSync(new URL("../marketing-website/index.html", import.meta.url), "utf8");

test("links to the Medium article and identifies release-dependent features", () => {
  assert.match(page, /href="https:\/\/medium\.com\/@ajinkya_Patil\/linux-terminal-for-claude-code-codex-gemini-221546a50632"/);
  assert.match(page, /availability in downloadable packages depends on the release/);
  assert.match(page, /docs\/RELEASE_NOTES\.md/);
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
      querySelectorAll: () => [button],
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
