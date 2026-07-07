//! 自動再接続のバックオフ計算。指数バックオフ＋上限。
//! 実際の再接続オーケストレーション（PF 張り直し・on_connect 再実行）は GUI 側が
//! この計算に従って SshSession::connect を呼び直すことで行う。

use mot_core::model::ReconnectCfg;

/// n 回目（0-based）の再接続までの待機秒を返す。
pub fn backoff_delay(cfg: &ReconnectCfg, attempt: u32) -> u64 {
    // base * 2^attempt、上限 60 秒
    let base = cfg.backoff_sec.max(1);
    base.saturating_mul(1u64 << attempt.min(6)).min(60)
}

/// まだ再試行してよいか。
pub fn should_retry(cfg: &ReconnectCfg, attempt: u32) -> bool {
    cfg.enabled && attempt < cfg.max_retries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> ReconnectCfg {
        ReconnectCfg {
            enabled: true,
            max_retries: 5,
            backoff_sec: 3,
        }
    }

    #[test]
    fn exponential_and_capped() {
        let c = cfg();
        assert_eq!(backoff_delay(&c, 0), 3);
        assert_eq!(backoff_delay(&c, 1), 6);
        assert_eq!(backoff_delay(&c, 2), 12);
        assert_eq!(backoff_delay(&c, 10), 60); // 上限
    }

    #[test]
    fn retry_limit() {
        let c = cfg();
        assert!(should_retry(&c, 0));
        assert!(should_retry(&c, 4));
        assert!(!should_retry(&c, 5));
        let disabled = ReconnectCfg {
            enabled: false,
            ..cfg()
        };
        assert!(!should_retry(&disabled, 0));
    }
}
