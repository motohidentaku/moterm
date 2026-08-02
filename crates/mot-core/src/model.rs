//! プロファイル・設定のデータモデル。
//! moterm.lua / profiles.json の両方からこの型に正規化される（docs/moterm.lua.example が仕様の正）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase", tag = "method")]
pub enum AuthMethod {
    /// SSH エージェント（既定）
    #[default]
    Agent,
    Publickey {
        /// 鍵ファイルパス（~ 展開前）。OpenSSH/PEM/.ppk(未暗号化)
        key: String,
    },
    Password {
        /// true なら認証成功時に暗号化ボールトへ保存
        #[serde(default)]
        save: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ForwardType {
    #[default]
    Local, // -L
    Remote, // -R
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortForward {
    #[serde(rename = "type", default)]
    pub kind: ForwardType,
    /// "127.0.0.1:8080" 形式
    pub listen: String,
    /// "127.0.0.1:80" 形式
    pub target: String,
    /// true なら確立失敗で接続自体を失敗扱い（既定 false = 警告して継続）
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconnectCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_backoff_sec")]
    pub backoff_sec: u64,
}

fn default_max_retries() -> u32 {
    5
}
fn default_backoff_sec() -> u64 {
    3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeepaliveCfg {
    /// キープアライブ送信間隔（秒）。0 で無効。省略時は既定 60。
    #[serde(default = "default_keepalive_interval")]
    pub interval_sec: u64,
    /// 無応答のキープアライブがこの回数連続したら切断とみなす（russh keepalive_max）。
    /// 回線が不安定な環境では増やすと寛容になる。省略時は russh 既定と同じ 3。
    #[serde(default = "default_keepalive_max")]
    pub max: usize,
}

fn default_keepalive_interval() -> u64 {
    60
}
fn default_keepalive_max() -> usize {
    3
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogCfg {
    #[serde(default)]
    pub enabled: bool,
    pub dir: Option<String>,
    /// エスケープ除去＋タイムスタンプ付きの .txt も出力
    #[serde(default)]
    pub plaintext: bool,
}

/// on_connect の送信タイミング
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OnConnectWait {
    #[serde(default = "default_wait_mode")]
    pub mode: String, // "prompt" | "delay"
    #[serde(default = "default_delay_ms")]
    pub delay_ms: u64,
    pub prompt: Option<String>,
}

fn default_wait_mode() -> String {
    "delay".into()
}
fn default_delay_ms() -> u64 {
    700
}

impl Default for OnConnectWait {
    fn default() -> Self {
        OnConnectWait {
            mode: default_wait_mode(),
            delay_ms: default_delay_ms(),
            prompt: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Profile {
    pub name: String,
    #[serde(default)]
    pub group: Option<String>,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default)]
    pub port_forwards: Vec<PortForward>,
    #[serde(default)]
    pub on_connect: Vec<String>,
    #[serde(default)]
    pub on_connect_wait: Option<OnConnectWait>,
    /// 再接続時に on_connect を再実行（既定 true）
    #[serde(default = "default_true")]
    pub on_connect_on_reconnect: bool,
    #[serde(default)]
    pub proxy_jump: Option<String>,
    #[serde(default)]
    pub proxy_command: Option<String>,
    #[serde(default)]
    pub reconnect: Option<ReconnectCfg>,
    #[serde(default)]
    pub keepalive: Option<KeepaliveCfg>,
    #[serde(default)]
    pub log: Option<LogCfg>,
    #[serde(default)]
    pub color_scheme: Option<String>,
    /// moterm.lua 由来（読み取り専用）か GUI 管理（profiles.json）か
    #[serde(skip)]
    pub read_only: bool,
}

fn default_port() -> u16 {
    22
}
fn default_true() -> bool {
    true
}

impl Profile {
    /// user 未指定時は環境のユーザ名を使う
    pub fn effective_user(&self) -> String {
        self.user.clone().unwrap_or_else(|| {
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "root".into())
        })
    }
    /// ボールトのキー "user@host:port"
    pub fn secret_key(&self) -> String {
        format!("{}@{}:{}", self.effective_user(), self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupCfg {
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    /// "parallel"(既定) | "sequential"
    #[serde(default = "default_connect")]
    pub connect: String,
    #[serde(default)]
    pub order: Option<Vec<String>>,
}

fn default_connect() -> String {
    "parallel".into()
}

/// キーバインド1件（config.keys）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default)]
    pub mods: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WindowCfg {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub menu_width: Option<u32>,
    pub menu_height: Option<u32>,
    #[serde(default = "default_true")]
    pub title_bar: bool,
}

/// リモートのシステムメトリクス採取設定（情報パネルの System 表示）。
///
/// 注意: `derive(Default)` は使わない。フィールド個別の serde default と
/// 食い違い、テーブルごと省略した場合だけ無効になる罠を避けるため手書きする。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsCfg {
    /// false なら採取タスクを起動しない
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 採取間隔（分）。0 なら採取しない。
    #[serde(default = "default_metrics_interval_min")]
    pub interval_min: u32,
    /// 右の情報パネルの既定の表示状態（F6 で切替）。
    /// メトリクス以外（ホスト情報・エージェント・認証）も載るため、
    /// `enabled = false` でもパネル自体はこの設定で出せる。
    #[serde(default = "default_true")]
    pub panel: bool,
}

fn default_metrics_interval_min() -> u32 {
    1
}

impl Default for MetricsCfg {
    fn default() -> Self {
        MetricsCfg {
            enabled: true,
            interval_min: default_metrics_interval_min(),
            panel: true,
        }
    }
}

impl MetricsCfg {
    /// 実際の採取間隔。無効なら None。
    pub fn interval(&self) -> Option<std::time::Duration> {
        if !self.enabled || self.interval_min == 0 {
            return None;
        }
        Some(std::time::Duration::from_secs(
            self.interval_min as u64 * 60,
        ))
    }
}

/// アプリ全体設定（moterm.lua 相当）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub groups: Vec<GroupCfg>,
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default)]
    pub font_fallback: Vec<String>,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default)]
    pub color_scheme: Option<String>,
    #[serde(default = "default_scrollback")]
    pub scrollback_lines: usize,
    #[serde(default)]
    pub window: WindowCfg,
    #[serde(default = "default_opacity")]
    pub window_background_opacity: f32,
    #[serde(default)]
    pub window_blur: u32,
    /// 背景画像のパス（~展開はGUI側）。指定時、端末/UI の背後に薄く敷く。
    #[serde(default)]
    pub background_image: Option<String>,
    /// 背景画像を敷く強さ（0.0..=1.0）。既定 0.25 で暗めに透かす。
    #[serde(default = "default_bg_image_opacity")]
    pub background_image_opacity: f32,
    #[serde(default = "default_true")]
    pub use_ime: bool,
    /// 'ja' | 'en' | None(=OSロケール判定)
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub download_dir: Option<String>,
    /// 'encrypted-file'(既定) | 'none'
    #[serde(default = "default_secret_store")]
    pub secret_store: String,
    #[serde(default)]
    pub secret_file: Option<String>,
    #[serde(default)]
    pub keys: Vec<KeyBinding>,
    /// East Asian Ambiguous を全角扱いにする（既定 false）
    #[serde(default)]
    pub treat_east_asian_ambiguous_width_as_wide: bool,
    /// UI テーマ: 'classic'(既定) | 'neo'（未来的3カラムUI）。
    #[serde(default)]
    pub ui: Option<String>,
    /// リモートのシステムメトリクス採取（NEO-UI の情報パネル）
    #[serde(default)]
    pub metrics: MetricsCfg,
}

fn default_font_size() -> f32 {
    14.0
}
fn default_scrollback() -> usize {
    1000
}
fn default_opacity() -> f32 {
    1.0
}
fn default_bg_image_opacity() -> f32 {
    0.25
}
fn default_secret_store() -> String {
    "encrypted-file".into()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            profiles: Vec::new(),
            groups: Vec::new(),
            font: None,
            font_fallback: Vec::new(),
            font_size: default_font_size(),
            color_scheme: None,
            scrollback_lines: default_scrollback(),
            window: WindowCfg::default(),
            window_background_opacity: default_opacity(),
            window_blur: 0,
            background_image: None,
            background_image_opacity: default_bg_image_opacity(),
            use_ime: true,
            lang: None,
            download_dir: None,
            secret_store: default_secret_store(),
            secret_file: None,
            keys: Vec::new(),
            treat_east_asian_ambiguous_width_as_wide: false,
            ui: None,
            metrics: MetricsCfg::default(),
        }
    }
}

/// `~` をホームディレクトリに展開する
pub fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    std::path::PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_cfg_uses_defaults_when_fields_omitted() {
        // keepalive を空オブジェクトで書いても既定（60秒間隔・max 3）で埋まる。
        let cfg: KeepaliveCfg = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.interval_sec, 60);
        assert_eq!(cfg.max, 3);
    }

    #[test]
    fn keepalive_cfg_respects_explicit_values() {
        let cfg: KeepaliveCfg = serde_json::from_str(r#"{"interval_sec": 15, "max": 6}"#).unwrap();
        assert_eq!(cfg.interval_sec, 15);
        assert_eq!(cfg.max, 6);
    }

    #[test]
    fn keepalive_cfg_interval_zero_disables() {
        // 明示的に 0 を指定すれば無効化できる（max は既定のまま）。
        let cfg: KeepaliveCfg = serde_json::from_str(r#"{"interval_sec": 0}"#).unwrap();
        assert_eq!(cfg.interval_sec, 0);
        assert_eq!(cfg.max, 3);
    }
}
