//! アプリの状態と、画面に依存しないロジック（通信イベントの処理・送信・メンバー一覧・音声・テーマの適用など）。
//! 描画は src/ui/ にある
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc::Receiver, Arc};

use std::sync::atomic::Ordering::Relaxed;
use tokio::sync::mpsc::UnboundedSender;

use crate::model::{Channel, Embed, Guild, Member, Message, Presence, Role, User, VoiceState};
use crate::net::{self, Cmd, Event, VoiceStatus};
use crate::wake::Waker;
use crate::{cache, clip, images, theme, voice};

mod events;
mod helpers;
mod member_list;
mod settings;
#[path = "app/voice.rs"]
mod app_voice;

pub use helpers::*;
pub use member_list::*;
pub use settings::*;

// ---- アプリ本体 ----
pub struct Backend {
    pub cmd: UnboundedSender<Cmd>,
    pub ev: Receiver<Event>,
}

pub enum MessageChange {
    Create(Message),
    Update { id: String, content: Option<String>, embeds: Option<Vec<Embed>> },
    Delete(String),
}

impl MessageChange {
    pub fn apply(self, messages: &mut Vec<Message>) {
        match self {
            Self::Create(message) => {
                if !messages.iter().any(|m| m.id == message.id) {
                    let at = messages.partition_point(|m| (m.id.len(), m.id.as_str()) < (message.id.len(), message.id.as_str()));
                    messages.insert(at, message);
                }
            }
            Self::Update { id, content, embeds } => {
                if let Some(message) = messages.iter_mut().find(|m| m.id == id) {
                    if let Some(content) = content {
                        if message.content != content {
                            message.content = content;
                            message.edited_timestamp = Some(String::new());
                        }
                    }
                    if let Some(embeds) = embeds { message.embeds = embeds; }
                }
            }
            Self::Delete(id) => messages.retain(|m| m.id != id),
        }
    }
}

pub struct ComposeSubmission {
    pub content: String,
    pub reply: Option<Message>,
    pub editing: Option<Message>,
    pub uploads: Vec<clip::Upload>,
    pub failed: bool,
    /// Discord に断られた理由（あれば確実に届いていない）
    pub rejected: Option<String>,
}

#[derive(Default)]
pub struct App {
    pub plugins: crate::plugins::Plugins,
    pub translator: crate::translator::Translator,
    /// MessageLogger の記録（削除・編集）
    pub message_log: crate::message_log::MessageLog,
    /// 別スレッドから画面を描き直させる口（描画側が作って渡す）
    pub waker: Waker,
    /// ウィンドウの幅（拡大率を除いた論理ピクセル）。狭い画面のときの並べ方に使う
    pub screen_w: f32,
    /// テーマ・フォントを適用し直してほしい（描画側で処理する）
    pub theme_dirty: bool,
    pub font_dirty: bool,
    /// 入力欄にフォーカスを移してほしい（描画側で処理する）
    pub focus_input: bool,
    pub backend: Option<Backend>,

    pub token_input: String,
    pub remember: bool,
    /// ログイン画面（QR コード・メールアドレス）の状態
    pub login: LoginState,
    pub login_error: Option<String>,

    pub me: Option<User>,
    pub guilds: Vec<Guild>,
    pub dms: Vec<Channel>,
    pub channels: HashMap<String, Vec<Channel>>,
    pub roles: HashMap<String, Vec<Role>>,
    pub members: HashMap<String, Vec<Member>>,
    pub member_index: Option<RecentMemberIndex>,
    /// メッセージ作者のメンバー情報を op 8 で問い合わせ済みのユーザーID（サーバーごと）
    pub requested_members: HashMap<String, HashSet<String>>,
    /// サーバーごとのメンバー一覧（Discord が並べて送ってくるもの。購読中のチャンネルの分）
    pub member_lists: HashMap<String, ServerMemberList>,
    /// 最後に送った購読（サーバー, チャンネル, 範囲）
    pub member_sub: Option<(String, String, Vec<[u32; 2]>)>,
    /// メンバー一覧で見えている行（先頭, 末尾）。この範囲を購読する
    pub member_view: (usize, usize),
    pub show_members: bool,
    pub show_members_narrow: bool,
    pub compact_sidebar_closed: bool,
    pub messages: HashMap<String, Vec<Message>>,
    /// messages のどこかが変わるたびに進む番号（描画側はこれが同じなら行の目印を作り直さない）
    pub msg_rev: u64,
    /// チャンネルごとの過去ログ読み足しの状態
    pub history: HashMap<String, History>,
    /// 見る権限がなかったチャンネル（開くと 403）
    pub no_access: HashSet<String>,
    pub message_errors: HashSet<String>,
    pub message_status_channel: Option<String>,
    pub pending_message_loads: HashMap<String, Vec<MessageChange>>,
    /// チャンネルごとに表示した添付・埋め込み画像の URI。チャンネルを捨てるときに画像キャッシュも解放する
    pub channel_images: HashMap<String, HashSet<String>>,
    /// 未読チャンネル → 所属サーバー（DMならNone）
    pub unread: HashMap<String, Option<String>>,
    /// DM 一覧の絞り込み
    pub dm_search: bool,
    pub dm_filter: String,
    /// 戻る/進むの履歴（サーバー, チャンネル）
    pub nav_back: Vec<(Option<String>, Option<String>)>,
    pub nav_fwd: Vec<(Option<String>, Option<String>)>,
    /// 返信先・編集中のメッセージ
    pub reply_to: Option<Message>,
    pub editing: Option<Message>,
    pub pending_delete: Option<Message>,
    pub compose_submissions: HashMap<String, ComposeSubmission>,
    /// Ctrl+K（検索文字, 選択中の行）
    pub switcher: Option<(String, usize)>,
    /// メッセージを読み込んだチャンネル（古い順）。上限を超えたら古いものから捨てる
    pub loaded_channels: std::collections::VecDeque<String>,
    /// DM のプロフィール欄を閉じたか
    pub hide_dm_profile: bool,
    /// メンバー・ロール・オンライン状態が変わるたびに増える（メンバーリストの作り直しの合図）
    pub member_ver: u64,
    pub member_cache: MemberListCache,
    pub sel_guild: Option<String>, // None = DM
    pub sel_channel: Option<String>,
    pub input: String,
    /// サーバーごとのカスタム絵文字（ピッカーと :名前: の変換に使う）
    pub guild_emojis: HashMap<String, Vec<crate::model::Emoji>>,
    /// guild_emojis が変わるたびに増える（ピッカーの作り直しの合図）
    pub emoji_rev: u64,
    /// 絵文字を REST で取りに行ったサーバー（READY に無かったときの補い。二重に頼まない）
    pub emoji_requested: HashSet<String>,
    /// ピッカーの検索欄
    pub emoji_query: String,
    /// 送信待ちの添付と、それを貼り付けたチャンネル（別のチャンネルに移ったら捨てる）
    pub uploads: Vec<clip::Upload>,
    /// 添付や文字数の上限についてのお知らせ（入力欄の上に出す）
    pub compose_notice: Option<(String, std::time::Instant)>,
    /// チャンネルごとに、最後に既読として Discord に送ったメッセージID
    pub acked: HashMap<String, String>,
    /// API エラー後に再試行できる時刻（チャンネルごと）
    pub ack_retry_at: HashMap<String, std::time::Instant>,
    /// 最後に既読を送った時刻（送りすぎて制限されないよう間を空ける）
    pub last_ack: Option<std::time::Instant>,
    /// 今使っている見た目（テーマ＋上書き）。変わったときだけ適用し直す
    pub theme_applied: Option<theme::Resolved>,
    pub theme_error: Option<String>,
    /// テーマファイルの読み飛ばした項目（知らない部品・読めない値）
    pub theme_warnings: Vec<String>,
    /// themes/ のファイルが変わったら届く（保存するとすぐ反映）
    pub theme_watch: Option<Receiver<()>>,
    /// 開いているファイル選択ダイアログの結果の受け口
    pub file_pick: Option<(String, Receiver<Vec<PathBuf>>)>,
    pub picked_files: Option<(String, Vec<PathBuf>)>,
    /// 背景画像を選ぶダイアログの結果の受け口
    pub bg_pick: Option<Receiver<Vec<PathBuf>>>,
    pub upload_channel: Option<String>,
    pub status: String,
    /// ゲートウェイから届いた接続の状態（接続中・再接続待ちなど）。ステータス帯のお知らせを閉じるとこれに戻す
    pub gateway_status: String,
    /// DM 一覧が一度でも届いたか（届く前は読み込み中、届いて 0 件なら空と出し分ける）
    pub dms_loaded: bool,
    /// 画面下に短く出す操作の結果（「コピーしました」など）
    pub toast: Option<(String, std::time::Instant)>,
    /// 次の描画でクリップボードに書くもの（gpui の cx が要るので画面側で書く）
    pub clipboard_out: Option<String>,
    /// サーバーごとの自分の通知設定（READY と USER_GUILD_SETTINGS_UPDATE から）
    pub guild_settings: HashMap<String, GuildSettings>,
    /// 未読のメンション数（チャンネル → (サーバー, 件数)。DM はサーバーが None で、メッセージごとに数える）
    pub mention_counts: HashMap<String, (Option<String>, u32)>,
    /// READY の read_state / MESSAGE_ACK から得たチャンネルごとの既読位置（last_viewed, last_message_id）
    pub channel_read_rank: HashMap<String, (u64, u64)>,
    /// サーバーごとの自分のロール（ロールへのメンションを見分ける）
    pub my_roles: HashMap<String, Vec<String>>,
    /// ウィンドウにフォーカスがあるか（見ているチャンネルの通知は出さない）
    pub window_focused: bool,
    /// 押されたデスクトップ通知の行き先
    pub notify_clicked: crate::notify::Clicked,
    /// Cmd::Api に付ける番号
    pub next_api_id: u64,
    /// 脱退の確認を出しているサーバー
    pub confirm_leave: Option<String>,

