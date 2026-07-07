//! セッションログ。受信バイトを端末供給前にタップして記録する。
//! 生バイト版（.log、再生用）＋任意でプレーンテキスト版（.txt、エスケープ除去＋行頭タイムスタンプ）。

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct LogTap {
    raw: BufWriter<File>,
    plain: Option<PlainWriter>,
}

struct PlainWriter {
    out: BufWriter<File>,
    parser: mot_term::parser::Parser,
    line_start: bool,
}

impl LogTap {
    /// {dir}/{profile}-{stamp}.log を開く。plaintext=true なら .txt も。
    /// stamp は呼び出し側で決める（例: 起動時刻の文字列）。
    pub fn create(
        dir: &Path,
        profile: &str,
        stamp: &str,
        plaintext: bool,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let base = format!("{}-{}", sanitize(profile), stamp);
        let raw = BufWriter::new(File::create(dir.join(format!("{base}.log")))?);
        let plain = if plaintext {
            let f = File::create(dir.join(format!("{base}.txt")))?;
            Some(PlainWriter {
                out: BufWriter::new(f),
                parser: mot_term::parser::Parser::new(),
                line_start: true,
            })
        } else {
            None
        };
        Ok(LogTap { raw, plain })
    }

    pub fn write_raw(&mut self, data: &[u8]) {
        let _ = self.raw.write_all(data);
        let _ = self.raw.flush();
        if let Some(p) = self.plain.as_mut() {
            p.feed(data);
        }
    }
}

impl PlainWriter {
    fn feed(&mut self, data: &[u8]) {
        // パーサでエスケープを剥がし、印字文字と改行だけを取り出す
        let mut sink = PlainSink {
            out: &mut self.out,
            line_start: &mut self.line_start,
        };
        self.parser.advance(&mut sink, data);
    }
}

struct PlainSink<'a> {
    out: &'a mut BufWriter<File>,
    line_start: &'a mut bool,
}

impl mot_term::parser::VtHandler for PlainSink<'_> {
    fn print(&mut self, ch: char) {
        if *self.line_start {
            let _ = write!(self.out, "{} ", timestamp());
            *self.line_start = false;
        }
        let mut b = [0u8; 4];
        let _ = self.out.write_all(ch.encode_utf8(&mut b).as_bytes());
    }
    fn execute(&mut self, byte: u8) {
        if byte == b'\n' {
            let _ = self.out.write_all(b"\n");
            let _ = self.out.flush();
            *self.line_start = true;
        }
    }
    fn csi(&mut self, _: &mot_term::parser::Params, _: Option<u8>, _: &[u8], _: u8) {}
    fn esc(&mut self, _: &[u8], _: u8) {}
    fn osc(&mut self, _: &[u8]) {}
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 行頭タイムスタンプ。Date 依存を避け、単調増加の相対秒でも良いが、
/// ここでは簡易に SystemTime を使う（ログ用途で決定性は不要）。
fn timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("[{secs}]")
}
