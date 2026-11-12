use super::{has_class, homepage, site_css, text_of};

#[test]
fn looks_have_a_bounded_native_browser_and_optional_search() {
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let picker = page.descendants().find(|node| has_class(*node, "looks-picker")).unwrap();
    let options = picker.descendants().find(|node| node.attribute("id") == Some("look-options")).unwrap();
    let inputs: Vec<_> = options.descendants().filter(|node| node.has_tag_name("input")).collect();
    assert_eq!(inputs.len(), mandara_config::looks::LOOKS.len());
    assert!(inputs.iter().all(|input| !input.has_attribute("hidden") && !input.has_attribute("disabled")));
    let tools = picker.descendants().find(|node| has_class(*node, "looks-tools")).unwrap();
    assert!(tools.has_attribute("hidden"), "search appears only when its handler is ready");
    let search = tools.descendants().find(|node| node.attribute("type") == Some("search")).unwrap();
    assert_eq!(search.attribute("aria-controls"), Some("look-options"));
    assert!(search.has_attribute("aria-label"));
    let count = tools.descendants().find(|node| has_class(*node, "looks-count")).unwrap();
    assert_eq!(count.attribute("role"), Some("status"));
    assert_eq!(text_of(count), format!("{} Looks", inputs.len()));
    for direction in ["-1", "1"] {
        let step = tools.descendants().find(|node| node.attribute("data-look-step") == Some(direction)).unwrap();
        assert!(step.has_attribute("aria-label"));
    }
    let css = site_css();
    let options_rule = css.split("\n.look-options {").nth(1).unwrap().split('}').next().unwrap();
    assert!(options_rule.contains("max-height:") && options_rule.contains("overflow-y: auto;"));
    assert!(css.contains(".look-choice {\n  position: relative;"));
    let focus = css.split(".look-choice:has(:focus-visible) {").nth(1).unwrap().split('}').next().unwrap();
    assert!(focus.contains("outline-offset: 1px;"), "focus is drawn against the page ground, outside the selected accent fill");
    assert!(page.descendants().any(|node| node.attribute("src") == Some("assets/js/look-picker.js") && node.has_attribute("defer")));
}

#[test]
fn public_counts_follow_studios_presets_and_the_plugin_guide() {
    let designer = std::fs::read_to_string(crate::root().join("crates/mandara-studio/src/app/designer.rs")).unwrap();
    let presets = designer.split("const PRESETS: &[Preset] = &[").nth(1).unwrap().split("\n];").next().unwrap();
    let count = presets.matches("name: tk(").count() + presets.matches("styled(tk(").count();
    assert_eq!(count, 19);
    let source = homepage();
    let page = roxmltree::Document::parse(&source).unwrap();
    let bars = page.descendants().find(|node| node.attribute("id") == Some("bars")).unwrap();
    assert!(text_of(bars).contains(&format!("{count} presets")));
    assert!(text_of(bars).contains("Nine styles"), "the screenshot gallery is a subset of Studio's presets");
    for line in presets.lines() {
        if let Some((_, tail)) = line.split_once("name: tk(\"").or_else(|| line.split_once("styled(tk(\"")) {
            let name = tail.split('"').next().unwrap();
            assert!(text_of(bars).contains(name), "{name}: every Studio preset is named on the website");
        }
    }
    let guide = std::fs::read_to_string(crate::root().join("docs/plugins.md")).unwrap();
    let count = guide.lines().filter(|line| line.starts_with("| **")).count();
    assert_eq!(count, 86);
    assert!(guide.contains("| **Taskbar** |"));
    for path in ["README.md", "llms.txt", "CITATION.cff", "AGENTS.md"] {
        let text = std::fs::read_to_string(crate::root().join(path)).unwrap();
        assert!(text.contains(&format!("{count} built-in plugins")), "{path}: plugin count");
        assert!(text.contains("19 bar presets"), "{path}: Studio's full preset count");
    }
}

