//! Discord の記法（コードブロック・太字・見出しなど）を、描く前の形に分ける。画面には触れない
use std::ops::Range;

use crate::app::find_links;

/// 文字の飾り
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub spoiler: bool,
}

/// 同じ飾りが続く文字
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    /// `#`〜`###` の見出し（1〜3）
    Heading(u8),
    /// `-# ` の小さい文字
    Subtext,
    /// `> ` の引用
    Quote,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// ``` で囲んだコード（言語名は捨てる）
    Code(String),
    Text {
        kind: Kind,
        spans: Vec<Span>,
    },
}

/// 本文を、上から順に並べる塊に分ける
pub fn blocks(s: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find("```") {
        let Some(close) = rest[open + 3..].find("```").map(|i| open + 3 + i) else {
            break;
        };
        let code = fence_body(&rest[open + 3..close]);
        if code.trim().is_empty() {
            break;
        }
        text_blocks(&rest[..open], &mut out);
        out.push(Block::Code(code.replace('\t', "    ")));
        rest = &rest[close + 3..];
    }
    text_blocks(rest, &mut out);
    out
}

/// ``` の中身から、1行目の言語名と前後の改行を除く
fn fence_body(inner: &str) -> &str {
    let body = match inner.split_once('\n') {
        Some((first, after))
            if first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '#' | '.' | '_')) =>
        {
            after
        }
        _ => inner,
    };
    body.trim_matches('\n')
}

fn text_blocks(chunk: &str, out: &mut Vec<Block>) {
    let chunk = chunk.trim_matches('\n');
    if chunk.is_empty() {
        return;
    }
    // 同じ種類が続く行（ふつうの行・引用）は1つの塊にまとめる
    let mut pending: Option<(Kind, String)> = None;
    let mut lines = chunk.split('\n');
    while let Some(line) = lines.next() {
        let (kind, body) = if let Some(first) = line.strip_prefix(">>> ") {
            // ここから最後まで引用
            let mut all = bullet(first);
            for l in lines.by_ref() {
                all.push('\n');
                all.push_str(&bullet(l));
            }
            (Kind::Quote, all)
        } else if let Some(t) = line.strip_prefix("> ") {
            (Kind::Quote, bullet(t))
        } else if let Some(t) = line.strip_prefix("-# ").filter(|t| !t.trim().is_empty()) {
            (Kind::Subtext, t.to_string())
        } else if let Some((level, t)) = heading(line) {
            (Kind::Heading(level), t.to_string())
        } else {
            (Kind::Plain, bullet(line))
        };
        match &mut pending {
            Some((k, text)) if *k == kind && matches!(kind, Kind::Plain | Kind::Quote) => {
                text.push('\n');
                text.push_str(&body);
            }
            _ => {
                flush(pending.take(), out);
                pending = Some((kind, body));
            }
        }
    }
    flush(pending, out);
}

fn flush(pending: Option<(Kind, String)>, out: &mut Vec<Block>) {
    if let Some((kind, text)) = pending {
        // 塊の間の空行は、塊の間隔に任せる
        let text = if kind == Kind::Plain {
            text.trim_matches('\n')
        } else {
            &text
        };
        if !text.is_empty() {
            out.push(Block::Text {
                kind,
                spans: inline(text),
            });
        }
    }
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let level = line.bytes().take_while(|b| *b == b'#').count();
    let t = line[level..].strip_prefix(' ')?;
    ((1..=3).contains(&level) && !t.trim().is_empty()).then_some((level as u8, t.trim()))
}

/// 行頭の `- ` `* ` を箇条書きの点に変える
fn bullet(line: &str) -> String {
    let t = line.trim_start_matches(' ');
    match t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        Some(item) if !item.trim().is_empty() => {
            format!("{}• {item}", &line[..line.len() - t.len()])
        }
        _ => line.to_string(),
    }
}

/// 文中の記法（**太字** *斜体* __下線__ ~~打ち消し~~ ||スポイラー|| `コード`）を解く。
/// 相手のいない記号はそのまま文字として残す
pub fn inline(s: &str) -> Vec<Span> {
    let links = find_links(s);
    let mut out = Vec::new();
    walk(s, 0..s.len(), Style::default(), &links, &mut out);
    out
}

fn push(out: &mut Vec<Span>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(last) if last.style == style => last.text.push_str(text),
        _ => out.push(Span {
            text: text.to_string(),
            style,
        }),
    }
}

/// i から始まる URL の終わり（URL の中の _ や * は記法として扱わない）
fn link_end(links: &[Range<usize>], i: usize, to: usize) -> Option<usize> {
    links
        .iter()
        .find(|l| l.start == i && l.end <= to)
        .map(|l| l.end)
}

