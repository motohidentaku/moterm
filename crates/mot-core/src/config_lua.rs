//! moterm.lua の評価と model::Config への変換。
//!
//! 仕様の正: moterm/docs/moterm.lua.example・design.md §3。
//! - `require 'moterm'` が動くように package.preload へ `moterm` モジュールを登録する
//!   （`moterm.config()` = 空テーブル、`moterm.font(name)` = name をそのまま返す）。
//! - 評価結果（return されたテーブル）を serde 経由で Config に変換する。
//! - 文法・型エラーは Err（呼び出し側が旧設定を維持できるように）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use crate::model::Config;

/// 設定ファイル名
pub const CONFIG_FILE: &str = "moterm.lua";

/// ユーザ既定の設定ディレクトリ。
/// Windows: `%APPDATA%\moterm` / それ以外: `$XDG_CONFIG_HOME/moterm` または `~/.config/moterm`。
pub fn default_config_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            if !appdata.is_empty() {
                return PathBuf::from(appdata).join("moterm");
            }
        }
    } else if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("moterm");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("moterm")
}

/// moterm.lua の探索。優先順: 引数パス > 実行ファイルと同じディレクトリ > ユーザ既定の場所。
/// 引数パスは存在確認せずそのまま返す（読めなければ `load_config` が Err にする）。
pub fn resolve_config_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join(CONFIG_FILE);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let p = default_config_dir().join(CONFIG_FILE);
    if p.is_file() {
        return Some(p);
    }
    None
}

/// profiles.json・ボールト等を置くディレクトリ（= 解決された moterm.lua と同じ場所）。
/// 設定ファイルが見つからなかった場合はユーザ既定の設定ディレクトリ。
pub fn config_dir(resolved: Option<&Path>) -> PathBuf {
    resolved
        .and_then(|p| p.parent())
        .filter(|d| !d.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(default_config_dir)
}

/// moterm.lua を読み込んで Config へ変換する。
/// どこにも設定ファイルが無ければ内蔵デモ（`Config::default()`、プロファイル0件）。
/// 明示パスが読めない場合・Lua エラー・型エラーは Err（メッセージに Lua のエラー内容を含む）。
pub fn load_config(path: Option<&Path>) -> Result<Config> {
    match resolve_config_path(path) {
        Some(p) => load_config_file(&p),
        None => Ok(Config::default()),
    }
}

/// 指定ファイルを評価して Config へ変換する。
pub fn load_config_file(path: &Path) -> Result<Config> {
    // 非 UTF-8 パスでも落ちないよう Path のまま読む（表示用のみ lossy 変換）。
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("設定ファイルを読み込めません: {}", path.display()))?;
    eval_config(&source, &path.to_string_lossy())
}

/// Lua ソースを評価して Config へ変換する（chunk_name はエラー表示用）。
pub fn eval_config(source: &str, chunk_name: &str) -> Result<Config> {
    let lua = Lua::new();
    register_moterm_module(&lua)
        .map_err(|e| anyhow::anyhow!("moterm モジュールの登録に失敗: {e}"))?;
    let value: Value = lua
        .load(source)
        .set_name(chunk_name)
        .eval()
        .map_err(|e| anyhow::anyhow!("{chunk_name} の評価に失敗: {e}"))?;
    // 関数など serde 化できない値は無視する（設定テーブルに紛れても致命傷にしない）
    let options = mlua::DeserializeOptions::new().deny_unsupported_types(false);
    let config: Config = lua
        .from_value_with(value, options)
        .map_err(|e| anyhow::anyhow!("{chunk_name} の設定形式が不正: {e}"))?;
    Ok(config)
}

