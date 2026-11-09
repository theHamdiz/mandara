// The homepage's interactive pieces. Everything here is an enhancement:
// without the script the page shows every Look, shortcut and answer, follows
// the system's light or dark setting and keeps the Mandara Corniche palette.
(() => {
  const root = document.documentElement;
  root.classList.add("js");
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
  const lightScheme = window.matchMedia("(prefers-color-scheme: light)");
  const store = {
    get: (key) => { try { return window.localStorage.getItem(key); } catch { return null; } },
    set: (key, value) => { try { window.localStorage.setItem(key, value); } catch { /* private window */ } },
  };

  // Light or dark: the system's choice unless the visitor picks one here.
  let scheme = ["light", "dark"].includes(store.get("mandara-scheme")) ? store.get("mandara-scheme") : "auto";
  const isLight = () => (scheme === "auto" ? lightScheme.matches : scheme === "light");

  // Looks: the chosen Look's palette replaces the page's colour tokens, in
  // the visitor's light or dark scheme. The palettes come from the shell's
  // own theme engine (checked by `cargo test -p xtask pages`).
  const ROLES = ["ground", "tile", "tile-2", "soft", "on-soft", "ink", "muted", "accent", "on-accent", "second", "on-second", "third", "on-third"];
  const data = document.getElementById("look-palettes");
  const parsed = data ? JSON.parse(data.textContent) : { palettes: {} };
  const palettes = parsed.palettes;
  const reset = document.querySelector("[data-look-reset]");
  const status = document.querySelector(".looks-status");
  const themeColors = document.querySelectorAll('meta[name="theme-color"]');
  let worn = null;

  // Screenshots follow the page's scheme: a forced scheme turns each light
  // source on or off; "auto" gives the media query back to the browser.
  const lightSources = [...document.querySelectorAll('source[media="(prefers-color-scheme: light)"]')];
  const showPictures = () => {
    for (const source of lightSources) source.media = scheme === "auto" ? "(prefers-color-scheme: light)" : (scheme === "light" ? "all" : "not all");
    const video = document.querySelector(".film-video");
    if (video && video.poster) video.poster = video.dataset[isLight() ? "posterLight" : "posterDark"];
  };

  const paint = () => {
    // With no Look worn and the system's scheme, the stylesheet's own tokens apply.
    const id = worn || (scheme === "auto" ? null : parsed.default);
    const palette = id && palettes[id];
    const mode = isLight() ? "light" : "dark";
    for (const role of ROLES) {
      if (palette) root.style.setProperty(`--${role}`, palette[mode][role]);
      else root.style.removeProperty(`--${role}`);
    }
    root.style.colorScheme = scheme === "auto" ? "" : mode;
    for (const meta of themeColors) {
      if (palette) meta.setAttribute("content", scheme === "auto" ? palette[meta.media.includes("light") ? "light" : "dark"].ground : palette[mode].ground);
    }
  };
  const wear = (id) => {
    worn = id;
    paint();
    if (reset) reset.hidden = id === null;
    if (status) status.textContent = id ? `This page now wears ${palettes[id].name}.` : "Back to Mandara Corniche.";
  };
  for (const input of document.querySelectorAll(".look-input")) {
    input.addEventListener("change", () => input.checked && palettes[input.value] && wear(input.value));
  }
  reset?.addEventListener("click", () => {
    wear(null);
    document.querySelector(".look-input:checked")?.focus();
  });
  lightScheme.addEventListener("change", () => { paint(); showPictures(); });

  const schemeGroup = document.querySelector(".scheme");
  if (schemeGroup) {
    schemeGroup.hidden = false;
    const buttons = [...schemeGroup.querySelectorAll("button")];
    const press = () => { for (const button of buttons) button.setAttribute("aria-pressed", String(button.dataset.scheme === scheme)); };
    for (const button of buttons) {
      button.addEventListener("click", () => {
        scheme = button.dataset.scheme;
        store.set("mandara-scheme", scheme);
        press(); paint(); showPictures();
      });
    }
    press();
  }
  if (scheme !== "auto") { paint(); showPictures(); }

  // Words that breathe: each letter of a .wave word swells in turn. The
  // word stays whole for screen readers.
  if (!reduceMotion.matches) {
    for (const word of document.querySelectorAll(".wave")) {
      const text = word.textContent;
      const from = word.matches(".d-accent") ? 500 : 200;
      word.setAttribute("aria-label", text);
      word.textContent = "";
      [...text].forEach((letter, index) => {
        const span = document.createElement("span");
        span.className = "ch";
        span.setAttribute("aria-hidden", "true");
        span.style.setProperty("--i", String(index));
        span.textContent = letter;
        word.append(span);
      });
      word.style.setProperty("--from", String(from));
      word.classList.add("wave-ready");
    }
  }

  // The slider preview: a native range input drawn in each Studio design.
  const preview = document.querySelector(".slider-preview");
  if (preview) {
    const range = preview.querySelector(".slider-input");
    const output = preview.querySelector(".slider-value");
    const segments = preview.querySelectorAll(".slider-segs i");
    const show = () => {
      const value = Number(range.value);
      preview.style.setProperty("--v", String(value / 100));
      output.textContent = `${value}%`;
      const lit = Math.round(value / 10);
      segments.forEach((segment, index) => segment.classList.toggle("on", index < lit));
    };
    range.addEventListener("input", show);
    range.addEventListener("pointerdown", () => preview.classList.add("is-dragging"));
    for (const end of ["pointerup", "pointercancel", "blur"]) range.addEventListener(end, () => preview.classList.remove("is-dragging"));
    for (const design of document.querySelectorAll('input[name="slider-design"]')) {
      design.addEventListener("change", () => { preview.dataset.design = design.value; });
    }
    show();
  }

  // Shortcuts: search and group filters over the real default keybinds.
  const binds = [...document.querySelectorAll(".bind")];
  const search = document.getElementById("keys-search");
  const groups = document.querySelector(".keys-groups");
  const count = document.querySelector(".keys-count");
  if (binds.length && search && groups && count) {
    groups.hidden = false;
    let group = "all";
    const haystack = new Map(binds.map((bind) => [bind, `${bind.textContent} ${bind.dataset.mods} ${bind.dataset.key}`.toLowerCase()]));
    const filter = () => {
      const words = search.value.toLowerCase().split(/\s+/).filter(Boolean);
      let shown = 0;
      for (const bind of binds) {
        const text = haystack.get(bind);
        const visible = (group === "all" || bind.dataset.group === group) && words.every((word) => text.includes(word));
        bind.hidden = !visible;
        if (visible) shown += 1;
      }
      count.textContent = shown === 0 ? "No shortcut matches." : `${shown} shortcut${shown === 1 ? "" : "s"}`;
    };
    search.addEventListener("input", filter);
    for (const button of groups.querySelectorAll("button")) {
      button.addEventListener("click", () => {
        group = button.dataset.group;
        for (const other of groups.querySelectorAll("button")) other.setAttribute("aria-pressed", String(other === button));
        filter();
      });
    }
  }

  // The film: its sources load only after the page has painted, never with
  // reduced motion until asked, and its controls appear only once it has a
  // frame. Without a video file the poster stays, as designed.
  const film = document.querySelector(".film");
  const video = film?.querySelector("video");
  if (film && video) {
    const controls = film.querySelector(".film-controls");
    const play = film.querySelector('[data-film="play"]');
    const sound = film.querySelector('[data-film="sound"]');
    const label = () => {
      play.textContent = video.paused ? "Play" : "Pause";
      play.setAttribute("aria-pressed", String(!video.paused));
      sound.textContent = video.muted ? "Sound on" : "Sound off";
      sound.setAttribute("aria-pressed", String(!video.muted));
    };
    const attach = () => {
      video.poster = video.dataset[isLight() ? "posterLight" : "posterDark"];
      if (reduceMotion.matches) video.removeAttribute("autoplay");
      for (const source of video.querySelectorAll("source[data-src]")) source.src = source.dataset.src;
      video.load();
    };
    video.addEventListener("loadeddata", () => { controls.hidden = false; label(); });
    video.addEventListener("playing", () => { film.classList.add("is-playing"); label(); });
    video.addEventListener("pause", label);
    video.addEventListener("volumechange", label);
    play.addEventListener("click", () => (video.paused ? video.play().catch(() => {}) : video.pause()));
    sound.addEventListener("click", () => {
      video.muted = !video.muted;
      if (!video.muted && video.paused) video.play().catch(() => {});
    });
    // Off screen, the film rests.
    new IntersectionObserver(([entry]) => {
      if (!entry.isIntersecting && !video.paused) { video.pause(); film.dataset.rested = "true"; }
      else if (entry.isIntersecting && film.dataset.rested) { delete film.dataset.rested; video.play().catch(() => {}); }
    }).observe(film);
    const later = () => ("requestIdleCallback" in window ? window.requestIdleCallback(attach, { timeout: 2500 }) : window.setTimeout(attach, 1200));
    if (document.readyState === "complete") later();
    else window.addEventListener("load", later, { once: true });
  }

  // Entrances: each section's parts rise in turn as it arrives.
  if ("IntersectionObserver" in window) {
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("is-in");
        observer.unobserve(entry.target);
      }
    }, { rootMargin: "0px 0px -8% 0px" });
    const facts = document.querySelector(".facts");
    if (facts) observer.observe(facts);
    if (!reduceMotion.matches) {
      for (const part of document.querySelectorAll(".section-head, .gallery .surface, .looks-picker, .bars-picker, .bento .piece, .studio-grid > *, .departure, .language-cloud, .arabic-shot, .shelf-group, .keys-panel, .install-grid > *, .question, .foot-cta")) {
        const siblings = [...part.parentElement.children];
        part.classList.add("reveal");
        part.style.setProperty("--i", String(Math.min(siblings.indexOf(part), 6)));
        observer.observe(part);
      }
    }
  } else {
    document.querySelector(".facts")?.classList.add("is-in");
  }

  // Alexandria's local time, for the footer.
  const clock = document.querySelector(".alexandria-time");
  if (clock) {
    const format = new Intl.DateTimeFormat("en-GB", { timeZone: "Africa/Cairo", hour: "2-digit", minute: "2-digit" });
    const tick = () => {
      const now = new Date();
      clock.textContent = format.format(now);
      clock.dateTime = now.toISOString();
    };
    tick();
    window.setInterval(tick, 30_000);
  }
})();
