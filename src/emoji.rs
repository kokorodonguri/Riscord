//! 絵文字の入力（:名前: と <:名前:id> の変換、検索）。UI には依存しない
use crate::app::{parse_emoji, split_emoji, Seg};

/// ピッカーや補完で選べる絵文字
#[derive(Clone, PartialEq, Debug)]
pub enum Pick {
    Custom {
        name: String,
        id: String,
        animated: bool,
    },
    /// 標準の絵文字（ch = 文字そのもの、name = ショートコード。無ければ英語名）
    Unicode {
        ch: &'static str,
        name: &'static str,
    },
}

impl Pick {
    pub fn name(&self) -> &str {
        match self {
            Pick::Custom { name, .. } => name,
            Pick::Unicode { name, .. } => name,
        }
    }

    /// 入力欄に入れる文字（カスタムは :名前: にして、送るときに <:名前:id> へ直す）
    pub fn insert_text(&self) -> String {
        match self {
            Pick::Custom { name, .. } => format!(":{name}: "),
            Pick::Unicode { ch, .. } => ch.to_string(),
        }
    }
}

/// 標準の絵文字の分類（ピッカーの見出し）
pub fn group_label(g: emojis::Group) -> &'static str {
    use emojis::Group as G;
    match g {
        G::SmileysAndEmotion => tr!("スマイリー", "Smileys"),
        G::PeopleAndBody => tr!("人", "People"),
        G::AnimalsAndNature => tr!("動物・自然", "Animals & Nature"),
        G::FoodAndDrink => tr!("食べ物・飲み物", "Food & Drink"),
        G::TravelAndPlaces => tr!("旅行・場所", "Travel & Places"),
        G::Activities => tr!("活動", "Activities"),
        G::Objects => tr!("物", "Objects"),
        G::Symbols => tr!("記号", "Symbols"),
        G::Flags => tr!("旗", "Flags"),
    }
}

/// 標準の絵文字を分類の順に（新しすぎてフォントに無さそうなものは除く）
pub fn standard() -> impl Iterator<Item = (emojis::Group, Pick)> {
    emojis::iter()
        .filter(|e| e.unicode_version().major() <= 15)
        .map(|e| {
            (
                e.group(),
                Pick::Unicode {
                    ch: e.as_str(),
                    name: e.shortcode().unwrap_or(e.name()),
                },
            )
        })
}

/// 名前で探す。前方一致を先に、部分一致を後に並べる（custom が先、標準が後）。
/// custom は（名前, id, アニメか）。数千件になるので、当たったものだけ Pick にする（打つたびに全件を複製しない）
pub fn search<'a>(
    query: &str,
    custom: impl Iterator<Item = (&'a str, &'a str, bool)>,
    limit: usize,
) -> Vec<Pick> {
    let q = query.to_lowercase();
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    let mut seen = std::collections::HashSet::new();
    let put = |rank: Option<usize>, p: Pick, head: &mut Vec<Pick>, tail: &mut Vec<Pick>| match rank
    {
        Some(0) => head.push(p),
        Some(_) => tail.push(p),
        None => {}
    };
    for (name, id, animated) in custom {
        let rank = name.to_lowercase().find(&q);
        if rank.is_none() || !seen.insert(id) {
            continue;
        }
        put(
            rank,
            Pick::Custom {
                name: name.to_string(),
                id: id.to_string(),
                animated,
            },
            &mut head,
            &mut tail,
        );
    }
    for e in emojis::iter().filter(|e| e.unicode_version().major() <= 15) {
        // ショートコードが複数あるものは、いちばん良く当たったもので並べる
        let rank = e
            .shortcodes()
            .filter_map(|s| s.find(&q))
            .min()
            .or_else(|| e.name().to_lowercase().find(&q).map(|i| i + 1));
        put(
            rank,
            Pick::Unicode {
                ch: e.as_str(),
                name: e.shortcode().unwrap_or(e.name()),
            },
            &mut head,
            &mut tail,
        );
    }
    head.extend(tail);
    head.truncate(limit);
    head
}

fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '+' | '-')
}

/// カーソルの直前が「:xx」（打ちかけの絵文字名）なら、その始まりの位置と名前を返す
pub fn token_before(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let head = text.get(..cursor)?;
    let start = head.rfind(':')?;
    let name = &head[start + 1..];
    // 「12:30」や「:name:」の閉じる側で出さないよう、: の前は空白か行頭だけ
    let before_ok = head[..start]
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace());
    (before_ok && name.chars().count() >= 2 && name.chars().all(name_char)).then_some((start, name))
}

