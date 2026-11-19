import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";

const script = readFileSync(new URL("../marketing-website/site.js", import.meta.url), "utf8");
const page = readFileSync(new URL("../marketing-website/index.html", import.meta.url), "utf8");

test("links to the Medium article and identifies release-dependent features", () => {
  assert.match(page, /href="https:\/\/medium\.com\/@ajinkya_Patil\/linux-terminal-for-claude-code-codex-gemini-221546a50632"/);
  assert.match(page, /availability in downloadable packages depends on the release/);
  assert.match(page, /docs\/RELEASE_NOTES\.md/);
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