/// i から始まる `コード` の（中身の範囲, 終わりの位置）
fn code_at(s: &str, i: usize, to: usize) -> Option<(Range<usize>, usize)> {
    let n = s[i..to].bytes().take_while(|b| *b == b'`').count();
    let start = i + n;
    let close = start + s[start..to].find(&s[i..start])?;
    (close > start).then_some((start..close, close + n))
}

fn escaped(s: &str, i: usize, to: usize) -> bool {
    i + 1 < to && s.as_bytes()[i + 1].is_ascii_punctuation()
}

fn walk(s: &str, range: Range<usize>, style: Style, links: &[Range<usize>], out: &mut Vec<Span>) {
    let to = range.end;
    let (mut i, mut lit) = (range.start, range.start);
    while i < to {
        if let Some(end) = link_end(links, i, to) {
            i = end;
            continue;
        }
        let c = s[i..].chars().next().unwrap();
        match c {
            '\\' if escaped(s, i, to) => {
                push(out, &s[lit..i], style);
                lit = i + 1;
                i += 2;
            }
            '`' => match code_at(s, i, to) {
                Some((body, end)) => {
                    push(out, &s[lit..i], style);
                    let mut code = &s[body];
                    // `` ` `` のように、バッククォートを含めるための前後の空白は落とす
                    if code.len() > 2
                        && code.starts_with(' ')
                        && code.ends_with(' ')
                        && code.contains('`')
                    {
                        code = &code[1..code.len() - 1];
                    }
                    push(
                        out,
                        code,
                        Style {
                            code: true,
                            ..style
                        },
                    );
                    (i, lit) = (end, end);
                }
                None => i += s[i..to].bytes().take_while(|b| *b == b'`').count(),
            },
            '*' | '_' | '~' | '|' => {
                let double = pair(s, i, to, c);
                let single = matches!(c, '*' | '_');
                let found = double
                    .then(|| close(s, i, to, c, 2, links))
                    .flatten()
                    .map(|j| (2, j))
                    .or_else(|| {
                        single
                            .then(|| close(s, i, to, c, 1, links))
                            .flatten()
                            .map(|j| (1, j))
                    });
                match found {
                    Some((n, j)) => {
                        push(out, &s[lit..i], style);
                        let inner = match (c, n) {
                            ('*', 2) => Style {
                                bold: true,
                                ..style
                            },
                            ('_', 2) => Style {
                                underline: true,
                                ..style
                            },
                            ('~', _) => Style {
                                strike: true,
                                ..style
                            },
                            ('|', _) => Style {
                                spoiler: true,
                                ..style
                            },
                            _ => Style {
                                italic: true,
                                ..style
                            },
                        };
                        walk(s, i + n..j, inner, links, out);
                        (i, lit) = (j + n, j + n);
                    }
                    None => i += 1,
                }
            }
            _ => i += c.len_utf8(),
        }
    }
    push(out, &s[lit..to], style);
}

/// i から記号 c が 2 つ続いているか
fn pair(s: &str, i: usize, to: usize, c: char) -> bool {
    s.as_bytes()[i..to].starts_with(&[c as u8; 2])
}

