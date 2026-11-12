//! メンバー一覧・履歴・ポップアップ・ログイン状態・プロフィール編集の下書きなどの補助型。
use std::collections::HashMap;
use std::sync::{mpsc::Receiver, Arc};
use std::sync::atomic::Ordering::Relaxed;

use gpui::{Bounds, Pixels, RenderImage};

use super::App;
use crate::images;
use crate::login::{self, LoginEvent};
use crate::model::{Member, Presence, ProfileData, Role, User};
use crate::net::{self, Cmd};
use crate::wake::Waker;

pub const MEMBER_ROW_H: f32 = 42.0;
pub const MEMBER_HEAD_H: f32 = 40.0;

/// メンバーリストの1行。Member は members[gid] の添字
pub enum MemberRow {
    Header(String),
    Member(usize),
    /// Discord からまだ届いていない行
    Placeholder,
}

/// Discord が並べて送ってくるメンバー一覧（GUILD_MEMBER_LIST_UPDATE を当てたもの）
#[derive(Default)]
pub struct ServerMemberList {
    /// どのチャンネルの一覧として購読したか
    pub channel_id: String,
    /// 一覧の ID（見える人が同じチャンネルは同じ一覧）。最初の SYNC で決まり、違う ID の差分は無視する
    pub list_id: Option<String>,
    pub groups: Vec<(String, u32)>,
    /// 行ごとの中身。None はまだ届いていない
    pub items: Vec<Option<net::ListItem>>,
}

impl ServerMemberList {
    /// 差分を当てる。当てたら true
    pub fn apply(&mut self, list_id: &str, groups: Vec<(String, u32)>, ops: Vec<net::ListOp>) -> bool {
        match &self.list_id {
            Some(id) if id != list_id => return false,
            None if !ops.iter().any(|o| matches!(o, net::ListOp::Sync { .. })) => return false,
            _ => {}
        }
        self.list_id = Some(list_id.to_string());
        for op in ops {
            match op {
                net::ListOp::Sync { range, items } => {
                    let start = range[0] as usize;
                    if self.items.len() < start + items.len() {
                        self.items.resize(start + items.len(), None);
                    }
                    for (k, item) in items.into_iter().enumerate() {
                        self.items[start + k] = Some(item);
                    }
                }
                net::ListOp::Insert { index, item } => {
                    let i = index.min(self.items.len());
                    self.items.insert(i, Some(item));
                }
                net::ListOp::Update { index, item } => {
                    if index >= self.items.len() {
                        self.items.resize(index + 1, None);
                    }
                    self.items[index] = Some(item);
                }
                net::ListOp::Delete { index } => {
                    if index < self.items.len() {
                        self.items.remove(index);
                    }
                }
                net::ListOp::Invalidate { range } => {
                    let end = (range[1] as usize + 1).min(self.items.len());
                    for item in self.items.iter_mut().take(end).skip(range[0] as usize) {
                        *item = None;
                    }
                }
            }
        }
        // 全体の行数 = 各グループの見出し + 人数。まだ届いていない所は None のまま
        if !groups.is_empty() {
            let total: usize = groups.iter().filter(|g| g.1 > 0).map(|g| g.1 as usize + 1).sum();
            self.items.resize(total, None);
            self.groups = groups;
        }
        true
    }
}

/// グループ分け・並べ替え済みのメンバーリスト（ver が変わるまで使い回す）
#[derive(Default)]
pub struct MemberListCache {
    pub gid: String,
    pub ver: u64,
    pub rows: Vec<MemberRow>,
    /// 各行の上端（仮想スクロール用の累積高さ）
    pub tops: Vec<f32>,
    pub height: f32,
}

