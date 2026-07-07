//! GUI の揮発しない UI 状態（ui_state.json）の永続化。
//!
//! 設定（moterm.lua）とは別物。moterm.lua はユーザが書く読み取り専用の設定、
//! こちらは「前回開いた場所」など操作の結果として GUI が書き戻す状態を保持する。
//! 置き場所は profiles.json と同じ config ディレクトリ（`config_lua::config_dir`）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// UI 状態のファイル名
pub const UI_STATE_FILE: &str = "ui_state.json";

/// 永続化する UI 状態。フィールドはすべて任意で、増えても後方互換を保つ。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiState {
    /// SFTP のローカルペインで最後に開いていたディレクトリ（絶対パス）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sftp_local_dir: Option<String>,
}

/// ui_state.json のフルパス
pub fn ui_state_path(dir: &Path) -> PathBuf {
    dir.join(UI_STATE_FILE)
}

/// ui_state.json を読み込む。無い/壊れている場合は既定値（起動を止めない）。
pub fn load_ui_state(dir: &Path) -> UiState {
    let path = ui_state_path(dir);
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return UiState::default(),
    };
    match serde_json::from_slice::<UiState>(&data) {
        Ok(state) => state,
        Err(e) => {
            log::warn!("{} の読み込みに失敗（既定値で継続）: {e}", path.display());
            UiState::default()
        }
    }
}

/// ui_state.json へ保存する（tmp へ書いてアトミック rename、破損防止）。
pub fn save_ui_state(dir: &Path, state: &UiState) -> Result<()> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("ディレクトリを作成できません: {}", dir.display()))?;
    let json = serde_json::to_string_pretty(state).context("ui_state.json の生成に失敗")?;
    let path = ui_state_path(dir);
    let tmp = dir.join(format!("{UI_STATE_FILE}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, json.as_bytes())
        .with_context(|| format!("一時ファイルへ書き込めません: {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| {
        let _ = std::fs::remove_file(&tmp);
        format!("保存に失敗: {}", path.display())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_default() {
        let dir = crate::test_util::temp_dir("ui_state_missing");
        assert_eq!(load_ui_state(&dir), UiState::default());
    }

    #[test]
    fn corrupt_file_is_default() {
        let dir = crate::test_util::temp_dir("ui_state_corrupt");
        std::fs::write(ui_state_path(&dir), b"{ not json").unwrap();
        assert_eq!(load_ui_state(&dir), UiState::default());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = crate::test_util::temp_dir("ui_state_roundtrip");
        let state = UiState {
            sftp_local_dir: Some("/home/user/dl".into()),
        };
        save_ui_state(&dir, &state).unwrap();
        assert_eq!(load_ui_state(&dir), state);
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
        let dir = crate::test_util::temp_dir("ui_state_mkdir").join("nested");
        let state = UiState {
            sftp_local_dir: Some("/tmp".into()),
        };
        save_ui_state(&dir, &state).unwrap();
        assert_eq!(load_ui_state(&dir), state);
    }

    #[test]
    fn empty_state_omits_null_fields() {
        // 既定値は空 JSON オブジェクトになる（None を null で書き出さない）。
        let dir = crate::test_util::temp_dir("ui_state_empty");
        save_ui_state(&dir, &UiState::default()).unwrap();
        let raw = std::fs::read_to_string(ui_state_path(&dir)).unwrap();
        assert!(!raw.contains("null"), "null を書き出している: {raw}");
    }
}