    pub settings: Settings,
    pub stickers: HashMap<String, StickerState>,
    pub presences: HashMap<String, Presence>,
    pub profiles: HashMap<String, ProfileState>,
    pub popup: Option<ProfilePopup>,
    pub settings_open: bool,
    pub settings_tab: usize,
    /// 設定の左上の検索欄
    pub settings_query: String,
    /// ログアウトの確認を出している
    pub confirm_logout: bool,
    /// 設定 → プロフィールで編集中の内容（プロフィールを読み込んでから作る）
    pub profile_draft: Option<ProfileDraft>,
    pub profile_saving: bool,
    pub profile_error: Option<String>,
    pub profile_saved_at: Option<std::time::Instant>,
    /// 保存せずに閉じようとした時刻（保存バーを赤くして知らせる）
    pub profile_nag: Option<std::time::Instant>,
    pub avatar_pick: Option<Receiver<Vec<PathBuf>>>,

    // ---- 音声 ----
    pub voice: Arc<voice::VoiceCtl>,
    /// user_id → どのボイスチャンネルにいるか
    pub voice_states: HashMap<String, VoiceState>,
    pub voice_index: HashMap<String, Vec<String>>,
    /// 自分の接続状態（None = 未接続）
    pub voice_status: Option<VoiceStatus>,
    /// 接続確立後の経過時間。チャンネル変更・切断でリセットする。
    pub voice_call_started: Option<(Option<String>, String, std::time::Instant)>,
    /// DM通話: channel_id → 呼び出し中のユーザー
    pub calls: HashMap<String, Vec<String>>,
    pub mic_test: Option<voice::MicTest>,
    pub audio_devs: Option<(Vec<String>, Vec<String>)>,
    pub binding_ptt: bool,
    /// 前回の控えから出しているだけで、まだ読み直していないチャンネル（開いたら取り直す・既読は送らない）
    pub stale_channels: HashSet<String>,
    /// 同じく、控えから出しているサーバーのチャンネル一覧
    pub stale_guilds: HashSet<String>,
    /// 今のトークンの持ち主のユーザー ID（控えを書くときに付ける）
    pub token_owner: Option<String>,
    /// このアカウントのプロファイル（None は既定）。トークン・設定・控えの置き場所が決まる
    pub profile: Option<String>,
    /// 最後に控えを書いた時刻と中身の目印（変わっていなければ書かない）
    pub cache_saved: Option<(std::time::Instant, u64)>,
    /// 控えを出したまま接続できなかったとき、この時刻にもう一度つなぐ
    pub login_retry_at: Option<std::time::Instant>,
    /// 前回書いたあと、控えに入る中身が変わったかもしれない
    pub cache_dirty: bool,
}