impl MemberListCache {
    /// Discord が並べた一覧から作る（行の順番 = 一覧の順番。見えている行の番号をそのまま購読に使える）
    pub fn from_server(gid: &str, ver: u64, list: &ServerMemberList, members: &[Member], roles: &[Role]) -> Self {
        let index: HashMap<&str, usize> = members.iter().enumerate().filter_map(|(i, m)| m.user.as_ref().map(|u| (u.id.as_str(), i))).collect();
        let mut out = MemberListCache { gid: gid.to_string(), ver, ..Default::default() };
        for item in &list.items {
            out.tops.push(out.height);
            match item {
                Some(net::ListItem::Group(id, count)) => {
                    // 一覧の中の見出しの行には人数が入っていないことがある。人数は一緒に届く groups のほうが正しい
                    let count = list.groups.iter().find(|g| &g.0 == id).map_or(*count, |g| g.1);
                    let name = match id.as_str() {
                        "online" => "オンライン",
                        "offline" => "オフライン",
                        id => roles.iter().find(|r| r.id == id).map_or("メンバー", |r| r.name.as_str()),
                    };
                    out.height += MEMBER_HEAD_H;
                    out.rows.push(MemberRow::Header(format!("{name} — {count}")));
                }
                Some(net::ListItem::Member(uid)) => {
                    out.height += MEMBER_ROW_H;
                    out.rows.push(index.get(uid.as_str()).map_or(MemberRow::Placeholder, |&i| MemberRow::Member(i)));
                }
                None => {
                    out.height += MEMBER_ROW_H;
                    out.rows.push(MemberRow::Placeholder);
                }
            }
        }
        out.height += 8.0;
        out
    }

    pub fn build(gid: &str, ver: u64, members: &[Member], roles: &[Role], presences: &HashMap<String, Presence>) -> Self {
        let mut hoisted: Vec<&Role> = roles.iter().filter(|r| r.hoist).collect();
        hoisted.sort_by_key(|r| std::cmp::Reverse(r.position));
        let online_index = hoisted.len();
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); online_index + 2];
        for (i, member) in members.iter().enumerate() {
            let Some(user) = &member.user else { continue };
            let online = presences.get(&user.id).is_some_and(|p| p.status != "offline");
            let index = if online {
                hoisted.iter().position(|r| member.roles.contains(&r.id)).unwrap_or(online_index)
            } else {
                online_index + 1
            };
            groups[index].push(i);
        }
        let names = hoisted.iter().map(|r| r.name.as_str()).chain(["オンライン", "オフライン"]);
        let mut out = MemberListCache { gid: gid.to_string(), ver, ..Default::default() };
        for (name, mut group) in names.zip(groups) {
            if group.is_empty() {
                continue;
            }
            group.sort_by_cached_key(|&i| members[i].display().to_lowercase());
            out.tops.push(out.height);
            out.height += MEMBER_HEAD_H;
            out.rows.push(MemberRow::Header(format!("{name} — {}", group.len())));
            for i in group {
                out.tops.push(out.height);
                out.height += MEMBER_ROW_H;
                out.rows.push(MemberRow::Member(i));
            }
        }
        out.height += 8.0;
        out
    }
}


pub enum ProfileState {
    Loading,
    Ready(ProfileData),
    Failed,
}

#[derive(Clone)]
pub struct ProfilePopup {
    pub user: User,
    pub guild_id: Option<String>,
    pub anchor: Bounds<Pixels>,
    /// 左下の自分のアバターから開いたときは上に出す
    pub above: bool,
}

/// 既読を送る最短の間隔。新着が続いても、この間隔で最新のものだけ送る
pub const ACK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);
pub(super) const ACK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// 過去ログの読み足し
#[derive(Default)]
pub struct History {
    /// 読み込み中の起点（このIDより前を頼んでいる）
    pub pending: Option<String>,
    /// チャンネルの始まりまで読んだ
    pub end: bool,
    /// 失敗したらしばらく待ってから取り直す
    pub retry_at: Option<std::time::Instant>,
}

/// 上端からこの行数まで近づいたら古いメッセージを読み足す
pub const HISTORY_TRIGGER_ROWS: usize = 20;
/// 一覧の先頭に置く、読み込み中の表示・チャンネルの始まりの表示の高さ（どちらでも同じにして行がずれないようにする）
pub const HISTORY_HEAD_H: f32 = 72.0;
/// 表示中のチャンネルで遡って持てるメッセージ数の上限
pub const MAX_HISTORY: usize = 5000;
/// 1チャンネルで保持するメッセージ数の上限
pub const MAX_MSGS: usize = 1000;
/// メッセージを保持しておくチャンネル数の上限
pub const MAX_CHANNELS: usize = 30;
/// 1メッセージに添付できるファイル数（Discord の上限）
pub const MAX_UPLOADS: usize = 10;

/// 「コピーしました」などを出しておく時間
pub const TOAST_TTL: std::time::Duration = std::time::Duration::from_millis(1600);
pub const COMPOSE_NOTICE_TTL: std::time::Duration = std::time::Duration::from_secs(8);
/// 受信中のメンバー索引は、更新が止まってから短時間で破棄する。
pub const MEMBER_INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(5);

