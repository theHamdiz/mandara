(() => {
  // A success message stays this long, then clears, so it never describes
  // an older press. Blocked-copy instructions stay until the next press,
  // while someone follows them.
  const SUCCESS_MS = 4000;
  // A live region announces changes, not states: the message is written in a
  // later task than the clearing, so pressing Copy again announces the same
  // words again instead of leaving the region unchanged.
  const ANNOUNCE_DELAY_MS = 100;

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
    // One polite status per block: it is in the page before any message,
    // so screen readers already watch it when the first one arrives.
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

    // The latest press: an older press's late result or timer never
    // replaces its message.
    let press = 0;
    let clearTimer = 0;
    const announce = (current, message, lifetime) => {
      window.setTimeout(() => {
        if (current !== press) return;
        status.textContent = message;
        if (lifetime > 0) {
          clearTimer = window.setTimeout(() => {
            if (current === press) status.textContent = "";
          }, lifetime);
        }
      }, ANNOUNCE_DELAY_MS);
    };

    button.addEventListener("click", async () => {
      const current = ++press;
      window.clearTimeout(clearTimer);
      status.textContent = "";
      try {
        await navigator.clipboard.writeText(code.textContent.replace(/\r?\n$/, ""));
        announce(current, "Copied to clipboard.", SUCCESS_MS);
      } catch {
        const selection = window.getSelection();
        if (selection) {
          const range = document.createRange();
          range.selectNodeContents(code);
          selection.removeAllRanges();
          selection.addRange(range);
        }
        announce(current, "Copy blocked. Select and copy the command manually.", 0);
      }
    });
  }
})();