/// 本文の :名前: を、lookup が返した文字（<:名前:id> や標準の絵文字）に置き換える。
/// コード（` や ```）の中と、既に <:名前:id> になっている所は触らない
pub fn expand(content: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(content.len());
    let mut i = 0;
    while i < content.len() {
        let rest = &content[i..];
        let c = rest.chars().next().unwrap();
        match c {
            '`' => {
                // 同じ数の ` で閉じていれば、そこまでそのまま写す
                let n = rest.bytes().take_while(|b| *b == b'`').count();
                let fence = &rest[..n];
                let len = rest[n..].find(fence).map_or(n, |end| n + end + n);
                out.push_str(&rest[..len]);
                i += len;
            }
            '<' => {
                let len = parse_emoji(rest).map_or(1, |(len, _)| len);
                out.push_str(&rest[..len]);
                i += len;
            }
            ':' => {
                let name_len = rest[1..]
                    .find(|c: char| !name_char(c))
                    .unwrap_or(rest.len() - 1);
                let name = &rest[1..1 + name_len];
                let closed = rest[1 + name_len..].starts_with(':');
                match (closed && !name.is_empty()).then(|| lookup(name)).flatten() {
                    Some(to) => {
                        out.push_str(&to);
                        i += name_len + 2;
                    }
                    None => {
                        out.push(':');
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// 編集用に <:名前:id> を :名前: に戻す。元の対応（名前・id・アニメか）も返す
pub fn shorten(content: &str) -> (String, Vec<(String, String, bool)>) {
    let mut out = String::with_capacity(content.len());
    let mut known = Vec::new();
    for seg in split_emoji(content) {
        match seg {
            Seg::Text(t) => out.push_str(t),
            Seg::Emoji { name, id, animated } => {
                out.push_str(&format!(":{name}:"));
                known.push((name.to_string(), id.to_string(), animated));
            }
        }
    }
    (out, known)
}

/// <:名前:id> / <a:名前:id> の形にする
pub fn markup(name: &str, id: &str, animated: bool) -> String {
    format!("<{}:{name}:{id}>", if animated { "a" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "pog" => Some(markup("pog", "123", false)),
            "dance" => Some(markup("dance", "456", true)),
            _ => emojis::get_by_shortcode(name).map(|e| e.as_str().to_string()),
        }
    }

    #[test]
    fn expand_replaces_known_names_only() {
        assert_eq!(
            expand("hi :pog: and :dance:!", lookup),
            "hi <:pog:123> and <a:dance:456>!"
        );
        assert_eq!(expand(":pog::pog:", lookup), "<:pog:123><:pog:123>");
        assert_eq!(expand("10:30 :nope: :pog", lookup), "10:30 :nope: :pog");
        assert_eq!(expand(":+1: 日本語:pog:", lookup), "👍 日本語<:pog:123>");
    }

    #[test]
    fn expand_keeps_code_and_existing_markup() {
        assert_eq!(
            expand("`:pog:` :pog: ```\n:pog:\n```", lookup),
            "`:pog:` <:pog:123> ```\n:pog:\n```"
        );
        assert_eq!(
            expand("<:pog:999> <a:dance:1> <@12> :pog:", lookup),
            "<:pog:999> <a:dance:1> <@12> <:pog:123>"
        );
        // 閉じていない ` はコードにならない
        assert_eq!(expand("it`s :pog:", lookup), "it`s <:pog:123>");
    }

    #[test]
    fn shorten_round_trips_through_expand() {
        let original = "a <:pog:999> b <a:dance:1> c";
        let (short, known) = shorten(original);
        assert_eq!(short, "a :pog: b :dance: c");
        let back = expand(&short, |n| {
            known
                .iter()
                .find(|k| k.0 == n)
                .map(|(n, id, a)| markup(n, id, *a))
        });
        assert_eq!(back, original);
    }

    #[test]
    fn token_before_finds_the_name_being_typed() {
        assert_eq!(token_before("hi :po", 6), Some((3, "po")));
        assert_eq!(token_before(":po", 3), Some((0, "po")));
        assert_eq!(token_before("hi :p", 5), None);
        assert_eq!(token_before("10:30", 5), None);
        assert_eq!(token_before(":pog:", 5), None);
        assert_eq!(token_before(":po g", 5), None);
        assert_eq!(
            token_before("あ :ねこ", "あ :ねこ".len()),
            Some((4, "ねこ"))
        );
    }

    #[test]
    fn search_puts_prefix_matches_and_custom_first() {
        let custom = [("catjam", "1", false), ("blobcat", "2", false)];
        let hits = search("cat", custom.into_iter(), 5);
        assert_eq!(hits[0].name(), "catjam");
        assert!(matches!(hits[1], Pick::Unicode { .. }), "{hits:?}");
        assert!(hits.iter().any(|p| p.name() == "cat" || p.name() == "cat2"));
        assert_eq!(hits.len(), 5);
    }
}
