(() => {
  for (const code of document.querySelectorAll(".install-command > code, .markdown-body pre > code")) {
    const block = code.parentElement;
    if (block.classList.contains("copy-ready")) continue;
    block.classList.add("copy-ready");

    const container = block.matches("pre") ? document.createElement("div") : block;
    if (container !== block) {
      container.className = "command-copy";
      block.before(container);
      container.append(block);
    }

    const controls = document.createElement("span");
    controls.className = "command-copy-controls";
    const status = document.createElement("span");
    status.className = "command-copy-status";
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    status.setAttribute("aria-atomic", "true");
    const button = document.createElement("button");
    button.className = "command-copy-button";
    button.type = "button";
    button.textContent = "Copy";
    button.setAttribute("aria-label", block.matches(".install-command") ? "Copy installation command" : "Copy code");
    controls.append(status, button);
    container.prepend(controls);

    button.addEventListener("click", async () => {
      status.textContent = "";
      try {
        await navigator.clipboard.writeText(code.textContent.replace(/\r?\n$/, ""));
        status.textContent = "Copied to clipboard.";
      } catch {
        const selection = window.getSelection();
        if (selection) {
          const range = document.createRange();
          range.selectNodeContents(code);
          selection.removeAllRanges();
          selection.addRange(range);
        }
        status.textContent = "Copy blocked. Select and copy the command manually.";
      }
    });
  }
})();
