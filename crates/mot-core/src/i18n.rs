//! UI 文字列の日英対応（端末内のシェル出力は翻訳しない）。
//!
//! `config.lang = 'ja' | 'en'` の明示指定 > OS ロケール（LC_ALL/LANG に "ja"）> 英語。

/// UI 表示言語
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// 日本語
    Ja,
    /// 英語（既定）
    #[default]
    En,
}

/// 言語を決定する。優先順: 明示指定（'ja'/'en'）> 環境変数 LC_ALL/LANG に "ja" > En。
pub fn detect_lang(explicit: Option<&str>) -> Lang {
    detect_lang_from(
        explicit,
        std::env::var("LC_ALL").ok().as_deref(),
        std::env::var("LANG").ok().as_deref(),
    )
}

fn detect_lang_from(explicit: Option<&str>, lc_all: Option<&str>, lang: Option<&str>) -> Lang {
    match explicit.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("ja") => return Lang::Ja,
        Some("en") => return Lang::En,
        _ => {}
    }
    for v in [lc_all, lang].into_iter().flatten() {
        if v.to_ascii_lowercase().contains("ja") {
            return Lang::Ja;
        }
    }
    Lang::En
}

/// キーに対応する UI 文字列を返す。未知キーはキー自身をそのまま返す
/// （新キー追加時に UI が空文字にならないためのフォールバック）。
pub fn tr(lang: Lang, key: &str) -> &str {
    let (ja, en): (&'static str, &'static str) = match key {
        // ランチャー
        "search" => ("検索", "Search"),
        "search_placeholder" => ("検索（あいまい一致）", "Search (fuzzy)"),
        "groups" => ("グループ", "Groups"),
        "hosts" => ("ホスト", "Hosts"),
        "all" => ("すべて", "All"),
        "connect" => ("接続", "Connect"),
        "quick_connect_hint" => (
            "Ctrl+Enter: [user@]host[:port] へ直接接続",
            "Ctrl+Enter: connect to [user@]host[:port]",
        ),
        // 接続管理（追加/編集/削除）
        "add" => ("追加", "Add"),
        "edit" => ("編集", "Edit"),
        "delete" => ("削除", "Delete"),
        "delete_confirm" => (
            "削除します。もう一度 Delete で確定",
            "Press Delete again to confirm removal",
        ),
        "readonly_profile" => (
            "moterm.lua 由来のため編集できません",
            "Defined in moterm.lua (read-only)",
        ),
        // 編集フォーム
        "name" => ("名前", "Name"),
        "host" => ("ホスト", "Host"),
        "port" => ("ポート", "Port"),
        "user" => ("ユーザ", "User"),
        "group" => ("グループ", "Group"),
        "auth" => ("認証", "Auth"),
        "key_path" => ("鍵ファイル", "Key file"),
        "save" => ("保存", "Save"),
        "cancel" => ("キャンセル", "Cancel"),
        // 認証ダイアログ
        "password_prompt" => ("パスワード:", "Password:"),
        "passphrase_prompt" => ("鍵のパスフレーズ:", "Key passphrase:"),
        "master_prompt" => ("マスターパスワード:", "Master password:"),
        "master_new_prompt" => (
            "新しいマスターパスワードを設定:",
            "Set a new master password:",
        ),
        // ホスト鍵検証（TOFU）
        "tofu_title" => ("未知のホスト鍵", "Unknown host key"),
        "tofu_question" => (
            "このホスト鍵を信頼して続行しますか？",
            "Trust this host key and continue?",
        ),
        // ファイルマネージャ / 転送
        "local" => ("ローカル", "Local"),
        "remote" => ("リモート", "Remote"),
        "overwrite_confirm" => ("上書きしますか？", "Overwrite?"),
        "download" => ("ダウンロード", "Download"),
        "upload" => ("アップロード", "Upload"),
        "failed" => ("失敗", "Failed"),
        "queued" => ("待機中", "Queued"),
        // 接続状態
        "reconnecting" => ("再接続中…", "Reconnecting..."),
        "reconnect_hint" => ("F5 で再接続", "Press F5 to reconnect"),
        "disconnected" => ("切断されました", "Disconnected"),
        // パネル・その他
        "pf_panel_title" => ("ポートフォワード", "Port forwards"),
        "broadcast_on" => ("ブロードキャスト入力 ON", "Broadcast input ON"),
        "reload" => ("設定を再読込", "Reload config"),
        _ => return key,
    };
    match lang {
        Lang::Ja => ja,
        Lang::En => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_overrides_env() {
        assert_eq!(
            detect_lang_from(Some("ja"), Some("en_US.UTF-8"), None),
            Lang::Ja
        );
        assert_eq!(
            detect_lang_from(Some("en"), Some("ja_JP.UTF-8"), None),
            Lang::En
        );
        assert_eq!(detect_lang_from(Some("JA"), None, None), Lang::Ja); // 大文字小文字無視
    }

    #[test]
    fn env_fallback() {
        assert_eq!(detect_lang_from(None, Some("ja_JP.UTF-8"), None), Lang::Ja);
        assert_eq!(detect_lang_from(None, None, Some("ja_JP.UTF-8")), Lang::Ja);
        assert_eq!(
            detect_lang_from(None, Some("en_US.UTF-8"), Some("ja_JP.UTF-8")),
            Lang::Ja
        );
        assert_eq!(
            detect_lang_from(None, Some("C"), Some("en_US.UTF-8")),
            Lang::En
        );
        assert_eq!(detect_lang_from(None, None, None), Lang::En);
    }

    #[test]
    fn unknown_explicit_falls_through_to_env() {
        assert_eq!(
            detect_lang_from(Some("fr"), Some("ja_JP.UTF-8"), None),
            Lang::Ja
        );
        assert_eq!(detect_lang_from(Some("fr"), None, None), Lang::En);
    }

    #[test]
    fn tr_returns_both_languages() {
        assert_eq!(tr(Lang::Ja, "connect"), "接続");
        assert_eq!(tr(Lang::En, "connect"), "Connect");
        assert_eq!(tr(Lang::Ja, "master_prompt"), "マスターパスワード:");
        assert_eq!(tr(Lang::En, "reconnecting"), "Reconnecting...");
    }

    #[test]
    fn tr_unknown_key_returns_key_itself() {
        assert_eq!(tr(Lang::Ja, "no_such_key"), "no_such_key");
        assert_eq!(tr(Lang::En, "no_such_key"), "no_such_key");
    }

    #[test]
    fn tr_covers_required_keys() {
        // 仕様で列挙された必須キーが全言語で翻訳されていること（キー自身が返らない）
        const REQUIRED: &[&str] = &[
            "search",
            "groups",
            "hosts",
            "connect",
            "add",
            "edit",
            "delete",
            "delete_confirm",
            "readonly_profile",
            "name",
            "host",
            "port",
            "user",
            "group",
            "auth",
            "key_path",
            "save",
            "cancel",
            "password_prompt",
            "passphrase_prompt",
            "master_prompt",
            "master_new_prompt",
            "tofu_title",
            "tofu_question",
            "overwrite_confirm",
            "local",
            "remote",
            "reconnecting",
            "reconnect_hint",
            "pf_panel_title",
            "broadcast_on",
            "search_placeholder",
            "quick_connect_hint",
            "failed",
            "queued",
        ];
        for key in REQUIRED {
            assert_ne!(tr(Lang::En, key), *key, "missing en: {key}");
            assert_ne!(tr(Lang::Ja, key), *key, "missing ja: {key}");
        }
    }
}
