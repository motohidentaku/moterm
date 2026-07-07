//! GUI 管理プロファイル（profiles.json）の永続化と moterm.lua 分とのマージ。
//!
//! 置き場所は moterm.lua と同じディレクトリ（`config_lua::config_dir` が解決した dir を渡す）。
//! moterm.lua 記載のプロファイルは読み取り専用、GUI 管理分は profiles.json に保存する。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::Profile;

/// GUI 管理プロファイルのファイル名
pub const PROFILES_FILE: &str = "profiles.json";

/// profiles.json のフルパス
pub fn gui_profiles_path(dir: &Path) -> PathBuf {
    dir.join(PROFILES_FILE)
}

/// profiles.json を読み込む。無い/壊れている場合は空（起動を止めない）。
/// 読み込んだプロファイルには read_only=false を付与する。
pub fn load_gui_profiles(dir: &Path) -> Vec<Profile> {
    let path = gui_profiles_path(dir);
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_slice::<Vec<Profile>>(&data) {
        Ok(mut profiles) => {
            for p in &mut profiles {
                p.read_only = false;
            }
            profiles
        }
        Err(e) => {
            log::warn!("{} の読み込みに失敗（無視して空扱い）: {e}", path.display());
            Vec::new()
        }
    }
}

/// profiles.json へ保存する（tmp へ書いてアトミック rename、破損防止）。
pub fn save_gui_profiles(dir: &Path, profiles: &[Profile]) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("ディレクトリを作成できません: {}", dir.display()))?;
    let json = serde_json::to_string_pretty(profiles).context("profiles.json の生成に失敗")?;
    let path = gui_profiles_path(dir);
    let tmp = dir.join(format!("{PROFILES_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json.as_bytes())
        .with_context(|| format!("一時ファイルへ書き込めません: {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存に失敗: {}", path.display())
    })?;
    Ok(())
}

/// moterm.lua 分（read_only=true を付与）と GUI 管理分をマージする。
/// 名前が重複した場合は moterm.lua 側を優先し、GUI 側は除外する。
pub fn merge_profiles(lua: &[Profile], gui: &[Profile]) -> Vec<Profile> {
    let mut out = Vec::with_capacity(lua.len() + gui.len());
    for p in lua {
        let mut p = p.clone();
        p.read_only = true;
        out.push(p);
    }
    for g in gui {
        if lua.iter().any(|l| l.name == g.name) {
            continue;
        }
        let mut g = g.clone();
        g.read_only = false;
        out.push(g);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, host: &str) -> Profile {
        Profile {
            name: name.into(),
            host: host.into(),
            port: 22,
            ..Profile::default()
        }
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = crate::test_util::temp_dir("store_missing");
        assert!(load_gui_profiles(&dir).is_empty());
    }

    #[test]
    fn corrupt_file_is_empty() {
        let dir = crate::test_util::temp_dir("store_corrupt");
        std::fs::write(gui_profiles_path(&dir), b"{ not json").unwrap();
        assert!(load_gui_profiles(&dir).is_empty());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = crate::test_util::temp_dir("store_roundtrip");
        let profiles = vec![profile("a", "host-a"), profile("b", "host-b")];
        save_gui_profiles(&dir, &profiles).unwrap();
        let loaded = load_gui_profiles(&dir);
        assert_eq!(loaded, profiles);
        assert!(loaded.iter().all(|p| !p.read_only));
        // 一時ファイルが残っていないこと（アトミック保存）
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "tmp が残留: {leftovers:?}");
    }

    #[test]
    fn save_creates_missing_dir() {
        let dir = crate::test_util::temp_dir("store_mkdir").join("nested");
        save_gui_profiles(&dir, &[profile("a", "h")]).unwrap();
        assert_eq!(load_gui_profiles(&dir).len(), 1);
    }

    #[test]
    fn merge_marks_lua_readonly_and_prefers_lua_on_name_clash() {
        let lua = vec![profile("web", "lua-host"), profile("db", "lua-db")];
        let gui = vec![profile("web", "gui-host"), profile("extra", "gui-extra")];
        let merged = merge_profiles(&lua, &gui);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].name, "web");
        assert_eq!(merged[0].host, "lua-host"); // lua 優先
        assert!(merged[0].read_only);
        assert!(merged[1].read_only);
        assert_eq!(merged[2].name, "extra");
        assert!(!merged[2].read_only);
    }
}