/// Opt in with MANDARA_SITE_BROWSER pointing to a Chromium binary. The test
/// uses a fresh profile and local files; it never touches a desktop session.
#[test]
fn looks_search_selection_and_fallback_work_in_a_browser() {
    let Some(browser) = std::env::var_os("MANDARA_SITE_BROWSER") else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = crate::root().display().to_string().replace('&', "&amp;").replace('"', "&quot;");
    for scripted in [true, false] {
        let mut source = homepage().replacen("<head>", &format!("<head><base href=\"file://{root}/\" />"), 1);
        if !scripted {
            for path in ["site.js", "look-picker.js"] {
                source = source.replace(&format!("<script src=\"assets/js/{path}\" defer=\"defer\"></script>"), "");
            }
        }
        let harness = r#"<script>
window.addEventListener('load', () => {
  const assert = (value, message) => { if (!value) throw new Error(message); };
  const active = () => document.querySelector('.look-input:checked').value;
  const shown = () => [...document.querySelectorAll('.look-choice')].filter(label => !label.hidden);
  const select = value => document.querySelector('.look-input[value="' + value + '"]').click();
  const preview = value => getComputedStyle(document.querySelector('.look-' + value)).display !== 'none';
  try {
    assert(shown().length === 19, 'initial catalogue');
    select('zen');
    assert(active() === 'zen' && preview('zen') && !preview('corniche-dusk'), 'native selection and preview');
    const tools = document.querySelector('.looks-tools');
    if (document.documentElement.classList.contains('js')) {
      assert(!tools.hidden, 'search controls ready');
      const previous = document.querySelector('[data-look-step="-1"]');
      const next = document.querySelector('[data-look-step="1"]');
      next.click();
      assert(active() === 'corniche-dusk', 'next wraps');
      previous.click();
      assert(active() === 'zen', 'previous wraps');
      const search = document.querySelector('.looks-search');
      const height = document.querySelector('.look-options').clientHeight;
      const filter = value => { search.value = value; search.dispatchEvent(new Event('input')); };
      filter('  SEA glass  ');
      assert(shown().length === 1, 'case-insensitive multi-word search');
      next.click();
      assert(active() === 'sea-glass' && preview('sea-glass'), 'select filtered result');
      document.querySelector('[data-scheme="dark"]').click();
      const accent = document.documentElement.style.getPropertyValue('--accent');
      assert(accent.length > 0, 'selection paints the page');
      document.querySelector('[data-scheme="light"]').click();
      assert(document.documentElement.style.getPropertyValue('--accent') !== accent, 'selected palette follows scheme');
      filter('no such look at all');
      assert(shown().length === 0 && !document.querySelector('.looks-empty').hidden, 'empty search');
      assert(document.querySelector('.look-options').clientHeight === height, 'search keeps the browser height');
      assert(previous.disabled && next.disabled && active() === 'sea-glass', 'empty search preserves selection');
      document.querySelector('[data-look-clear]').click();
      assert(shown().length === 19 && !next.disabled && active() === 'sea-glass', 'clear preserves selection');
      document.querySelector('#look-zen').focus();
      assert(document.querySelector('.look-options').scrollTop > 0, 'focus reaches the final choice');
      document.querySelector('[data-look-reset]').click();
      assert(document.querySelector('[data-look-reset]').hidden, 'page palette resets');
    } else {
      assert(tools.hidden, 'search is absent without scripts');
      assert(shown().every(label => getComputedStyle(label).display !== 'none'), 'every native choice remains available');
    }
    document.body.dataset.pickerTest = 'passed';
  } catch (error) { document.body.dataset.pickerTest = error.message; }
});
</script>"#;
        // The homepage is XML-compatible; inline test JavaScript is HTML text.
        source = source.replace("</body>", &format!("{harness}</body>"));
        let file = dir.path().join("index.html");
        std::fs::write(&file, source).unwrap();
        let output = std::process::Command::new(&browser)
            .args(["--headless=new", "--disable-gpu", "--allow-file-access-from-files", "--force-prefers-reduced-motion", "--dump-dom", "--virtual-time-budget=4000"])
            .arg(format!("--user-data-dir={}", dir.path().join(if scripted { "scripted" } else { "native" }).display()))
            .arg(&file)
            .output().unwrap();
        let rendered = String::from_utf8_lossy(&output.stdout);
        let result = rendered.split("data-picker-test=\"").nth(1).and_then(|tail| tail.split('"').next());
        assert!(output.status.success(), "browser: {}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(result, Some("passed"), "scripted={scripted}");
    }
}
