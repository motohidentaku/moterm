//! ビルドスクリプト。Windows ターゲットのとき、assets/icon.png を .ico に変換して
//! 実行ファイル(moterm.exe)へ埋め込む（エクスプローラ／タスクバーのアイコンになる）。
//! PNG が無ければ何もしない（アイコン無しでビルドは通る）。
//! Windows 以外のターゲット（Linux/macOS）では完全に no-op。

use std::path::PathBuf;

fn main() {
    // リポジトリ直下の assets/icon.png（crates/mot-gui から2つ上）。
    let icon_png = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/icon.png")
        .canonicalize()
        .ok();
    if let Some(p) = &icon_png {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    println!("cargo:rerun-if-changed=build.rs");

    // Windows ターゲットのときだけアイコンを埋め込む。
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let Some(icon_png) = icon_png else {
        println!("cargo:warning=assets/icon.png が見つかりません。アイコン無しでビルドします。");
        return;
    };

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico_path = out_dir.join("moterm.ico");

    if let Err(e) = png_to_ico(&icon_png, &ico_path) {
        println!("cargo:warning=アイコン変換に失敗（アイコン無しで継続）: {e}");
        return;
    }

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_str().unwrap());
    if let Err(e) = res.compile() {
        println!("cargo:warning=アイコン埋め込みに失敗（アイコン無しで継続）: {e}");
    }
}

/// PNG を複数サイズの .ico へ変換する（Windows のアイコンは複数解像度を内包する）。
fn png_to_ico(
    png: &std::path::Path,
    ico: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let img = image::open(png)?.to_rgba8();
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    // 代表的なアイコンサイズを内包する。
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        let resized =
            image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3);
        let icon_image = ico::IconImage::from_rgba_data(size, size, resized.into_raw());
        dir.add_entry(ico::IconDirEntry::encode(&icon_image)?);
    }
    let file = std::fs::File::create(ico)?;
    dir.write(file)?;
    Ok(())
}
