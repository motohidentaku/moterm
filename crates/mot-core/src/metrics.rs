//! リモートホストのシステムメトリクス（CPU/メモリ/ディスク）の取得コマンドとパーサ。
//!
//! SSH・GUI 非依存の純粋な文字列処理に閉じてある（取得は mot-ssh の exec、
//! 定期実行と描画は mot-gui が持つ）。時刻もここでは扱わない。
//!
//! CPU は瞬間値を出すため、リモート側で 1 秒あけて `/proc/stat` を 2 回読む。
//! sleep をリモートで行うので SSH の往復は 1 回で済む。

/// メトリクス採取コマンド（Linux 前提）。出力は
/// `cpu 行 × 2` → `MemTotal/MemAvailable` → `df -Pk /` の順に並ぶ。
///
/// `^cpu ` は末尾スペースにより集約行のみを拾う（cpu0/cpu1… を除外）。
pub const METRICS_COMMAND: &str = "grep '^cpu ' /proc/stat; sleep 1; grep '^cpu ' /proc/stat; \
     grep -E '^Mem(Total|Available):' /proc/meminfo; df -Pk /";

/// `/proc/stat` の集約 cpu 行から取り出した積算時間。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuTimes {
    /// 全モードの積算（jiffies）
    pub total: u64,
    /// アイドル扱いの積算（idle + iowait）
    pub idle: u64,
}

impl CpuTimes {
    /// `cpu  123 456 ...` 形式の 1 行をパースする。フィールドが足りなければ None。
    pub fn parse(line: &str) -> Option<CpuTimes> {
        let mut it = line.split_whitespace();
        if it.next()? != "cpu" {
            return None;
        }
        // user nice system idle iowait irq softirq steal [guest guest_nice]
        // guest 系は user/nice に二重計上されているため先頭 8 個だけを使う。
        let v: Vec<u64> = it.take(8).map_while(|f| f.parse::<u64>().ok()).collect();
        if v.len() < 5 {
            return None; // idle/iowait まで取れないと使えない
        }
        let idle = v[3] + v[4];
        Some(CpuTimes {
            total: v.iter().sum(),
            idle,
        })
    }

    /// 2 サンプルの差分から使用率（0.0..=100.0）を出す。
    /// 差分が取れない（同一・逆転・カウンタリセット）場合は 0.0。
    pub fn usage_pct(prev: &CpuTimes, cur: &CpuTimes) -> f32 {
        let dt = cur.total.saturating_sub(prev.total);
        if dt == 0 {
            return 0.0;
        }
        let di = cur.idle.saturating_sub(prev.idle);
        let busy = dt.saturating_sub(di);
        ((busy as f64 / dt as f64) * 100.0).clamp(0.0, 100.0) as f32
    }
}

/// メモリ使用状況（kB 単位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemStat {
    pub total_kb: u64,
    pub available_kb: u64,
}

impl MemStat {
    /// 使用率（0.0..=100.0）。MemAvailable ベース（cache/buffer を空きとみなす）。
    pub fn used_pct(&self) -> f32 {
        if self.total_kb == 0 {
            return 0.0;
        }
        let used = self.total_kb.saturating_sub(self.available_kb);
        ((used as f64 / self.total_kb as f64) * 100.0).clamp(0.0, 100.0) as f32
    }
    /// 使用中のバイト数
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }
}

/// ルートファイルシステムの使用状況（kB 単位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskStat {
    pub total_kb: u64,
    pub used_kb: u64,
}

impl DiskStat {
    /// 使用率（0.0..=100.0）。df の Capacity 列ではなく blocks/used から計算する
    /// （Capacity は予約ブロックを除いた値で、バーの見た目と食い違うため）。
    pub fn used_pct(&self) -> f32 {
        if self.total_kb == 0 {
            return 0.0;
        }
        ((self.used_kb as f64 / self.total_kb as f64) * 100.0).clamp(0.0, 100.0) as f32
    }
}

/// 1 回の採取で得られるスナップショット。取得時刻は呼び出し側（GUI）が持つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostMetrics {
    pub cpu_pct: f32,
    pub mem: MemStat,
    pub disk: DiskStat,
}

/// `METRICS_COMMAND` の出力をパースする。
///
/// 想定外の行は読み飛ばすため、シェルの警告や motd が混ざっても
/// 必要な行が揃っていれば成功する。1 つでも欠けたら None。
pub fn parse_metrics(out: &str) -> Option<HostMetrics> {
    let mut cpu: Vec<CpuTimes> = Vec::with_capacity(2);
    let mut total_kb: Option<u64> = None;
    let mut available_kb: Option<u64> = None;
    let mut disk: Option<DiskStat> = None;

    for line in out.lines() {
        if let Some(c) = CpuTimes::parse(line) {
            if cpu.len() < 2 {
                cpu.push(c);
            }
        } else if let Some(v) = mem_field(line, "MemTotal:") {
            total_kb = Some(v);
        } else if let Some(v) = mem_field(line, "MemAvailable:") {
            available_kb = Some(v);
        } else if let Some(d) = parse_df_line(line) {
            disk = Some(d);
        }
    }

    if cpu.len() < 2 {
        return None;
    }
    Some(HostMetrics {
        cpu_pct: CpuTimes::usage_pct(&cpu[0], &cpu[1]),
        mem: MemStat {
            total_kb: total_kb?,
            available_kb: available_kb?,
        },
        disk: disk?,
    })
}

