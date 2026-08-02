//! mot-core — 設定(Lua)・profiles.json・ランチャーモデル・暗号ボールト・ppk 変換・i18n。
//! GUI/SSH 非依存でユニットテスト可能。

pub mod config_lua;
pub mod i18n;
pub mod keymap;
pub mod launcher;
pub mod metrics;
pub mod model;
pub mod ppk;
pub mod store;
pub mod ui_state;
pub mod vault;

pub use model::{
    AuthMethod, Config, ForwardType, GroupCfg, KeepaliveCfg, KeyBinding, LogCfg, MetricsCfg,
    OnConnectWait, PortForward, Profile, ReconnectCfg, WindowCfg,
};

#[cfg(test)]
pub(crate) mod test_util {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// テスト用の一意な一時ディレクトリを作成する（プロセス終了時に OS 任せで消える）。
    pub fn temp_dir(tag: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("mot-core-test-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir 作成");
        dir
    }
}