/// `require 'moterm'` 用のモジュールを package.preload に登録する。
fn register_moterm_module(lua: &Lua) -> mlua::Result<()> {
    let package: mlua::Table = lua.globals().get("package")?;
    let preload: mlua::Table = package.get("preload")?;
    let loader = lua.create_function(|lua, _: MultiValue| {
        let module = lua.create_table()?;
        // moterm.config() -> 空テーブル
        module.set("config", lua.create_function(|lua, ()| lua.create_table())?)?;
        // moterm.font(name) -> name をそのまま返す
        module.set("font", lua.create_function(|_, name: Value| Ok(name))?)?;
        Ok(module)
    })?;
    preload.set("moterm", loader)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AuthMethod;

    const SAMPLE: &str = r#"
        local moterm = require 'moterm'
        local config = moterm.config()
        config.font = moterm.font('HackGen Console NF')
        config.font_size = 12.5
        config.lang = 'ja'
        config.keys = { { key = 'j', mods = 'CTRL', action = 'next_tab' } }
        config.profiles = {
          {
            name = 'web', group = 'production', host = '10.0.0.10', port = 2222,
            user = 'deploy',
            auth = { method = 'publickey', key = '~/.ssh/id_ed25519' },
            port_forwards = {
              { type = 'local', listen = '127.0.0.1:8080', target = '127.0.0.1:80' },
            },
            on_connect = { 'cd /var/www', 'tail -F log/app.log' },
            reconnect = { enabled = true, max_retries = 5, backoff_sec = 3 },
          },
          { name = 'db', host = '10.0.0.20', user = 'admin',
            auth = { method = 'password', save = true } },
          { name = 'laptop', host = '192.168.1.2' },
        }
        config.groups = { { name = 'production', label = '本番環境' } }
        return config
    "#;

    #[test]
    fn evaluates_example_config() {
        let cfg = eval_config(SAMPLE, "test.lua").unwrap();
        assert_eq!(cfg.font.as_deref(), Some("HackGen Console NF"));
        assert_eq!(cfg.font_size, 12.5);
        assert_eq!(cfg.lang.as_deref(), Some("ja"));
        assert_eq!(cfg.profiles.len(), 3);
        let web = &cfg.profiles[0];
        assert_eq!(web.name, "web");
        assert_eq!(web.port, 2222);
        assert_eq!(
            web.auth,
            AuthMethod::Publickey {
                key: "~/.ssh/id_ed25519".into()
            }
        );
        assert_eq!(web.port_forwards.len(), 1);
        assert_eq!(web.on_connect.len(), 2);
        assert!(web.reconnect.as_ref().unwrap().enabled);
        // 省略時の既定値
        assert_eq!(cfg.profiles[1].port, 22);
        assert_eq!(cfg.profiles[1].auth, AuthMethod::Password { save: true });
        assert_eq!(cfg.profiles[2].auth, AuthMethod::Agent);
        assert!(cfg.profiles[2].on_connect_on_reconnect);
        // groups
        assert_eq!(cfg.groups.len(), 1);
        assert_eq!(cfg.groups[0].label.as_deref(), Some("本番環境"));
        assert_eq!(cfg.groups[0].connect, "parallel");
        // keys
        assert_eq!(cfg.keys.len(), 1);
        assert_eq!(cfg.keys[0].action, "next_tab");
    }

    #[test]
    fn minimal_config_uses_defaults() {
        let cfg = eval_config("return (require 'moterm').config()", "min.lua").unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.font_size, 14.0);
        assert_eq!(cfg.scrollback_lines, 1000);
        assert_eq!(cfg.secret_store, "encrypted-file");
        assert!(cfg.use_ime);
    }

    #[test]
    fn syntax_error_is_err_with_lua_message() {
        let err = eval_config("this is not lua !!", "broken.lua").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("broken.lua"), "message was: {msg}");
    }

    #[test]
    fn runtime_error_is_err() {
        assert!(eval_config("error('boom')", "boom.lua").is_err());
    }

    #[test]
    fn type_error_is_err() {
        // port に文字列 → 変換エラー
        let src = "return { profiles = { { name='x', host='h', port='abc' } } }";
        assert!(eval_config(src, "bad.lua").is_err());
    }

    #[test]
    fn load_config_from_explicit_path() {
        let dir = crate::test_util::temp_dir("config_lua");
        let path = dir.join("moterm.lua");
        std::fs::write(&path, "return { profiles = { { name='a', host='b' } } }").unwrap();
        let cfg = load_config(Some(&path)).unwrap();
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.profiles[0].host, "b");
    }

    #[test]
    fn load_config_missing_explicit_path_is_err() {
        let dir = crate::test_util::temp_dir("config_lua_missing");
        assert!(load_config(Some(&dir.join("nope.lua"))).is_err());
    }

    #[test]
    fn config_dir_follows_resolved_path() {
        let dir = crate::test_util::temp_dir("config_dir");
        let path = dir.join("moterm.lua");
        assert_eq!(config_dir(Some(&path)), dir);
        assert_eq!(config_dir(None), default_config_dir());
    }
}
