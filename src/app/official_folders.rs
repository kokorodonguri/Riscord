//! 公式クライアントのサーバーの並びとフォルダ（READY の user_settings_proto）を読む。
//! 使うのは Riscord にまだ並びもフォルダも保存されていないとき（初回）だけで、
//! Riscord で並べ替えたあとは保存した並びを優先する（READY のたびに上書きしない）。
use super::*;

/// 公式クライアントのフォルダ。id が無く guild が 1 つだけのものはフォルダに入っていないサーバー
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OfficialFolder {
    pub id: Option<u64>,
    pub name: Option<String>,
    pub color: Option<u32>,
    pub guild_ids: Vec<String>,
}

fn read_varint(buf: &mut &[u8]) -> Option<u64> {
    let mut val = 0u64;
    let mut shift = 0u32;
    while let Some((&b, rest)) = buf.split_first() {
        *buf = rest;
        val |= u64::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some(val);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

fn read_bytes<'a>(buf: &mut &'a [u8]) -> Option<&'a [u8]> {
    let len = usize::try_from(read_varint(buf)?).ok()?;
    if buf.len() < len {
        return None;
    }
    let (head, tail) = buf.split_at(len);
    *buf = tail;
    Some(head)
}

fn skip_field(buf: &mut &[u8], wire_type: u64) -> Option<()> {
    match wire_type {
        0 => read_varint(buf).map(|_| ()),
        1 if buf.len() >= 8 => {
            *buf = &buf[8..];
            Some(())
        }
        2 => read_bytes(buf).map(|_| ()),
        5 if buf.len() >= 4 => {
            *buf = &buf[4..];
            Some(())
        }
        _ => None,
    }
}

fn read_u64_le(buf: &mut &[u8]) -> Option<u64> {
    let (head, tail) = buf.split_first_chunk::<8>()?;
    *buf = tail;
    Some(u64::from_le_bytes(*head))
}

/// google.protobuf.UInt64Value / Int64Value など（field 1 に値）
fn read_wrapped_u64(mut buf: &[u8]) -> Option<u64> {
    let mut out = 0;
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, 0) => out = read_varint(&mut buf)?,
            (1, 1) => out = read_u64_le(&mut buf)?,
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    Some(out)
}

fn read_wrapped_string(mut buf: &[u8]) -> Option<String> {
    let mut out = String::new();
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, 2) => out = std::str::from_utf8(read_bytes(&mut buf)?).ok()?.to_owned(),
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    Some(out)
}

/// repeated fixed64（packed と、1 つずつの両方）
fn read_ids(buf: &mut &[u8], wire_type: u64, out: &mut Vec<String>) -> Option<()> {
    match wire_type {
        2 => {
            let mut b = read_bytes(buf)?;
            while let Some(id) = read_u64_le(&mut b) {
                out.push(id.to_string());
            }
            Some(())
        }
        1 => {
            out.push(read_u64_le(buf)?.to_string());
            Some(())
        }
        0 => {
            out.push(read_varint(buf)?.to_string());
            Some(())
        }
        _ => skip_field(buf, wire_type),
    }
}

fn parse_folder(mut buf: &[u8]) -> Option<OfficialFolder> {
    let mut f = OfficialFolder::default();
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, wt) => read_ids(&mut buf, wt, &mut f.guild_ids)?,
            (2, 2) => f.id = Some(read_wrapped_u64(read_bytes(&mut buf)?)?),
            (2, 0) => f.id = Some(read_varint(&mut buf)?),
            (3, 2) => {
                let s = read_wrapped_string(read_bytes(&mut buf)?)?;
                f.name = Some(s.trim().to_owned()).filter(|s| !s.is_empty());
            }
            (4, 2) => f.color = Some(read_wrapped_u64(read_bytes(&mut buf)?)? as u32),
            (4, 0) => f.color = Some(read_varint(&mut buf)? as u32),
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    Some(f)
}

/// GuildFolders（folders = 1, guild_positions = 2）
fn parse_guild_folders(mut buf: &[u8]) -> Option<Vec<OfficialFolder>> {
    let (mut folders, mut positions) = (Vec::new(), Vec::new());
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (1, 2) => {
                let f = parse_folder(read_bytes(&mut buf)?)?;
                if !f.guild_ids.is_empty() {
                    folders.push(f);
                }
            }
            (2, wt) => read_ids(&mut buf, wt, &mut positions)?,
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    if folders.is_empty() {
        folders = positions
            .into_iter()
            .map(|id| OfficialFolder {
                guild_ids: vec![id],
                ..Default::default()
            })
            .collect();
    }
    (!folders.is_empty()).then_some(folders)
}

/// PreloadedUserSettings（base64）の guild_folders（field 14）
pub fn folders_from_proto(b64: &str) -> Option<Vec<OfficialFolder>> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let mut buf = bytes.as_slice();
    let mut result = None;
    while !buf.is_empty() {
        let tag = read_varint(&mut buf)?;
        match (tag >> 3, tag & 7) {
            (14, 2) => result = parse_guild_folders(read_bytes(&mut buf)?).or(result),
            (_, wt) => skip_field(&mut buf, wt)?,
        }
    }
    result
}

