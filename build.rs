//! Вбудовує у exe значок, відомості про версію й маніфест (шаблон `assets/app.rc`) — лише для Windows.
//! На Linux (тести бібліотеки) нічого не робить.

use std::path::PathBuf;

fn main() {
    for f in ["assets/app.rc", "assets/app.manifest", "assets/icon.ico", "assets/icon_off.ico", "build.rs"] {
        println!("cargo:rerun-if-changed={f}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b, c) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));

    // Препроцесор windres «з'їдає» лапки в -D, тож версію й шляхи підставляємо в шаблон самі.
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let assets = root.join("assets").display().to_string().replace('\\', "/");
    let rc = std::fs::read_to_string(root.join("assets/app.rc"))
        .expect("assets/app.rc")
        .replace("@VER_COMMA@", &format!("{a},{b},{c},0"))
        .replace("@VER_STR@", &format!("{a}.{b}.{c}.0"))
        .replace("@ASSETS@", &assets);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.rc");
    std::fs::write(&out, rc).expect("запис app.rc в OUT_DIR");

    embed_resource::compile(&out, embed_resource::NONE).manifest_required().unwrap();
}
