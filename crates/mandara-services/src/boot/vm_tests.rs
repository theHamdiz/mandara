//! The user side of an end-to-end run in a disposable virtual machine
//! (never on a real system): stages a theme or an image, detects the
//! machine, plans and writes the approved request that `mandara
//! boot-admin apply` then applies as root. Run inside the VM with
//! `--ignored`:
//!
//! ```text
//! MANDARA_BOOT_VM=menu:vimix/1080p MANDARA_BOOT_VM_OUT=/tmp/req.json <test binary> --ignored boot::vm_tests
//! ```
//!
//! `MANDARA_BOOT_VM` is `menu:<catalog id>`, `splash:<catalog id>`,
//! `menu-image:<file>`, `splash-image:<file>` or `limine:<Mandara theme>[:<image file>]`.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::detect;
use super::image::Options;
use super::plan::{self, Approved, LimineStyle, Request, SplashChoice};
use super::stage::{self, Env};

#[test]
#[ignore = "runs inside a disposable VM only"]
fn vm_stage_plan_and_approve() {
    let Ok(spec) = std::env::var("MANDARA_BOOT_VM") else { return };
    let out = PathBuf::from(std::env::var("MANDARA_BOOT_VM_OUT").expect("MANDARA_BOOT_VM_OUT"));
    let env = Env::current();
    let det = detect::detect_now();
    let screen = det.screen.unwrap_or((1280, 800));
    let menu = (1280, 800);
    let cancel = AtomicBool::new(false);
    let (what, arg) = spec.split_once(':').expect("kind:argument");
    let request = match what {
        "menu" | "splash" => {
            let (t, v) = super::catalog::resolve(arg).expect("a catalog id");
            let staged = stage::stage_catalog(t, v, &env, &|_| {}, &cancel).expect("staged");
            if what == "menu" { Request::MenuTheme { gfxmode: staged.include.is_none().then(|| format!("{}x{}", menu.0, menu.1)), theme: staged } } else { Request::Splash { theme: SplashChoice::Staged(staged) } }
        }
        "menu-image" | "splash-image" => {
            let opts = Options { background: [0x0b, 0x10, 0x26], accent: [0x7c, 0xd5, 0xc4], ..Options::default() };
            let made = stage::from_image(Path::new(arg), &opts, screen, menu, &env).expect("made from the image");
            if what == "menu-image" { Request::MenuTheme { theme: made.grub, gfxmode: Some(format!("{}x{}", menu.0, menu.1)) } } else { Request::Splash { theme: SplashChoice::Staged(made.splash) } }
        }
        "limine" => {
            let (theme, image) = arg.split_once(':').map(|(t, i)| (t, Some(i))).unwrap_or((arg, None));
            let wallpaper = image.map(|i| stage::from_image(Path::new(i), &Options::default(), screen, menu, &env).expect("wallpaper").wallpaper);
            let p = mandara_theme::catalog::find(theme).expect("a Mandara theme").palette(true);
            let mut style = LimineStyle::from_palette(&p, wallpaper);
            style.branding = Some("Mandara Boot look".into());
            Request::LimineStyle { style }
        }
        other => panic!("unknown {other}"),
    };
    let plan = plan::build(&det, &request, None).unwrap_or_else(|e| panic!("no plan: {e}"));
    for s in &plan.steps {
        println!("{:<60} {}", s.label(), s.detail());
    }
    let approved = Approved { digest: plan.digest(), request };
    std::fs::write(&out, serde_json::to_string_pretty(&approved).unwrap()).unwrap();
    println!("approved request: {}", out.display());
}