/// 控えを書き直す間隔
pub const CACHE_SAVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
/// 控えを出したまま接続できなかったとき、つなぎ直すまでの時間
pub const LOGIN_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// msg_rev の元。ログアウトで App を作り直しても前の値に戻らないよう、プロセスで1つ
static MSG_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl App {
    /// messages を変えたら呼ぶ
    pub fn bump_msgs(&mut self) {
        self.msg_rev = MSG_REV.fetch_add(1, Relaxed) + 1;
    }

    /// profile = このアカウントのプロファイル（None は既定）。分割表示では画面ごとに違う
    pub fn new(waker: Waker, profile: Option<String>) -> Self {
        let settings = load_settings(profile.as_deref());
        let mut app = App { waker, screen_w: 1200.0, remember: true, settings, show_members: true, ..Default::default() };
        app.set_profile(profile);
        app.plugins.reload(&app.settings.enabled_plugins);
        app.theme_watch = Some(watch_themes(app.waker.clone()));
        app.apply_theme(false);
        if crate::demo::enabled() {
            app.voice.set_ctx(app.waker.clone());
            let (cmd, ev) = crate::demo::spawn(app.waker.clone());
            app.backend = Some(Backend { cmd, ev });
            // RISCORD_DEMO=guild なら最初からサーバーのチャンネルを、dm なら DM を開いておく（画面確認用）
            match std::env::var("RISCORD_DEMO").as_deref() {
                Ok("guild") => app.select(Some("100".into()), Some("10011".into())),
                Ok("dm") => app.select(None, Some("900".into())),
                _ => {}
            }
        } else if let Some(t) = app.load_token() {
            let snapshot = cache::load(app.profile.as_deref(), &t);
            std::thread::spawn(cache::prune_images);
            app.start(t);
            if let Some(s) = snapshot {
                app.restore_snapshot(s);
            }
        }
        app
    }

    /// 前回の控えを画面に出す。ログインは裏で続き、届いたものから入れ替わる。
    /// 開いていたチャンネルとサーバーは、今すぐ読み直しを頼んでおく（ログインが終わりしだい送られる）
    pub fn restore_snapshot(&mut self, s: cache::Snapshot) {
        self.me = s.me;
        self.guilds = s.guilds;
        self.apply_guild_order();
        self.dms = s.dms;
        self.dms_loaded = true;
        for (gid, chans) in s.channels {
            self.stale_guilds.insert(gid.clone());
            self.channels.insert(gid, chans);
        }
        self.roles.extend(s.roles);
        for (cid, list) in s.messages {
            self.stale_channels.insert(cid.clone());
            self.history.insert(cid.clone(), History::default());
            self.loaded_channels.push_back(cid.clone());
            self.messages.insert(cid, list);
        }
        self.bump_msgs();
        let guild_ok = s.sel_guild.as_ref().is_none_or(|g| self.guilds.iter().any(|x| &x.id == g));
        if guild_ok {
            let channel = s.sel_channel.filter(|c| self.messages.contains_key(c));
            self.select(s.sel_guild, channel);
        }
    }

    /// 次の起動ですぐ出すための控えを作る。トークンを保存していないときは作らない
    pub fn snapshot(&self) -> Option<cache::Snapshot> {
        let owner = self.token_owner.as_ref()?;
        // 保存済みのトークンがこのアカウントのものであるときだけ（保存していないなら書かない）
        if self.load_token().and_then(|t| cache::token_owner(&t)).as_ref() != Some(owner) {
            return None;
        }
        self.build_snapshot()
    }

    pub fn build_snapshot(&self) -> Option<cache::Snapshot> {
        let owner = self.token_owner.clone()?;
        let me = self.me.clone().filter(|m| m.id == owner)?;
        let messages = self
            .loaded_channels
            .iter()
            .filter(|c| !self.no_access.contains(*c))
            .filter_map(|c| self.messages.get(c).filter(|l| !l.is_empty()).map(|l| (c, l)))
            .rev()
            .take(cache::CHANNELS)
            .map(|(c, l)| (c.clone(), l[l.len().saturating_sub(cache::MESSAGES_PER_CHANNEL)..].to_vec()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Some(cache::Snapshot {
            version: cache::VERSION,
            owner,
            me: Some(me),
            guilds: self.guilds.clone(),
            dms: self.dms.clone(),
            channels: self.channels.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            roles: self.roles.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            messages,
            sel_guild: self.sel_guild.clone(),
            sel_channel: self.sel_channel.clone(),
        })
    }

    /// 控えを書く。now=false なら CACHE_SAVE_INTERVAL ごとに裏で、now=true（終了時）ならその場で書く
    pub fn save_snapshot(&mut self, now: bool) {
        if !now {
            if !self.cache_dirty {
                return;
            }
            if let Some(wait) = self.cache_saved.and_then(|(t, _)| CACHE_SAVE_INTERVAL.checked_sub(t.elapsed())) {
                // 描き直しがなくても、間隔が空いたら書けるように起こしてもらう
                self.repaint_after(wait);
                return;
            }
        }
        self.cache_dirty = false;
        let Some(s) = self.snapshot() else { return };
        let bytes = match serde_json::to_vec(&s) {
            Ok(b) => b,
            Err(e) => return eprintln!("起動用の控えを作れませんでした: {e}"),
        };
        let mark = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut h);
            h.finish()
        };
        let changed = self.cache_saved.is_none_or(|(_, m)| m != mark);
        self.cache_saved = Some((std::time::Instant::now(), mark));
        if !changed {
            return;
        }
        let profile = self.profile.clone();
        let write = move || {
            if let Err(e) = cache::write(profile.as_deref(), &bytes) {
                eprintln!("起動用の控えを保存できませんでした: {e}");
            }
        };
        if now { write() } else { std::thread::spawn(write); }
    }

    /// 控えを出したまま接続できなかったあとのつなぎ直し。届かなかった読み込みを頼み直す
    fn retry_login(&mut self, token: String) {
        self.pending_message_loads.clear();
        self.start(token);
        if let Some(c) = self.sel_channel.clone() {
            if self.stale_channels.contains(&c) || self.message_errors.contains(&c) {
                self.load_messages(c);
            }
        }
        if let Some(g) = self.sel_guild.clone() {
            self.ensure_guild_channels(&g);
        }
    }

    /// サーバーのチャンネル一覧・ロールがまだない（または控えのまま）なら取りに行く
    pub fn ensure_guild_channels(&mut self, gid: &str) {
        if !self.channels.contains_key(gid) || !self.roles.contains_key(gid) || self.stale_guilds.contains(gid) {
            self.send(Cmd::LoadGuildChannels(gid.to_string()));
        }
    }

    pub fn reset(&mut self) {
        // 会話に紐づく画像と、読み込み中の要求を破棄する。
        for uri in self.channel_images.values().flatten() {
            images::forget(uri);
        }
        let waker = self.waker.clone();
        let screen_w = self.screen_w;
        let settings = std::mem::take(&mut self.settings);
        let (applied, watch) = (self.theme_applied.take(), self.theme_watch.take());
        let plugins = std::mem::take(&mut self.plugins);
        let message_log = std::mem::take(&mut self.message_log);
        let profile = self.profile.take();
        *self = App { waker, screen_w, remember: true, settings, show_members: true, theme_applied: applied, theme_watch: watch, plugins, message_log, profile, ..Default::default() };
    }

    /// プロファイルを決め、それに合わせて記録・プラグインのデータの置き場所を変える
    pub fn set_profile(&mut self, profile: Option<String>) {
        self.message_log.path = crate::message_log::path_for(profile.as_deref());
        self.plugins.data_root = crate::profile::account_dir(profile.as_deref()).join("plugin-data");
        self.profile = profile;
    }

    pub fn token_path(&self) -> PathBuf {
        token_path(self.profile.as_deref())
    }

    pub fn load_token(&self) -> Option<String> {
        load_token(self.profile.as_deref())
    }

    pub fn save_settings(&self) {
        save_settings(self.profile.as_deref(), &self.settings);
    }

    /// d 後に描き直す（期限のあるお知らせを消すときなど）
    pub fn repaint_after(&self, d: std::time::Duration) {
        self.waker.wake_after(d);
    }

    fn connect(&mut self, token: String) {
        self.voice.set_ctx(self.waker.clone());
        self.apply_voice_settings();
        let (cmd, ev) = net::spawn(
            token,
            net::MyPresence { status: self.settings.status.clone(), activity: self.settings.show_activity },
            self.waker.clone(),
            self.voice.clone(),
        );
        self.backend = Some(Backend { cmd, ev });
        // ログイン中は「接続中」を出さない（ホーム画面をそのまま使ってもらう）。切断・エラーはゲートウェイから届く
        self.status.clear();
        self.gateway_status.clear();
    }

    /// ホーム画面を出すか（ログイン済み、またはつなぎ始めている）。つないでいる間も「ログイン中」画面は出さない
    pub fn in_home(&self) -> bool {
        self.me.is_some() || self.backend.is_some()
    }

    /// トークンでつなぐ（控えのハッシュ用に覚えておく）
    pub fn start(&mut self, token: String) {
        self.token_owner = cache::token_owner(&token);
        self.connect(token);
    }

    pub fn send(&self, c: Cmd) {
        if let Some(b) = &self.backend {
            let _ = b.cmd.send(c);
        }
    }

    fn set_guild_emojis(&mut self, gid: String, list: serde_json::Value) {
        if let Ok(list) = serde_json::from_value(list) {
            self.guild_emojis.insert(gid, list);
            self.emoji_rev += 1;
        }
    }

    /// 開いているサーバーの絵文字をまだ持っていなければ取りに行く（READY に入っていなかったとき）
    pub fn ensure_guild_emojis(&mut self) {
        let Some(gid) = self.sel_guild.clone() else { return };
        if !self.guild_emojis.contains_key(&gid) && self.emoji_requested.insert(gid.clone()) {
            self.api(net::ApiCall::GetEmojis(gid));
        }
    }

    /// Nitro があれば、ほかのサーバーの絵文字とアニメ絵文字も使える
    pub fn has_nitro(&self) -> bool {
        self.me.as_ref().and_then(|u| u.premium_type).is_some_and(|t| t > 0)
    }

    /// いま送れるカスタム絵文字を、サーバーごとに（開いているサーバーが先頭、あとはサーバーの並び順）
    pub fn usable_emojis(&self) -> Vec<(&str, Vec<&crate::model::Emoji>)> {
        let nitro = self.has_nitro();
        let current = self.sel_guild.as_deref();
        let mut out = Vec::new();
        let ids = current.into_iter().chain(self.guilds.iter().map(|g| g.id.as_str()).filter(|g| nitro && Some(*g) != current));
        for gid in ids {
            let list: Vec<_> = self.guild_emojis.get(gid).into_iter().flatten().filter(|e| e.available && (nitro || !e.animated)).collect();
            if !list.is_empty() {
                out.push((gid, list));
            }
        }
        out
    }

    /// 名前で絵文字を探す（補完とピッカーの検索）
    pub fn emoji_search(&self, query: &str, limit: usize) -> Vec<crate::emoji::Pick> {
        let usable = self.usable_emojis();
        crate::emoji::search(query, usable.iter().flat_map(|(_, l)| l).map(|e| (e.name.as_str(), e.id.as_str(), e.animated)), limit)
    }

    /// 本文の :名前: を送れる形にする。known = 編集中のメッセージに元からあった絵文字（使えないサーバーのものでも保つ）
    pub fn expand_emoji_names(&self, content: &str, known: &[(String, String, bool)]) -> String {
        if !content.contains(':') {
            return content.to_string();
        }
        let usable = self.usable_emojis();
        crate::emoji::expand(content, |name| {
            if let Some((n, id, animated)) = known.iter().find(|k| k.0 == name) {
                return Some(crate::emoji::markup(n, id, *animated));
            }
            if let Some(e) = usable.iter().flat_map(|(_, l)| l).find(|e| e.name == name) {
                return Some(crate::emoji::markup(&e.name, &e.id, e.animated));
            }
            emojis::get_by_shortcode(name).map(|e| e.as_str().to_string())
        })
    }

    pub fn submit_compose(&mut self, channel_id: String, content: String) {
        if self.compose_submissions.contains_key(&channel_id) { return; }
        if self.editing.is_none() && self.uploads.is_empty() && self.plugins.command(&content, &channel_id) { self.waker.wake(); return; }
        let known = self.editing.as_ref().map(|m| crate::emoji::shorten(&m.content).1).unwrap_or_default();
        let mut content = self.expand_emoji_names(&content, &known);
        if self.editing.is_none() && self.plugins.translator_enabled() {
            let auto_target = match self.plugins.translation_outgoing() {
                Ok(target) => target,
                Err(error) => {
                    let reply = self.reply_to.take();
                    let uploads = std::mem::take(&mut self.uploads);
                    self.compose_submissions.insert(channel_id, ComposeSubmission { content, reply, editing: None, uploads, failed: true, rejected: Some(error) });
                    return;
                }
            };
            if auto_target.is_some() && content.starts_with("!tr ") {
                content = content[4..].to_owned();
            } else {
                let request = crate::translator::prefix(&content).map(|(language, body)| (language, body.to_owned()))
                    .or_else(|| auto_target.filter(|_| !content.trim().is_empty()).map(|language| (language, content.clone())));
                if let Some((language, body)) = request {
                    self.translator.next_request_id = self.translator.next_request_id.wrapping_add(1);
                    let request_id = self.translator.next_request_id;
                    let target = crate::translator::Target::Outgoing { channel: channel_id.clone(), request_id, generation: self.plugins.generation };
                    let result = self.translator.request(target, body, language, self.waker.clone());
                    if result.is_ok() { self.translator.outgoing.insert(channel_id.clone(), request_id); }
                    let reply = self.reply_to.take();
                    let uploads = std::mem::take(&mut self.uploads);
                    let (failed, rejected) = match result { Ok(()) => (false, None), Err(e) => (true, Some(e)) };
                    self.compose_submissions.insert(channel_id, ComposeSubmission { content, reply, editing: None, uploads, failed, rejected });
                    return;
                }
            }
        }
        if let Some(m) = self.editing.as_ref() {
            if content.trim().is_empty() {
                self.pending_delete = self.editing.take();
                return;
            }
            if content == m.content {
                self.editing = None;
                return;
            }
        }
        let content = if self.editing.is_none() { self.plugins.transform("on_send", &content, &channel_id) } else { content };
        if message_text_len(&content) > message_text_limit(self.me.as_ref().and_then(|u| u.premium_type)) {
            self.input = content;
            self.compose_notice = Some(("プラグインによる変換後の本文が文字数制限を超えました".into(), std::time::Instant::now()));
            self.repaint_after(COMPOSE_NOTICE_TTL);
            return;
        }
        let editing = self.editing.take();
        let reply = self.reply_to.take();
        let uploads = if editing.is_none() { std::mem::take(&mut self.uploads) } else { Vec::new() };
        let cmd = if let Some(m) = &editing {
            Cmd::Edit { channel_id: channel_id.clone(), id: m.id.clone(), content: content.clone() }
        } else {
            Cmd::Send {
                channel_id: channel_id.clone(), content: content.clone(),
                reply_to: reply.as_ref().map(|m| m.id.clone()),
                files: uploads.iter().map(|u| (u.name.clone(), u.data.clone())).collect(),
            }
        };
        let failed = self.backend.as_ref().is_none_or(|b| b.cmd.send(cmd).is_err());
        self.compose_submissions.insert(channel_id, ComposeSubmission { content, reply, editing, uploads, failed, rejected: None });
    }

    pub fn restore_compose(&mut self, channel_id: &str) -> bool {
        if !self.input.is_empty() || !self.uploads.is_empty() || self.editing.is_some() || self.reply_to.is_some() {
            return false;
        }
        if !self.compose_submissions.get(channel_id).is_some_and(|s| s.failed) { return false; }
        let submission = self.compose_submissions.remove(channel_id).unwrap();
        self.input = submission.content;
        self.reply_to = submission.reply;
        self.editing = submission.editing;
        self.uploads = submission.uploads;
        self.upload_channel = Some(channel_id.to_string());
        true
    }

    pub fn logout(&mut self) {
        self.send(Cmd::LeaveVoice);
        let _ = std::fs::remove_file(self.token_path());
        cache::remove(self.profile.as_deref());
        self.reset();
    }


    /// クリップボードの画像を添付に加える。画像がなければ false
    pub fn paste_image(&mut self) -> bool {
        match clip::clipboard_image() {
            Some(Ok(u)) => {
                self.add_uploads(vec![u], 0, Vec::new());
                true
            }
            Some(Err(e)) => {
                self.set_compose_notice(e);
                true
            }
            None => false,
        }
    }

    /// 貼り付けた文字がファイルのパス（ファイルマネージャでコピーしたもの）なら添付に回す。回したら true
    pub fn paste_text(&mut self, text: &str) -> bool {
        match clip::pasted_paths(text) {
            Some(paths) => {
                self.attach_paths(paths);
                true
            }
            None => false,
        }
    }

    /// ファイル選択ダイアログで選んだファイルを、ダイアログを開いたチャンネルに戻ったときだけ添付する
    pub fn take_picked_files(&mut self) {
        if self.picked_files.as_ref().is_some_and(|(channel_id, _)| self.sel_channel.as_ref() == Some(channel_id)) {
            let Some((_, picked)) = self.picked_files.take() else { return };
            if matches!(self.status.as_str(), "選択したファイルは元のチャンネルに戻ると添付されます。" | "先に開いたファイル選択を完了し、元のチャンネルで確認してください。") {
                self.status = self.gateway_status.clone();
            }
            self.attach_paths(picked);
        }
    }

    /// 貼り付け・ドロップ・ファイル選択で来たファイルを添付に加える
    pub fn attach_paths(&mut self, paths: Vec<PathBuf>) {
        // 上限を超える分は読み込まずに数だけ数える（大きいファイルを無駄に読まない）
        let room = MAX_UPLOADS.saturating_sub(self.uploads.len());
        let skipped = paths.len().saturating_sub(room);
        let (hard, _) = self.upload_limits();
        let mut loaded = Vec::new();
        let mut too_big = Vec::new();
        for p in paths.into_iter().take(room) {
            // 上限を超えるファイルは中身を読まない
            if let Some(len) = std::fs::metadata(&p).ok().map(|m| m.len()).filter(|&len| len > hard) {
                too_big.push((p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), len));
                continue;
            }
            match clip::load_file(&p) {
                Ok(u) => loaded.push(u),
                Err(e) => self.set_compose_notice(e),
            }
        }
        if !loaded.is_empty() || skipped > 0 || !too_big.is_empty() {
            self.add_uploads(loaded, skipped, too_big);
        }
    }

    /// 1ファイルの上限（送れないことが確実な大きさ, 超えると送れないかもしれない大きさ）。
    /// サーバーではブーストで上限が上がるが段階は分からないので、アカウントの上限を超えても注意だけにする
    pub fn upload_limits(&self) -> (u64, Option<u64>) {
        let account = account_upload_limit(self.me.as_ref().and_then(|u| u.premium_type));
        if self.sel_guild.is_some() && account < GUILD_MAX_UPLOAD {
            (GUILD_MAX_UPLOAD, Some(account))
        } else {
            (account, None)
        }
    }

    /// 添付に加える。Discord の上限（1メッセージ MAX_UPLOADS 件・1ファイルの大きさ）を超えるものは加えずにお知らせを出す。
    /// skipped = 件数の上限で読み込む前に除いた数、too_big = 大きすぎて読み込む前に除いたファイル（名前, 大きさ）
    pub fn add_uploads(&mut self, new: Vec<clip::Upload>, skipped: usize, mut too_big: Vec<(String, u64)>) {
        let (hard, soft) = self.upload_limits();
        let mut risky = Vec::new();
        let mut over = Vec::new();
        for u in new {
            let len = u.data.len() as u64;
            if len > hard {
                too_big.push((u.name.clone(), len));
            } else if self.uploads.len() >= MAX_UPLOADS {
                over.push(u.name.clone());
            } else {
                if soft.is_some_and(|s| len > s) {
                    risky.push((u.name.clone(), len));
                }
                self.uploads.push(u);
            }
        }
        let mut lines = Vec::new();
        let dropped = over.len() + skipped;
        if dropped > 0 {
            lines.push(match over.first() {
                Some(name) if dropped > 1 => format!("添付できるのは1メッセージ{MAX_UPLOADS}件までです。「{name}」ほか{}件は追加していません。", dropped - 1),
                Some(name) => format!("添付できるのは1メッセージ{MAX_UPLOADS}件までです。「{name}」は追加していません。"),
                None => format!("添付できるのは1メッセージ{MAX_UPLOADS}件までです。{dropped}件のファイルは追加していません。"),
            });
        }
        if let Some((name, len)) = too_big.first() {
            let more = if too_big.len() > 1 { format!("ほか{}件", too_big.len() - 1) } else { String::new() };
            lines.push(format!("「{name}」（{}）{more}は大きすぎるため追加していません。1ファイル{}までです。", fmt_size(*len), fmt_size(hard)));
        }
        if let (Some((name, len)), Some(s)) = (risky.first(), soft) {
            let more = if risky.len() > 1 { format!("ほか{}件", risky.len() - 1) } else { String::new() };
            lines.push(format!("「{name}」（{}）{more}はアカウントの上限 {} を超えています。サーバーのブーストが足りないと送信できません。", fmt_size(*len), fmt_size(s)));
        }
        if lines.is_empty() {
            return;
        }
        self.compose_notice = Some((lines.join("\n"), std::time::Instant::now()));
        self.repaint_after(COMPOSE_NOTICE_TTL);
    }

    /// 入力欄の上のお知らせを出す（しばらくすると消える）
    pub fn set_compose_notice(&mut self, text: String) {
        self.compose_notice = Some((text, std::time::Instant::now()));
        self.repaint_after(COMPOSE_NOTICE_TTL);
    }

    /// REST API をどれでも呼ぶ。結果は on_api_result に届く。返り値はこの呼び出しの番号
    pub fn api(&mut self, call: net::ApiCall) -> u64 {
        self.next_api_id += 1;
        let id = self.next_api_id;
        self.send(Cmd::Api { id, call });
        id
    }


    /// 保存してある並び（settings.guild_folders または settings.guild_order）にサーバーを並べる
    pub fn apply_guild_order(&mut self) {
        let folder_order: Vec<&str> = self
            .settings
            .guild_folders
            .iter()
            .flat_map(|f| f.guild_ids.iter().map(String::as_str))
            .collect();
        let pos: HashMap<&str, usize> = if !folder_order.is_empty() {
            folder_order.into_iter().enumerate().map(|(i, id)| (id, i)).collect()
        } else {
            self.settings.guild_order.iter().enumerate().map(|(i, id)| (id.as_str(), i)).collect()
        };
        let mut keyed: Vec<(usize, Guild)> = std::mem::take(&mut self.guilds)
            .into_iter()
            .map(|g| (pos.get(g.id.as_str()).copied().unwrap_or(usize::MAX), g))
            .collect();
        keyed.sort_by_key(|(k, _)| *k);
        self.guilds = keyed.into_iter().map(|(_, g)| g).collect();
    }

    /// 公式設定（user_settings_proto / user_settings）から受け取ったフォルダと並び順を反映する
    pub fn apply_guild_folders(&mut self, folders: Vec<GuildFolder>) {
        if folders.is_empty() {
            return;
        }
        let mut order: Vec<String> = folders.iter().flat_map(|f| f.guild_ids.iter().cloned()).collect();
        let in_folders: HashSet<String> = order.iter().cloned().collect();
        order.extend(self.settings.guild_order.iter().filter(|id| !in_folders.contains(*id)).cloned());
        self.settings.guild_folders = folders;
        self.settings.guild_order = order;
        self.apply_guild_order();
        self.save_settings();
        self.cache_dirty = true;
    }

    /// サーバーレールに並べる一覧（公式設定のフォルダ構造があればフォルダごとにまとめる）
    pub fn rail_items(&self) -> Vec<RailItem<'_>> {
        if self.settings.guild_folders.is_empty() {
            return self.guilds.iter().map(RailItem::Guild).collect();
        }
        let by_id: HashMap<&str, &Guild> = self.guilds.iter().map(|g| (g.id.as_str(), g)).collect();
        let mut out = Vec::new();
        let mut placed: HashSet<&str> = HashSet::new();
        for (idx, f) in self.settings.guild_folders.iter().enumerate() {
            let guilds: Vec<&Guild> = f
                .guild_ids
                .iter()
                .filter_map(|id| {
                    let g = *by_id.get(id.as_str())?;
                    placed.insert(g.id.as_str()).then_some(g)
                })
                .collect();
            if guilds.is_empty() {
                continue;
            }
            if f.id.is_none() && guilds.len() == 1 {
                out.push(RailItem::Guild(guilds[0]));
            } else {
                let id = f.id.unwrap_or(-((idx as i64) + 1));
                let name = f
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| guilds.iter().map(|g| g.name.as_str()).collect::<Vec<_>>().join("、"));
                let open = self.settings.open_folders.contains(&id);
                out.push(RailItem::Folder { id, name, color: f.color, open, guilds });
            }
        }
        for g in &self.guilds {
            if placed.insert(g.id.as_str()) {
                out.push(RailItem::Guild(g));
            }
        }
        out
    }

    /// サーバーフォルダの開閉を切り替える
    pub fn toggle_folder(&mut self, id: i64) {
        if let Some(pos) = self.settings.open_folders.iter().position(|&x| x == id) {
            self.settings.open_folders.remove(pos);
        } else {
            self.settings.open_folders.push(id);
        }
        self.save_settings();
    }

    /// チャンネルカテゴリが閉じているか
    pub fn is_category_collapsed(&self, id: &str) -> bool {
        self.settings.collapsed_categories.contains(id)
    }

    /// チャンネルカテゴリの開閉を切り替える
    pub fn toggle_category(&mut self, id: &str) {
        if !self.settings.collapsed_categories.remove(id) {
            self.settings.collapsed_categories.insert(id.to_string());
        }
        self.save_settings();
    }

    /// 閉じたカテゴリの中でも表示し続けるチャンネルか（選択中・未読あり・通話参加者がいるボイスチャンネル）
    pub fn channel_visible_in_collapsed_category(&self, c: &Channel, cur_voice: Option<&str>) -> bool {
        self.sel_channel.as_deref() == Some(c.id.as_str())
            || self.unread.contains_key(&c.id)
            || (c.is_voice() && (cur_voice == Some(c.id.as_str()) || self.voice_members(&c.id).next().is_some()))
    }

    /// サーバー from を to の位置へ動かす（下へ動かしたら to の後ろ、上へなら前）。
    /// 並びを settings に写すので、呼んだ側で save_settings する。動いたら true
    pub fn move_guild(&mut self, from: &str, to: &str) -> bool {
        let (Some(fi), Some(ti)) = (self.guilds.iter().position(|g| g.id == from), self.guilds.iter().position(|g| g.id == to)) else { return false };
        if fi == ti {
            return false;
        }
        let g = self.guilds.remove(fi);
        self.guilds.insert(ti, g);
        // フォルダがある場合は、同じフォルダ内の並び替え・トップレベルの単体サーバーの移動も反映する
        if !self.settings.guild_folders.is_empty() {
            if let Some(folder) = self.settings.guild_folders.iter_mut().find(|f| f.guild_ids.iter().any(|x| x == from) && f.guild_ids.iter().any(|x| x == to)) {
                if let (Some(a), Some(b)) = (folder.guild_ids.iter().position(|x| x == from), folder.guild_ids.iter().position(|x| x == to)) {
                    let id = folder.guild_ids.remove(a);
                    folder.guild_ids.insert(b, id);
                }
            } else if let (Some(a), Some(b)) = (
                self.settings.guild_folders.iter().position(|f| f.id.is_none() && f.guild_ids.len() == 1 && f.guild_ids[0] == from),
                self.settings.guild_folders.iter().position(|f| f.guild_ids.iter().any(|x| x == to)),
            ) {
                let entry = self.settings.guild_folders.remove(a);
                self.settings.guild_folders.insert(b, entry);
            }
        }
        // 抜けたサーバーの分も含めて覚えておく（一時的に一覧にないだけかもしれないので）
        let shown: std::collections::HashSet<&str> = self.guilds.iter().map(|g| g.id.as_str()).collect();
        let mut order: Vec<String> = self.guilds.iter().map(|g| g.id.clone()).collect();
        order.extend(self.settings.guild_order.iter().filter(|id| !shown.contains(id.as_str())).cloned());
        self.settings.guild_order = order;
        true
    }

    /// 抜けた・消えたサーバーを一覧から外す
    pub fn remove_guild(&mut self, gid: &str) {
        self.guilds.retain(|g| g.id != gid);
        self.channels.remove(gid);
        self.unread.retain(|_, g| g.as_deref() != Some(gid));
        self.mention_counts.retain(|_, (g, _)| g.as_deref() != Some(gid));
        self.my_roles.remove(gid);
        self.members.remove(gid);
        self.requested_members.remove(gid);
        self.guild_settings.remove(gid);
        if self.settings.last_channels.remove(gid).is_some() {
            self.save_settings();
        }
        if self.guild_emojis.remove(gid).is_some() {
            self.emoji_rev += 1;
        }
        if self.sel_guild.as_deref() == Some(gid) {
            self.sel_guild = None;
            self.sel_channel = None;
        }
        self.cache_dirty = true;
    }

    /// サーバーのチャンネルをすべて既読にする
    pub fn mark_guild_read(&mut self, gid: &str) {
        self.unread.retain(|_, g| g.as_deref() != Some(gid));
        self.mention_counts.retain(|_, (g, _)| g.as_deref() != Some(gid));
        self.api(net::ApiCall::AckGuild(gid.to_string()));
    }

    /// 自分あてのメンションか（@自分・@everyone/@here・自分のロール）
    pub fn mentions_me(&self, m: &Message) -> bool {
        let Some(me) = &self.me else { return false };
        if m.mentions.iter().any(|u| u.id == me.id) || m.mention_everyone {
            return true;
        }
        let gid = m.guild_id.as_deref().unwrap_or("");
        self.my_roles.get(gid).is_some_and(|mine| m.mention_roles.iter().any(|r| mine.contains(r)))
    }

    /// サーバー（None なら DM）の未読メンション数の合計
    pub fn mention_count(&self, gid: Option<&str>) -> u32 {
        self.mention_counts.values().filter(|(g, _)| g.as_deref() == gid).map(|(_, n)| n).sum()
    }

    /// 未読のある DM と件数（新しい順）。レールに相手のアイコンを並べるのに使う
    pub fn unread_dms(&self) -> Vec<(&Channel, u32)> {
        self.dms
            .iter()
            .filter_map(|c| match self.mention_counts.get(&c.id) {
                Some((None, n)) if *n > 0 => Some((c, *n)),
                _ => None,
            })
            .collect()
    }


    /// サーバーの通知設定を変える（先に画面へ反映し、結果で上書きする）
    pub fn set_guild_settings(&mut self, gid: &str, muted: Option<bool>, notifications: Option<u8>) {
        let s = self.guild_settings.entry(gid.to_string()).or_default();
        let mut body = serde_json::Map::new();
        if let Some(m) = muted {
            s.muted = m;
            body.insert("muted".into(), m.into());
            if m {
                body.insert("mute_config".into(), serde_json::json!({ "selected_time_window": -1, "end_time": null }));
            }
        }
        if let Some(n) = notifications {
            s.notifications = n;
            body.insert("message_notifications".into(), n.into());
        }
        self.api(net::ApiCall::ModifyGuildSettings { guild_id: gid.to_string(), settings: serde_json::Value::Object(body) });
    }

    /// サーバーの招待リンクを作ってコピーする（見えている最初のテキストチャンネルで作る）
    pub fn create_guild_invite(&mut self, gid: &str) {
        let Some(cid) = self.channels.get(gid).and_then(|cs| cs.iter().find(|c| c.is_text()).map(|c| c.id.clone())) else {
            self.ensure_guild_channels(gid);
            self.show_toast("チャンネルを読み込み中です。少し待ってからもう一度お試しください");
            return;
        };
        self.api(net::ApiCall::CreateInvite { channel_id: cid, body: serde_json::json!({ "max_age": 604800, "max_uses": 0, "temporary": false }) });
    }

    /// 画面下に短いお知らせを出す（「コピーしました」など）
    pub fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), std::time::Instant::now()));
        self.repaint_after(TOAST_TTL);
    }

    /// 結果は選択を開始したチャンネルに属する。画面が変わっても失わず、別の宛先に添付しない。
    pub fn poll_file_pick(&mut self) {
        let result = self.file_pick.as_ref().map(|(channel_id, rx)| (channel_id.clone(), rx.try_recv()));
        match result {
            Some((channel_id, Ok(paths))) => {
                self.file_pick = None;
                if !paths.is_empty() {
                    if self.sel_channel.as_ref() != Some(&channel_id) {
                        self.status = "選択したファイルは元のチャンネルに戻ると添付されます。".into();
                    }
                    self.picked_files = Some((channel_id, paths));
                }
            }
            Some((_, Err(std::sync::mpsc::TryRecvError::Disconnected))) => self.file_pick = None,
            _ => {}
        }
    }

    /// OS のファイル選択ダイアログを別スレッドで開く。
    pub fn pick_files(&mut self) {
        if self.file_pick.is_some() || self.picked_files.is_some() {
            self.status = "先に開いたファイル選択を完了し、元のチャンネルで確認してください。".into();
            return;
        }
        if let Some(channel_id) = self.sel_channel.clone() {
            self.file_pick = Some((channel_id, spawn_file_dialog(&self.waker, "ファイルをアップロード", true, None)));
        }
    }

    /// 設定のテーマを読み直し、変わっていれば配色・フォント・背景画像を適用する。force = ファイルが変わった
    pub fn apply_theme(&mut self, force: bool) {
        let dir = themes_dir();
        let (r, err, warnings) = theme::resolve(&self.settings.theme, &dir, &self.settings.color_overrides, &self.settings.background);
        self.theme_error = err;
        self.theme_warnings = warnings;
        let old = self.theme_applied.as_ref();
        if !force && old == Some(&r) {
            return;
        }
        let font_changed = old.map(|o| &o.font) != Some(&r.font);
        theme::set_colors(&r.colors);
        theme::set_parts(&r.parts);
        self.theme_dirty = true;
        if font_changed && (r.font.is_some() || old.is_some()) {
            self.font_dirty = true;
        }
        if let Some(theme::BackgroundImage { uri, .. }) = &r.background {
            if let Some(path) = uri.strip_prefix("file://") {
                // ファイルの背景は毎回読み直す（同じ名前のまま描き変えても反映させる）
                images::forget(uri);
                if let Err(e) = std::fs::metadata(path) {
                    self.theme_error = Some(format!("背景画像 {path}: {e}"));
                }
            }
        }
        self.theme_applied = Some(r);
        self.waker.wake();
    }

    /// 表示中のチャンネルの最新メッセージまでを Discord に既読として送る。
    /// ウィンドウにフォーカスがある（＝実際に見ている）ときだけ。新着が続くときは ACK_INTERVAL ごとにまとめる
    pub fn sync_read_state(&mut self, focused: bool) {
        if !focused {
            // フォーカス喪失時にはキーアップが届かないことがある。
            // PTT が押されたまま残って音声が送信され続けるのを防ぐ。
            self.voice.ptt_down.store(false, Relaxed);
            return;
        }
        let Some(cid) = self.sel_channel.clone() else { return };
        // 控えのままの一覧は最新とは限らないので、既読を古い位置に戻さないよう送らない
        if self.stale_channels.contains(&cid) { return; }
        // MessageLogger が残した削除済みメッセージは既読の位置に使わない（もう存在しない ID のため）
        let Some(last) = self.messages.get(&cid).and_then(|l| l.iter().rev().find(|m| m.log.as_ref().is_none_or(|x| x.deleted_at.is_none()))) else { return };
        if self.acked.get(&cid) == Some(&last.id) {
            // 最新まで既読にしてあるのに未読の印だけ残っていたら消す（届いたメッセージがすぐ消された、
            // つなぎ直しで件数だけ入り直した、など。残すと次のメッセージが来るまでレールのアイコンが出たままになる）
            self.unread.remove(&cid);
            self.mention_counts.remove(&cid);
            return;
        }
        if let Some(wait) = self.ack_retry_at.get(&cid).and_then(|t| t.checked_duration_since(std::time::Instant::now())) {
            self.repaint_after(wait);
            return;
        }
        if let Some(wait) = self.last_ack.and_then(|t| ACK_INTERVAL.checked_sub(t.elapsed())) {
            self.repaint_after(wait);
            return;
        }
        let message_id = last.id.clone();
        let queued = self.backend.as_ref().is_some_and(|backend| {
            backend.cmd.send(Cmd::Ack { channel_id: cid.clone(), message_id: message_id.clone() }).is_ok()
        });
        if !queued {
            // コマンドが届いていないときは既読扱いにせず、接続復帰後に再試行する。
            self.repaint_after(ACK_INTERVAL);
            return;
        }
        self.ack_retry_at.remove(&cid);
        self.acked.insert(cid.clone(), message_id);
        self.last_ack = Some(std::time::Instant::now());
        self.unread.remove(&cid);
        self.mention_counts.remove(&cid);
    }

    pub fn clear_message_error_status(&mut self, channel_id: &str) {
        if self.message_status_channel.as_deref() == Some(channel_id) {
            if self.status == MESSAGE_RELOAD_ERROR { self.status = self.gateway_status.clone(); }
            self.message_status_channel = None;
        }
    }

    pub fn load_messages(&mut self, channel_id: String) {
        if self.pending_message_loads.contains_key(&channel_id) { return; }
        self.message_errors.remove(&channel_id);
        self.pending_message_loads.insert(channel_id.clone(), Vec::new());
        if self.backend.as_ref().is_none_or(|backend| backend.cmd.send(Cmd::LoadMessages(channel_id.clone())).is_err()) {
            self.pending_message_loads.remove(&channel_id);
            self.message_errors.insert(channel_id);
        }
    }

    pub fn open_channel(&mut self, id: String) {
        if self.sel_channel.as_deref() != Some(&id) {
            if self.editing.take().is_some() {
                self.input.clear();
            }
            self.reply_to = None;
        }
        self.unread.remove(&id);
        self.mention_counts.remove(&id);
        if !self.messages.contains_key(&id) || self.stale_channels.contains(&id) {
            self.load_messages(id.clone());
        }
        self.touch_channel(&id);
        let key = self.sel_guild.as_deref().unwrap_or("");
        if self.settings.last_channels.get(key).map(String::as_str) != Some(id.as_str()) {
            self.settings.last_channels.insert(key.to_string(), id.clone());
            self.save_settings();
        }
        self.sel_channel = Some(id);
        self.focus_input = true;
        self.cache_dirty = true;
        if self.screen_w < 700.0 {
            self.compact_sidebar_closed = true;
        }
    }

    /// 開いたチャンネルを「最近」に回し、MAX_CHANNELS を超えた古いチャンネルのメッセージと行の高さを捨てる。
    /// 1チャンネル最大 MAX_MSGS 件なので、巡回するほどメモリが増え続けるのを防ぐ
    pub fn touch_channel(&mut self, id: &str) {
        if let Some(i) = self.loaded_channels.iter().position(|c| c == id) {
            self.loaded_channels.remove(i);
        }
        self.loaded_channels.push_back(id.to_string());
        let mut evicted = false;
        while self.loaded_channels.len() > MAX_CHANNELS {
            let Some(old) = self.loaded_channels.pop_front() else { break };
            self.messages.remove(&old);
            self.bump_msgs();
            self.history.remove(&old);
            self.stale_channels.remove(&old);
            self.no_access.remove(&old);
            self.message_errors.remove(&old);
            self.acked.remove(&old);
            self.ack_retry_at.remove(&old);
            evicted = true;
            // そのチャンネルでしか使っていない画像は、デコード済みの画像・テクスチャごと解放する
            if let Some(uris) = self.channel_images.remove(&old) {
                for uri in uris {
                    if !self.channel_images.values().any(|s| s.contains(&uri)) {
                        images::forget(&uri);
                    }
                }
            }
        }
        if evicted {
            self.prune_stickers();
        }
    }

    pub fn prune_stickers(&mut self) {
        let used: HashSet<&str> = self.messages.values().flatten()
            .flat_map(|m| m.sticker_items.iter().map(|s| s.id.as_str())).collect();
        self.stickers.retain(|id, _| used.contains(id.as_str()));
    }

    /// そのサーバーで最初に開くテキストチャンネル（最後に開いたもの → 公式クライアントの read_state で最後に見たもの → 一番上のチャンネル）
    pub fn default_guild_channel(&self, gid: &str) -> Option<String> {
        let Some(chs) = self.channels.get(gid) else {
            return self.settings.last_channels.get(gid).cloned();
        };
        if let Some(saved) = self.settings.last_channels.get(gid) {
            if chs.iter().any(|c| &c.id == saved && c.is_text()) {
                return Some(saved.clone());
            }
        }
        if let Some(best) = chs
            .iter()
            .filter(|c| c.is_text())
            .filter_map(|c| self.channel_read_rank.get(&c.id).copied().filter(|&r| r > (0, 0)).map(|r| (r, &c.id)))
            .max_by_key(|(r, _)| *r)
            .map(|(_, id)| id.clone())
        {
            return Some(best);
        }
        if let Some(c) = chs.iter().find(|c| c.is_text() && c.parent_id.is_none()) {
            return Some(c.id.clone());
        }
        let mut cats: Vec<&Channel> = chs.iter().filter(|c| c.kind == Channel::CATEGORY).collect();
        cats.sort_by_key(|c| c.position);
        for cat in cats {
            if let Some(c) = chs.iter().find(|c| c.is_text() && c.parent_id.as_deref() == Some(cat.id.as_str())) {
                return Some(c.id.clone());
            }
        }
        chs.iter().find(|c| c.is_text()).map(|c| c.id.clone())
    }

    /// サーバー（None なら DM）を切り替え、そのサーバーで最後に開いていたチャンネルを自動で開く
    pub fn pick_guild(&mut self, guild: Option<String>) {
        let channel = match &guild {
            Some(gid) => self.default_guild_channel(gid),
            None => self.settings.last_channels.get("").filter(|cid| self.dms.iter().any(|c| &c.id == *cid)).cloned(),
        };
        self.select(guild, channel);
    }

    /// チャンネル一覧ペインの横幅（最小 160px〜最大 480px）
    pub fn channel_pane_width(&self) -> f32 {
        let w = if self.settings.channel_pane_width > 0.0 { self.settings.channel_pane_width } else { 240.0 };
        w.clamp(160.0, 480.0)
    }

    /// メンバー一覧ペインの横幅（最小 180px〜最大 480px）
    pub fn member_pane_width(&self) -> f32 {
        let w = if self.settings.member_pane_width > 0.0 { self.settings.member_pane_width } else { 240.0 };
        w.clamp(180.0, 480.0)
    }

    pub fn current_channel(&self) -> Option<&Channel> {
        let id = self.sel_channel.as_deref()?;
        match &self.sel_guild {
            None => self.dms.iter().find(|c| c.id == id),
            Some(g) => self.channels.get(g)?.iter().find(|c| c.id == id),
        }
    }

    /// 未読チャンネルを持つサーバー（DM は None）。サーバーごとに未読全体をなめないよう1回で集める
    pub fn unread_guilds(&self) -> HashSet<Option<&str>> {
        self.unread.values().map(|g| g.as_deref()).collect()
    }

    /// チャンネルIDが属するサーバーIDを返す（DM なら None）
    pub fn guild_for_channel(&self, channel_id: &str) -> Option<String> {
        if self.sel_channel.as_deref() == Some(channel_id) && self.sel_guild.is_some() {
            return self.sel_guild.clone();
        }
        self.channels
            .iter()
            .find_map(|(gid, chs)| chs.iter().any(|c| c.id == channel_id).then(|| gid.clone()))
    }

    /// サーバー内のメンバー情報をユーザーIDで引く
    pub fn find_member(&self, guild_id: &str, user_id: &str) -> Option<&Member> {
        self.members
            .get(guild_id)?
            .iter()
            .find(|m| m.user.as_ref().is_some_and(|u| u.id == user_id))
    }

    /// チャットメッセージの投稿者のロール色（サーバーのメンバー一覧・メッセージ付属メンバー・自分のロールから求める）
    pub fn message_author_color(&self, m: &Message) -> Option<crate::color::Color32> {
        let gid = m.guild_id.as_deref().or(self.sel_guild.as_deref())?;
        let roles = self.roles.get(gid)?.as_slice();
        if let Some(member) = self.find_member(gid, &m.author.id) {
            if !member.roles.is_empty() {
                return member_role_color(member, roles);
            }
        }
        if let Some(color) = m.member.as_ref().and_then(|member| member_role_color(member, roles)) {
            return Some(color);
        }
        if self.me.as_ref().is_some_and(|me| me.id == m.author.id) {
            if let Some(my_roles) = self.my_roles.get(gid) {
                return role_color(my_roles, roles);
            }
        }
        None
    }

    /// チャットメッセージの投稿者の表示名（サーバーニックネーム → グローバル表示名 → ユーザー名）
    pub fn message_author_name<'a>(&'a self, m: &'a Message) -> &'a str {
        if let Some(nick) = m.member.as_ref().and_then(|member| member.nick.as_deref()).filter(|s| !s.is_empty()) {
            return nick;
        }
        if let Some(gid) = m.guild_id.as_deref().or(self.sel_guild.as_deref()) {
            if let Some(nick) = self.find_member(gid, &m.author.id).and_then(|member| member.nick.as_deref()).filter(|s| !s.is_empty()) {
                return nick;
            }
        }
        author_name(m)
    }

    /// チャットメッセージの投稿者のアバター URL（サーバー別アバター → ユーザーアバター）
    pub fn message_author_avatar_url(&self, m: &Message) -> String {
        if let Some(gid) = m.guild_id.as_deref().or(self.sel_guild.as_deref()) {
            let guild_av = m
                .member
                .as_ref()
                .and_then(|mem| mem.avatar.as_deref())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    self.find_member(gid, &m.author.id)
                        .and_then(|mem| mem.avatar.as_deref())
                        .filter(|s| !s.is_empty())
                });
            if let Some(h) = guild_av {
                let ext = if h.starts_with("a_") { "gif" } else { "png" };
                return format!(
                    "https://cdn.discordapp.com/guilds/{gid}/users/{}/avatars/{h}.{ext}?size=80",
                    m.author.id
                );
            }
        }
        user_avatar_url(&m.author)
    }

    /// チャットメッセージの投稿者のアバターデコレーション URL（サーバー別 → ユーザー設定）
    pub fn message_author_decoration_url(&self, m: &Message) -> Option<String> {
        if let Some(asset) = m.member.as_ref().and_then(Member::decoration_asset) {
            return Some(decoration_url(asset));
        }
        if let Some(gid) = m.guild_id.as_deref().or(self.sel_guild.as_deref()) {
            if let Some(asset) = self.find_member(gid, &m.author.id).and_then(Member::decoration_asset) {
                return Some(decoration_url(asset));
            }
        }
        user_decoration_url(&m.author)
    }

    /// REST で取得したメッセージ群について、まだメンバー情報（ロール・ニックネーム）を持たない投稿者をゲートウェイ（op 8）でまとめて問い合わせる
    pub fn request_missing_message_members(&mut self, channel_id: &str, messages: &[Message]) {
        let Some(gid) = self.guild_for_channel(channel_id) else { return };
        let known: HashSet<&str> = self
            .members
            .get(&gid)
            .into_iter()
            .flatten()
            .filter_map(|m| m.user.as_ref().map(|u| u.id.as_str()))
            .collect();
        let requested = self.requested_members.entry(gid.clone()).or_default();
        let mut to_fetch = Vec::new();
        for m in messages {
            for uid in std::iter::once(m.author.id.as_str()).chain(m.referenced_message.as_deref().map(|r| r.author.id.as_str())) {
                if !uid.is_empty() && !known.contains(uid) && requested.insert(uid.to_string()) {
                    to_fetch.push(uid.to_string());
                }
            }
        }
        for chunk in to_fetch.chunks(100) {
            self.send(Cmd::Gateway(net::request_guild_members(&gid, None, Some(chunk), 0, false)));
        }
    }
}

#[cfg(test)]
mod regression_tests;