pub struct RecentMemberIndex {
    pub guild_id: String,
    pub index: HashMap<String, usize>,
    pub updated: std::time::Instant,
}


// ---- ログイン画面の部品 ----
/// ログイン画面の状態（QR コード・メールアドレス・二段階認証）
#[derive(Default)]
pub struct LoginState {
    pub email: String,
    pub password: String,
    /// 二段階認証のコードを待っている（/auth/login が返した ticket）
    pub mfa_ticket: Option<String>,
    pub mfa_code: String,
    /// 送信中のメールアドレス・二段階認証の結果の受け口
    pub busy: Option<Receiver<LoginEvent>>,
    pub qr: Option<QrSession>,
}

/// QR コードでのログイン 1回分。捨てると通信も止まる
pub struct QrSession {
    pub rx: Receiver<LoginEvent>,
    pub cancel: Arc<std::sync::atomic::AtomicBool>,
    /// QR コードのマス目（1辺の数, 黒か）
    pub modules: Option<(usize, Vec<bool>)>,
    /// 読み取ったアカウント（名前, アバター）
    pub scanned: Option<(String, Option<String>)>,
    /// 期限切れ・取り消し（説明）
    pub ended: Option<String>,
}

impl QrSession {
    pub fn start(ctx: &Waker) -> Self {
        let (rx, cancel) = login::start_qr(ctx);
        QrSession { rx, cancel, modules: None, scanned: None, ended: None }
    }
}

impl Drop for QrSession {
    fn drop(&mut self) {
        self.cancel.store(true, Relaxed);
    }
}


pub const PROFILE_NAME_MAX: usize = 32;
pub const PROFILE_PRONOUNS_MAX: usize = 40;
pub const PROFILE_BIO_MAX: usize = 190;
/// アバターは正方形に切り抜き、この大きさまで縮めて送る
pub const AVATAR_SIZE: u32 = 512;

pub enum AvatarEdit {
    /// 新しい画像（送る data URI と、プレビュー用の画像）
    Set { uri: String, tex: Arc<RenderImage> },
    Remove,
}

// プレビューを差し替え・取消・保存したら GPUI の GPU キャッシュも解放する。
impl Drop for AvatarEdit {
    fn drop(&mut self) {
        if let Self::Set { tex, .. } = self {
            images::release(tex.clone());
        }
    }
}

/// 編集中のプロフィール。fields / base = [表示名, 代名詞, 自己紹介]
pub struct ProfileDraft {
    pub fields: [String; 3],
    pub base: [String; 3],
    pub avatar: Option<AvatarEdit>,
}

impl ProfileDraft {
    pub fn dirty(&self) -> bool {
        self.avatar.is_some() || self.fields.iter().zip(&self.base).any(|(a, b)| a.trim() != b.trim())
    }
}


impl App {
    /// 開いているテキストチャンネルのメンバー一覧を、見えている範囲だけ購読する（公式と同じく全員は取らない）。
    /// 先頭の 100 人は常に、スクロールしたらその辺りの 100 人ずつを足す
    pub fn subscribe_member_list(&mut self) {
        let (Some(gid), Some(cid)) = (self.sel_guild.clone(), self.sel_channel.clone()) else { return };
        if !self.current_channel().is_some_and(|c| c.is_text()) {
            return;
        }
        let same_channel = self.member_sub.as_ref().is_some_and(|(g, c, _)| *g == gid && *c == cid);
        let (first, last) = if same_channel { self.member_view } else { (0, 0) };
        let mut ranges = vec![[0u32, 99]];
        for chunk in [first / 100, last / 100] {
            let r = [chunk as u32 * 100, chunk as u32 * 100 + 99];
            if !ranges.contains(&r) {
                ranges.push(r);
            }
        }
        if self.member_sub.as_ref().is_some_and(|(g, c, r)| *g == gid && *c == cid && *r == ranges) {
            return;
        }
        if !same_channel || !self.member_lists.get(&gid).is_some_and(|l| l.channel_id == cid) {
            // 別のチャンネルの一覧は、新しい一覧が届くまで出さない
            self.member_lists.insert(gid.clone(), ServerMemberList { channel_id: cid.clone(), ..Default::default() });
            self.member_ver += 1;
        }
        self.send(Cmd::SubscribeMembers { guild_id: gid.clone(), channel_id: cid.clone(), ranges: ranges.clone() });
        self.member_sub = Some((gid, cid, ranges));
    }

}
