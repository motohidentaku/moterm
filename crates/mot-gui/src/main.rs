//! moterm GUI エントリポイント。
//! 使い方:
//!   moterm [config.lua]              設定を指定して起動
//!   moterm --screenshot <png> [cfg]  数フレーム描画後に PNG を保存して終了（検証用）

// Windows: リリースビルドのみ GUI サブシステムで起動し、コンソール窓を出さない
// （配布時に黒いコンソールが一緒に開かないようにする）。
// デバッグビルド(debug_assertions)はコンソール付きにして、env_logger のログ(stderr)を
// その場のコンソールで確認できるようにする（診断用。レベルは RUST_LOG で制御）。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use mot_gui::app::App;
use std::path::PathBuf;
use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    // ログは既定で stderr へ出す。ただし Windows は GUI サブシステム
    // (`windows_subsystem = "windows"`) でコンソールを持たないため stderr が失われる。
    // 診断用に MOTERM_LOG_FILE=<path> を指定するとそのファイルへ追記出力する。
    // レベルは従来どおり RUST_LOG（既定 warn）で制御。例: RUST_LOG=info,russh=debug
    {
        let mut builder =
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"));
        if let Ok(path) = std::env::var("MOTERM_LOG_FILE") {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                Ok(file) => {
                    builder.target(env_logger::Target::Pipe(Box::new(file)));
                }
                Err(e) => eprintln!("MOTERM_LOG_FILE を開けません ({path}): {e}"),
            }
        }
        builder.init();
    }
    log::info!("moterm 起動");

    let mut args = std::env::args().skip(1);
    let mut config_path: Option<PathBuf> = None;
    let mut screenshot: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--screenshot" => screenshot = args.next(),
            other => config_path = Some(PathBuf::from(other)),
        }
    }
    if screenshot.is_none() {
        screenshot = std::env::var("MOTERM_SCREENSHOT").ok();
    }

    let config = mot_core::config_lua::load_config(config_path.as_deref()).unwrap_or_else(|e| {
        eprintln!("設定読み込みに失敗（デモ設定で起動）: {e}");
        demo_config()
    });
    let config = if config.profiles.is_empty() {
        // 空設定はデモプロファイルで補う（初回起動・スクショ用）
        demo_config()
    } else {
        config
    };
    // 設定アイコンで開く設定ファイル: 解決できた実ファイル、無ければ既定の moterm.lua。
    let resolved = mot_core::config_lua::resolve_config_path(config_path.as_deref());
    let config_dir = mot_core::config_lua::config_dir(resolved.as_deref());
    let config_file =
        resolved.unwrap_or_else(|| config_dir.join(mot_core::config_lua::CONFIG_FILE));

    let mut app = App::new(config, config_dir, config_file, screenshot)?;
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// 内蔵デモ設定（設定ファイルが無い/空のとき）。
fn demo_config() -> mot_core::model::Config {
    use mot_core::model::{AuthMethod, GroupCfg, Profile};
    let mk = |name: &str, host: &str, group: Option<&str>| Profile {
        name: name.into(),
        host: host.into(),
        group: group.map(|g| g.into()),
        user: Some("deploy".into()),
        auth: AuthMethod::Agent,
        ..Default::default()
    };
    mot_core::model::Config {
        profiles: vec![
            mk("web", "10.0.0.10", Some("production")),
            mk("db", "10.0.0.20", Some("production")),
            mk("staging", "10.0.1.5", Some("staging")),
            mk("laptop", "192.168.1.2", None),
        ],
        groups: vec![
            GroupCfg {
                name: "production".into(),
                label: Some("本番環境".into()),
                connect: "parallel".into(),
                order: None,
            },
            GroupCfg {
                name: "staging".into(),
                label: Some("検証環境".into()),
                connect: "sequential".into(),
                order: None,
            },
        ],
        ..Default::default()
    }
}