/// 公式の並びを Riscord の (並び, フォルダ) にする
pub fn to_local(official: &[OfficialFolder]) -> (Vec<String>, Vec<GuildFolder>) {
    let mut order = Vec::new();
    let mut folders = Vec::new();
    let mut seen = HashSet::new();
    for f in official {
        let ids: Vec<String> = f
            .guild_ids
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .cloned()
            .collect();
        if ids.is_empty() {
            continue;
        }
        order.extend(ids.iter().cloned());
        if let Some(id) = f.id {
            folders.push(GuildFolder {
                // 手元で作るフォルダの ID（16 桁の 16 進）とぶつからないよう前に印を付ける
                id: format!("discord:{id}"),
                name: f.name.clone().unwrap_or_default(),
                color: f.color.unwrap_or(GuildFolder::default().color) & 0xff_ffff,
                collapsed: false,
                guild_ids: ids,
            });
        }
    }
    (order, folders)
}

impl App {
    /// READY の公式の並びを、まだ何も保存していないときだけ使う
    pub(super) fn import_official_folders(&mut self, d: &serde_json::Value) {
        if !self.settings.guild_order.is_empty() || !self.settings.guild_folders.is_empty() {
            return;
        }
        let Some(official) = d["user_settings_proto"]
            .as_str()
            .and_then(folders_from_proto)
        else {
            return;
        };
        let (order, folders) = to_local(&official);
        if order.is_empty() {
            return;
        }
        self.settings.guild_order = order;
        self.settings.guild_folders = folders;
        self.apply_guild_order();
        self.save_settings();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn field(n: u64, wt: u64, body: &[u8], out: &mut Vec<u8>) {
        varint(n << 3 | wt, out);
        if wt == 2 {
            varint(body.len() as u64, out);
        }
        out.extend_from_slice(body);
    }

    fn wrapped_u64(v: u64) -> Vec<u8> {
        let mut b = Vec::new();
        varint(1 << 3, &mut b);
        varint(v, &mut b);
        b
    }

    fn folder(ids: &[u64], id: Option<u64>, name: Option<&str>, color: Option<u64>) -> Vec<u8> {
        let mut f = Vec::new();
        let packed: Vec<u8> = ids.iter().flat_map(|i| i.to_le_bytes()).collect();
        field(1, 2, &packed, &mut f);
        if let Some(id) = id {
            field(2, 2, &wrapped_u64(id), &mut f);
        }
        if let Some(n) = name {
            let mut s = Vec::new();
            field(1, 2, n.as_bytes(), &mut s);
            field(3, 2, &s, &mut f);
        }
        if let Some(c) = color {
            field(4, 2, &wrapped_u64(c), &mut f);
        }
        f
    }

    fn proto(folders: &[Vec<u8>]) -> String {
        use base64::Engine as _;
        let mut gf = Vec::new();
        for f in folders {
            field(1, 2, f, &mut gf);
        }
        let mut root = Vec::new();
        field(1, 0, &[1], &mut root); // 関係のない項目は読み飛ばす
        field(14, 2, &gf, &mut root);
        base64::engine::general_purpose::STANDARD.encode(root)
    }

    #[test]
    fn decodes_folders_and_converts_to_local_order() {
        let b64 = proto(&[
            folder(&[30], None, None, None),
            folder(&[10, 20], Some(77), Some(" 友達 "), Some(0x11ff22)),
            folder(&[40], Some(78), None, None),
        ]);
        let official = folders_from_proto(&b64).unwrap();
        assert_eq!(official.len(), 3);
        assert_eq!(official[1].name.as_deref(), Some("友達"));
        let (order, folders) = to_local(&official);
        assert_eq!(order, ["30", "10", "20", "40"]);
        assert_eq!(folders.len(), 2, "id のない 1 件はフォルダにしない");
        assert_eq!(folders[0].id, "discord:77");
        assert_eq!(folders[0].guild_ids, ["10", "20"]);
        assert_eq!(folders[0].color, 0x11ff22);
        assert_eq!(folders[1].name, "");
        assert!(folders_from_proto("not base64!").is_none());
        assert!(folders_from_proto(&proto(&[])).is_none());
    }

    #[test]
    fn ready_uses_official_order_only_when_nothing_is_saved() {
        let profile = "official-folders-test";
        let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
        let mut app = App::default();
        app.set_profile(Some(profile.into()));
        app.guilds = ["10", "20", "30"]
            .iter()
            .map(|id| serde_json::from_value(serde_json::json!({"id": id, "name": id})).unwrap())
            .collect();
        let ready = serde_json::json!({ "user_settings_proto":
            proto(&[folder(&[30], None, None, None), folder(&[20, 10], Some(5), Some("f"), None)]) });
        app.import_official_folders(&ready);
        let ids: Vec<&str> = app.guilds.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["30", "20", "10"]);
        assert_eq!(app.settings.guild_folders[0].guild_ids, ["20", "10"]);
        assert_eq!(load_settings(Some(profile)).guild_order, ["30", "20", "10"]);
        // Riscord で並べ替えたあとは、次の READY で上書きしない
        assert!(app.move_guild("10", "30"));
        let saved = app.settings.guild_order.clone();
        app.import_official_folders(&ready);
        assert_eq!(app.settings.guild_order, saved);
        let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
    }
}