/// `MemTotal:  16307180 kB` から数値部分を取り出す。
fn mem_field(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?;
    rest.split_whitespace().next()?.parse().ok()
}

/// `df -Pk /` のデータ行（ヘッダは弾く）。POSIX 形式は
/// `Filesystem 1024-blocks Used Available Capacity Mounted-on` の 6 列固定。
fn parse_df_line(line: &str) -> Option<DiskStat> {
    let f: Vec<&str> = line.split_whitespace().collect();
    if f.len() != 6 || f[5] != "/" {
        return None;
    }
    Some(DiskStat {
        total_kb: f[1].parse().ok()?,
        used_kb: f[2].parse().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実機の出力を模したサンプル（順序・空白もそのまま）。
    const SAMPLE: &str = "\
cpu  1000 100 500 8000 400 0 0 0 0 0
cpu  1100 100 550 8300 450 0 0 0 0 0
MemTotal:       16307180 kB
MemAvailable:    9876543 kB
Filesystem     1024-blocks      Used Available Capacity Mounted on
/dev/sda1         98304000  20000000  73000000      22% /
";

    #[test]
    fn parses_full_sample() {
        let m = parse_metrics(SAMPLE).expect("パース成功");
        // total: 10000 -> 10500 (d=500), idle: 8400 -> 8750 (d=350) => busy 150/500 = 30%
        assert!((m.cpu_pct - 30.0).abs() < 0.01, "cpu_pct={}", m.cpu_pct);
        assert_eq!(m.mem.total_kb, 16307180);
        assert_eq!(m.mem.available_kb, 9876543);
        assert_eq!(m.disk.total_kb, 98304000);
        assert_eq!(m.disk.used_kb, 20000000);
    }

    #[test]
    fn ignores_extra_noise_lines() {
        let noisy = format!("Welcome to Ubuntu\nbash: warning: setlocale\n{SAMPLE}\nlast login\n");
        assert!(parse_metrics(&noisy).is_some());
    }

    #[test]
    fn per_core_lines_are_not_counted_as_samples() {
        // "cpu0" は集約行ではないので CpuTimes::parse が弾く
        assert!(CpuTimes::parse("cpu0 1 2 3 4 5 6 7 8").is_none());
        assert!(CpuTimes::parse("cpu  1 2 3 4 5 6 7 8").is_some());
    }

    #[test]
    fn missing_second_sample_fails() {
        let one = "cpu  1000 100 500 8000 400 0 0 0\nMemTotal: 100 kB\nMemAvailable: 50 kB\n\
                   /dev/sda1 100 20 80 20% /";
        assert!(parse_metrics(one).is_none());
    }

    #[test]
    fn missing_mem_fails() {
        let no_mem = "cpu  1000 100 500 8000 400 0 0 0\ncpu  1100 100 550 8300 450 0 0 0\n\
                      /dev/sda1 100 20 80 20% /";
        assert!(parse_metrics(no_mem).is_none());
    }

    #[test]
    fn missing_df_fails() {
        let no_df = "cpu  1000 100 500 8000 400 0 0 0\ncpu  1100 100 550 8300 450 0 0 0\n\
                     MemTotal: 100 kB\nMemAvailable: 50 kB";
        assert!(parse_metrics(no_df).is_none());
    }

    #[test]
    fn df_header_is_not_mistaken_for_data() {
        assert!(
            parse_df_line("Filesystem 1024-blocks Used Available Capacity Mounted on").is_none()
        );
        // 他のマウントポイントは拾わない
        assert!(parse_df_line("/dev/sdb1 100 20 80 20% /home").is_none());
        assert!(parse_df_line("/dev/sda1 100 20 80 20% /").is_some());
    }

    #[test]
    fn cpu_usage_edge_cases() {
        let a = CpuTimes {
            total: 100,
            idle: 50,
        };
        // 差分ゼロ（アイドル完全静止）は 0%
        assert_eq!(CpuTimes::usage_pct(&a, &a), 0.0);
        // カウンタ巻き戻り（再起動）でも panic せず 0%
        let older = CpuTimes {
            total: 50,
            idle: 25,
        };
        assert_eq!(CpuTimes::usage_pct(&a, &older), 0.0);
        // 全部ビジー
        let busy = CpuTimes {
            total: 200,
            idle: 50,
        };
        assert_eq!(CpuTimes::usage_pct(&a, &busy), 100.0);
    }

    #[test]
    fn percentages_from_stats() {
        let mem = MemStat {
            total_kb: 1000,
            available_kb: 250,
        };
        assert_eq!(mem.used_pct(), 75.0);
        assert_eq!(mem.used_kb(), 750);
        let disk = DiskStat {
            total_kb: 400,
            used_kb: 100,
        };
        assert_eq!(disk.used_pct(), 25.0);
        // ゼロ除算しない
        assert_eq!(
            MemStat {
                total_kb: 0,
                available_kb: 0
            }
            .used_pct(),
            0.0
        );
        assert_eq!(
            DiskStat {
                total_kb: 0,
                used_kb: 0
            }
            .used_pct(),
            0.0
        );
    }
}
