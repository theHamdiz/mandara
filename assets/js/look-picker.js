// Keep the Look collection beside its preview as the catalogue grows.
(() => {
  const picker = document.querySelector(".looks-picker");
  if (!picker) return;
  const tools = picker.querySelector(".looks-tools");
  const search = picker.querySelector(".looks-search");
  const count = picker.querySelector(".looks-count");
  const clear = picker.querySelector("[data-look-clear]");
  const empty = picker.querySelector(".looks-empty");
  const list = picker.querySelector(".look-options");
  const steps = [...picker.querySelectorAll("[data-look-step]")];
  const choices = [...list.querySelectorAll(".look-choice")].map((label) => {
    const input = label.querySelector("input");
    const preview = picker.querySelector(`.look-${input.value} .look-blurb`);
    return { label, input, text: `${label.textContent} ${preview?.textContent || ""}`.toLowerCase() };
  });
  const reveal = (label) => {
    const top = label.offsetTop;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + label.offsetHeight > list.scrollTop + list.clientHeight) list.scrollTop = top + label.offsetHeight - list.clientHeight;
  };
  const filter = () => {
    const words = search.value.toLowerCase().split(/\s+/).filter(Boolean);
    let shown = 0;
    for (const choice of choices) {
      choice.label.hidden = !words.every((word) => choice.text.includes(word));
      if (!choice.label.hidden) shown += 1;
    }
    count.textContent = words.length ? `${shown} of ${choices.length} Looks` : `${choices.length} Looks`;
    empty.hidden = shown !== 0;
    clear.disabled = search.value.length === 0;
    for (const step of steps) step.disabled = shown === 0;
    list.scrollTop = 0;
    const selected = choices.find((choice) => choice.input.checked && !choice.label.hidden);
    if (selected) reveal(selected.label);
  };
  search.addEventListener("input", filter);
  search.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowDown") return;
    const first = choices.find((choice) => !choice.label.hidden);
    if (!first) return;
    event.preventDefault();
    first.input.focus({ preventScroll: true });
    reveal(first.label);
  });
  clear.addEventListener("click", () => {
    search.value = "";
    filter();
    search.focus({ preventScroll: true });
  });
  for (const { input, label } of choices) input.addEventListener("focus", () => reveal(label));
  for (const step of steps) {
    step.addEventListener("click", () => {
      const visible = choices.filter((choice) => !choice.label.hidden);
      if (!visible.length) return;
      const current = visible.findIndex((choice) => choice.input.checked);
      const direction = Number(step.dataset.lookStep);
      const index = current < 0 ? (direction > 0 ? 0 : visible.length - 1) : (current + direction + visible.length) % visible.length;
      const { input, label } = visible[index];
      input.checked = true;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      reveal(label);
    });
  }
  tools.hidden = false;
  filter();
})();