/// i にある開く記号（c を n 個）に対応する、閉じる記号の位置
fn close(s: &str, i: usize, to: usize, c: char, n: usize, links: &[Range<usize>]) -> Option<usize> {
    let start = i + n;
    let next = s[start..to].chars().next()?;
    if n == 1 {
        // 「2 * 3」や snake_case を斜体にしない
        let word_before = s[..i]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        if next.is_whitespace() || (c == '_' && word_before) {
            return None;
        }
    }
    let mut j = start;
    while j < to {
        if let Some(end) = link_end(links, j, to) {
            j = end;
            continue;
        }
        let d = s[j..].chars().next().unwrap();
        if d == '\\' && escaped(s, j, to) {
            j += 2;
        } else if d == '`' {
            j = code_at(s, j, to).map_or(j + 1, |(_, end)| end);
        } else if d == c && n == 2 && pair(s, j, to, c) {
            if j == start {
                j += 1;
                continue;
            }
            // ***太字の斜体*** のように記号が続くときは、後ろの2つで閉じる
            let mut at = j;
            while s[at + 2..to].starts_with(c) {
                at += 1;
            }
            return Some(at);
        } else if d == c && n == 1 {
            if pair(s, j, to, c) {
                // 中にある **太字** や __下線__ の記号は飛ばす
                j += 2;
                continue;
            }
            let space_before = s[..j].chars().next_back().is_some_and(char::is_whitespace);
            let word_after = s[j + 1..].chars().next().is_some_and(char::is_alphanumeric);
            if j > start && !space_before && !(c == '_' && word_after) {
                return Some(j);
            }
            j += 1;
        } else {
            j += d.len_utf8();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 飾りを 1 文字の印にして並べる（b=太字 i=斜体 u=下線 s=打ち消し c=コード h=スポイラー）
    fn show(s: &str) -> String {
        inline(s)
            .iter()
            .map(|sp| {
                let st = sp.style;
                let marks: String = [
                    (st.bold, 'b'),
                    (st.italic, 'i'),
                    (st.underline, 'u'),
                    (st.strike, 's'),
                    (st.code, 'c'),
                    (st.spoiler, 'h'),
                ]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, m)| *m)
                .collect();
                if marks.is_empty() {
                    sp.text.clone()
                } else {
                    format!("<{marks}:{}>", sp.text)
                }
            })
            .collect()
    }

    #[test]
    fn inline_marks() {
        assert_eq!(show("a **b** c"), "a <b:b> c");
        assert_eq!(show("**x** と **y**"), "<b:x> と <b:y>");
        assert_eq!(show("*斜体* と _斜体_"), "<i:斜体> と <i:斜体>");
        assert_eq!(
            show("__下線__ ~~消す~~ ||秘密||"),
            "<u:下線> <s:消す> <h:秘密>"
        );
        assert_eq!(show("***両方***"), "<bi:両方>");
        assert_eq!(show("**太字 *斜体* 太字**"), "<b:太字 ><bi:斜体><b: 太字>");
        assert_eq!(show("`let x = 1;` です"), "<c:let x = 1;> です");
        assert_eq!(show("``a ` b``"), "<c:a ` b>");
    }

    #[test]
    fn unpaired_marks_stay_as_text() {
        assert_eq!(show("2 ** 3"), "2 ** 3");
        assert_eq!(show("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(show("**a** **b"), "<b:a> **b");
        assert_eq!(show("snake_case_name"), "snake_case_name");
        assert_eq!(show("`閉じない"), "`閉じない");
        assert_eq!(show("普通"), "普通");
        assert_eq!(show("a | b || c"), "a | b || c");
    }

    #[test]
    fn code_links_and_escapes_are_left_alone() {
        assert_eq!(show("`**a**`"), "<c:**a**>");
        assert_eq!(
            show("https://example.com/a_b_c と _x_"),
            "https://example.com/a_b_c と <i:x>"
        );
        assert_eq!(show(r"\*a\* ¯\_(ツ)_/¯"), "*a* ¯_(ツ)_/¯");
        assert_eq!(show(r"C:\Users\name"), r"C:\Users\name");
    }

    fn kinds(s: &str) -> Vec<String> {
        blocks(s)
            .iter()
            .map(|b| match b {
                Block::Code(c) => format!("code:{c}"),
                Block::Text { kind, spans } => format!(
                    "{kind:?}:{}",
                    spans.iter().map(|s| s.text.as_str()).collect::<String>()
                ),
            })
            .collect()
    }

    #[test]
    fn code_fences() {
        assert_eq!(
            kinds("前\n```rust\nfn main() {}\n```\n後"),
            ["Plain:前", "code:fn main() {}", "Plain:後"]
        );
        assert_eq!(kinds("```\n  a\n\tb\n```"), ["code:  a\n    b"]);
        assert_eq!(kinds("```1行だけ```"), ["code:1行だけ"]);
        assert_eq!(kinds("```**a**\n_b_```"), ["code:**a**\n_b_"]);
        assert_eq!(kinds("```閉じない"), ["Plain:```閉じない"]);
        assert_eq!(kinds("``````"), ["Plain:``````"]);
    }

    #[test]
    fn block_kinds() {
        assert_eq!(
            kinds("# 大\n## 中\n### 小\n#### 文字"),
            [
                "Heading(1):大",
                "Heading(2):中",
                "Heading(3):小",
                "Plain:#### 文字"
            ]
        );
        assert_eq!(kinds("#タグ\n-# 小さく"), ["Plain:#タグ", "Subtext:小さく"]);
        assert_eq!(
            kinds("> 引用\n> 続き\n本文\n続き"),
            ["Quote:引用\n続き", "Plain:本文\n続き"]
        );
        assert_eq!(kinds("あ\n>>> い\nう"), ["Plain:あ", "Quote:い\nう"]);
        assert_eq!(
            kinds("- 一\n  - 二\n* 三\n-四"),
            ["Plain:• 一\n  • 二\n• 三\n-四"]
        );
        assert_eq!(kinds("あ\n\nい"), ["Plain:あ\n\nい"]);
    }
}
