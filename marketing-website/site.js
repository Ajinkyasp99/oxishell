const copyStatus = document.getElementById("copy-status");

if (navigator.clipboard?.writeText) {
  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.hidden = false;
    const command = document.getElementById(button.dataset.copy);

    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(command.textContent.trim());
        button.textContent = "Copied";
        copyStatus.textContent = "Install command copied to clipboard.";
      } catch {
        const selection = window.getSelection();
        selection.selectAllChildren(command);
        button.textContent = "Copy";
        copyStatus.textContent = "Clipboard access was unavailable. The command is selected for manual copying.";
      }
    });
    button.addEventListener("blur", () => {
      button.textContent = "Copy";
      copyStatus.textContent = "";
    });
  });
}
