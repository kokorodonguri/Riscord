//! 画面（gpui）。状態とロジックは crate::app にあり、ここではそれを描いて操作をつなぐ
mod channel_create;
mod channel_permissions;
mod chat;
mod commands;
mod dialogs;
mod emoji;
mod forum;
mod friends;
mod guild_profile;
mod kit;
mod login;
mod members;
mod message;
mod moderation;
mod panel;
mod popup;
mod role_admin;
mod server_settings;
mod settings;
mod sidebar;
mod soundboard;
mod split;
mod voice_stage;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use gpui::component::input::{InputEvent, InputState, TextareaState};
use gpui::component::slider::SliderState;
use gpui::prelude::*;
use gpui::*;

use crate::app::{self as st, App as AppState};
use crate::color::Color32;
use crate::theme::*;
use crate::wake::Waker;
pub use kit::*;

// 即時通知は受信側が取り出すまで一つにまとめる。遅延通知はそのまま送る。
fn enqueue_wake(
    tx: &futures_channel::mpsc::UnboundedSender<Option<Duration>>,
    immediate_pending: &AtomicBool,
    delay: Option<Duration>,
) {
    if delay.is_none() && immediate_pending.swap(true, Ordering::AcqRel) {
        return;
    }
    if tx.unbounded_send(delay).is_err() && delay.is_none() {
        immediate_pending.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod wake_queue_tests {
    use super::enqueue_wake;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    #[test]
    fn immediate_wakes_coalesce_and_rearm_without_losing_delayed_wakes() {
        let (tx, mut rx) = futures_channel::mpsc::unbounded();
        let pending = AtomicBool::new(false);
        for _ in 0..10_000 {
            enqueue_wake(&tx, &pending, None);
        }
        let delay = Duration::from_millis(250);
        enqueue_wake(&tx, &pending, Some(delay));
        assert_eq!(rx.try_recv().unwrap(), None);
        assert_eq!(rx.try_recv().unwrap(), Some(delay));
        assert!(rx.try_recv().is_err());

        // 受信側は notify 前に解除するので、その直後の通知も別に届く。
        pending.store(false, Ordering::Release);
        enqueue_wake(&tx, &pending, None);
        assert_eq!(rx.try_recv().unwrap(), None);
    }

    #[test]
    #[ignore = "manual performance measurement"]
    fn bench_wake_burst() {
        use std::hint::black_box;
        use std::time::Instant;
        for coalesce in [false, true] {
            let mut times = Vec::new();
            let mut count = 0;
            for _ in 0..20 {
                let (tx, mut rx) = futures_channel::mpsc::unbounded();
                let pending = AtomicBool::new(false);
                let start = Instant::now();
                for _ in 0..100_000 {
                    if coalesce {
                        enqueue_wake(&tx, &pending, None);
                    } else {
                        tx.unbounded_send(None).unwrap();
                    }
                }
                count = 0;
                while rx.try_recv().is_ok() {
                    count += 1;
                }
                black_box(count);
                times.push(start.elapsed());
            }
            times.sort_unstable();
            eprintln!(
                "coalesce={coalesce}: queued={count}, median={:?}, p95={:?}",
                times[10], times[19]
            );
        }
    }
}

gpui::assets::icon_assets!(
    ExtraIcons,
    [
        ArrowLeft,
        ArrowRight,
        AtSign,
        Download,
        ChevronDown,
        ChevronRight,
        CirclePlus,
        CircleX,
        ClipboardPaste,
        Copy,
        FileUp,
        Folder,
        Hash,
        HeadphoneOff,
        Headphones,
        Menu,
        Mic,
        MicOff,
        Palette,
        PanelRight,
        Pencil,
        Phone,
        PhoneOff,
        Play,
        Plus,
        RefreshCw,
        Reply,
        RotateCw,
        Search,
        Settings,
        Signal,
        Trash,
        Upload,
        User,
        UserRound,
        Users,
        Video,
        Volume2,
        X,
        Check,
        IdCard,
        LogOut,
        Info,
        TriangleAlert,
        Paperclip,
        SignalHigh,
        File,
        Bell,
        BellOff,
        UserPlus,
        CheckCheck,
        Pin,
        PinOff,
        ArrowUp,
        MessagesSquare,
        Archive,
        Lock,
        Tag,
        MonitorUp,
        MonitorOff,
        FaceSlightlySmiling
    ]
);

/// 同梱のアイコン（gpui-component が使う分）と、このアプリで使う分を合わせたもの
struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(b) = ExtraIcons.load(path)? {
            return Ok(Some(b));
        }
        gpui::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut v = ExtraIcons.list(path)?;
        v.extend(gpui::assets::Assets.list(path)?);
        Ok(v)
    }
}

pub fn run() {
    gpui::application()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
            gpui::init(cx);
            let bounds = Bounds::centered(None, size(px(1200.0), px(760.0)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(320.0), px(400.0))),
                app_id: Some("riscord".into()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Riscord".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let opened = gpui::open_window(options, cx, |window, cx| {
                cx.new(|cx| split::Shell::new(window, cx))
            });
            if let Err(e) = opened {
                eprintln!("ウィンドウを開けませんでした: {e}");
                cx.quit();
            }
        });
}

/// 1チャンネル分のメッセージ一覧（gpui の list が高さを測り、スクロール位置を保つ）
pub struct MsgList {
    pub state: ListState,
    /// 前に渡した行の中身の目印（変わった行だけ測り直す）
    pub keys: Vec<u64>,
    /// 前に渡した各行のメッセージ ID の目印（先頭・末尾の番兵は 0）。読み足しの前後で見ていた行を探す
    pub ids: Vec<u64>,
    /// keys を作ったときの一覧の状態（AppState::msg_rev など）。同じなら keys は作り直さない
    pub stamp: u64,
    /// 最新とつながっていない一覧を出している（その間は下端に付いていかない）
    pub detached: bool,
}

/// メッセージ本文を描く前の形（プラグインの表示加工 → メンションの名前 → 記法の分解）の控え。
/// 見えている行は描き直すたびに Lua の呼び出しと記法の解析をしていたので、元が同じ間は使い回す
pub struct BodyCache {
    /// 元の本文・チャンネル・プラグインの状態（revision）。変わったら表示加工をやり直す
    /// （本文は毎フレームハッシュせず、そのまま比べる。長さが違えばすぐ分かる）
    pub content: String,
    pub channel_id: String,
    pub plugin_rev: u64,
    /// on_display を通した後の本文
    pub displayed: String,
    /// メンションの名前を入れた後の本文の目印（チャンネル名・ロール名が変わったら解析し直す）
    pub resolved: u64,
    /// 最後にメンションの名前を入れたときのチャンネル名・ロール名の版とロール一覧。
    /// MentionFix などで本文を書き換えない（借りたまま）ときは、これが同じなら入れ直さない
    pub names_key: Option<(u64, usize, usize)>,
    pub blocks: Rc<[crate::markdown::Block]>,
}

/// 本文の控えの上限。
pub const BODY_CACHE_MAX: usize = 2048;

/// チャンネルの書きかけ（別のチャンネルへ移ったときにしまっておくもの）
#[derive(Default)]
pub struct Draft {
    pub text: String,
    pub uploads: Vec<crate::clip::Upload>,
}

/// 入力欄（gpui-component）。中身は AppState の文字列と同期する
pub struct Inputs {
    pub compose: Entity<TextareaState>,
    pub dm_filter: Entity<InputState>,
    pub switcher: Entity<InputState>,
    pub email: Entity<InputState>,
    pub password: Entity<InputState>,
    pub mfa: Entity<InputState>,
    pub settings_query: Entity<InputState>,
    pub name: Entity<InputState>,
    pub pronouns: Entity<InputState>,
    pub bio: Entity<TextareaState>,
    pub bg_image: Entity<InputState>,
    /// 絵文字ピッカーの検索欄
    pub emoji_query: Entity<InputState>,
    /// メッセージ検索の欄
    pub search: Entity<InputState>,
    /// メッセージから作るスレッドの名前
    pub thread_name: Entity<InputState>,
    pub role_name: Entity<InputState>,
    pub guild_name: Entity<InputState>,
    pub guild_description: Entity<TextareaState>,
    pub channel_member_id: Entity<InputState>,
    pub friend_name: Entity<InputState>,
    pub folder_name: Entity<InputState>,
    pub poll_question: Entity<InputState>,
    pub poll_answers: Entity<TextareaState>,
    /// 作るチャンネル・カテゴリの名前
    pub channel_name: Entity<InputState>,
    /// このサーバーでのプロフィール
    pub guild_nick: Entity<InputState>,
    pub guild_pronouns: Entity<InputState>,
    pub guild_bio: Entity<TextareaState>,
    /// サーバー管理: イベントと AutoMod ルールの下書き
    pub event_name: Entity<InputState>,
    pub event_start: Entity<InputState>,
    pub event_end: Entity<InputState>,
    pub event_location: Entity<InputState>,
    pub event_description: Entity<TextareaState>,
    pub rule_name: Entity<InputState>,
    pub rule_keywords: Entity<TextareaState>,
    /// フォーラムの新しい投稿の題と本文
    pub post_title: Entity<InputState>,
    pub post_body: Entity<TextareaState>,
}

/// 境界をドラッグして横幅を変えるペイン
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ResizeTarget {
    Channels,
    Members,
}

/// ポップアップ（開いている間だけ出す小さなメニュー）
#[derive(Clone, Copy, PartialEq)]
pub enum PopMenu {
    Plus,
    Mic,
    Headset,
    /// サーバーアイコンの右クリック（対象は Root::menu_guild）
    Guild,
    Folder,
    /// メッセージの右クリック（対象は Root::menu_msg）
    Message,
    /// 入力欄の絵文字ボタン
    Emoji,
    /// チャットのヘッダーの「…」（狭くて入りきらないボタン。中身は Root::header_more）
    Header,
}

pub struct Root {
    pub app: AppState,
    pub this: WeakEntity<Root>,
    pub focus: FocusHandle,
    pub inp: Inputs,
    pub lists: HashMap<String, MsgList>,
    /// 行の高さに効く設定（変わったら全部測り直す）
    pub list_key: (u32, u32, bool, u32, u32),
    /// ペインの境界のドラッグ（対象, 押したときのマウスの X, そのときの幅）
    pub resize_drag: Option<(ResizeTarget, f32, f32)>,
    pub member_list: ListState,
    /// (サーバー, メンバーの版, プラグインの状態)。プラグインでタグや行の中身が変わったら測り直す
    pub member_key: (String, u64, u64),
    /// 右側の枠（ピン一覧・検索結果）の一覧と、作ったときの目印
    pub side_list: ListState,
    pub side_key: (u8, String, u64),
    /// フォーラムの投稿一覧
    pub forum_list: ListState,
    pub forum_key: (u8, String, u64),
    /// マウスが乗っているメッセージ（ツールバーを出す）
    pub hover_msg: Option<String>,
    /// スポイラーを押して開いたメッセージ
    pub revealed: std::collections::HashSet<String>,
    /// CollapseEmbeds: 既定から開閉を切り替えたメッセージ
    pub collapsed_media: std::collections::HashSet<String>,
    /// メッセージ ID ごとの本文の控え（BodyCache）
    pub body_cache: HashMap<String, BodyCache>,
    /// 最初に挿入した順。既存項目の更新では順序を変えない。
    pub body_cache_order: std::collections::VecDeque<String>,
    pub menu: Option<(PopMenu, Point<Pixels>)>,
    /// PopMenu::Guild の対象のサーバー
    pub menu_guild: Option<String>,
    /// サーバーのメニューを、チャンネル一覧の上のサーバー名から開いた（見出しの下に出す）
    pub menu_from_header: bool,
    pub guild_notify_expanded: bool,
    /// サーバー名の見出しの位置（描いたときに覚える。メニューを真下に出すため）
    pub guild_header_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub menu_folder: Option<String>,
    /// PopMenu::Message の対象のメッセージ
    pub menu_msg: Option<crate::model::Message>,
    /// ヘッダーで「…」へ回したボタン（描くたびに決め直す）
    pub header_more: Vec<chat::HeaderExtra>,
    /// 絵文字ピッカーの行（目印が変わったら作り直す）
    pub emoji_rows: Option<(emoji::EmojiKey, Rc<Vec<emoji::EmojiRow>>)>,
    /// :名前: の補完で選んでいる候補（打ちかけの名前, 何番目か）
    pub emoji_ac_sel: (String, usize),
    /// スラッシュコマンドの候補で選んでいる位置
    pub cmd_ac_sel: usize,
    /// コマンドの引数の欄（引数の名前 → 欄）と、作ったときのコマンド
    pub cmd_inputs: HashMap<String, Entity<InputState>>,
    pub cmd_subs: Vec<Subscription>,
    pub cmd_inputs_key: String,
    /// Esc で閉じた補完（: の位置, 名前）。続きを打つまで出さない
    pub emoji_ac_dismissed: Option<(usize, String)>,
    /// プロフィールのポップアップで「ステータス」のサブメニューを開いている
    pub status_sub: bool,
    pub sliders: HashMap<&'static str, Entity<SliderState>>,
    /// 設定 → テーマの色ごとの入力欄
    pub color_inputs: HashMap<&'static str, Entity<InputState>>,
    pub plugin_inputs: HashMap<(String, String), Entity<InputState>>,
    /// プラグイン一覧の絞り込み欄の購読と「有効のみ」
    pub plugin_filter_sub: Option<Subscription>,
    pub plugins_only_enabled: bool,
    /// MessageLogger のログ一覧で選んでいる種類（0 = 削除、1 = 編集、2 = ゴーストピン）
    pub log_tab: usize,
    /// MessageLogger のログ一覧の並び（記録が変わるまで使い回す）
    pub log_order: Option<settings::LogOrder>,
    /// 「別のアカウント」で削除を押したプロファイル（もう一度押すと消す）
    pub confirm_profile_delete: Option<String>,
    pub settings_scroll: ScrollHandle,
    pub font_family: SharedString,
    /// テーマで指定したフォントを読み込んだもの（パス, ファミリー名）
    pub theme_font: Option<(std::path::PathBuf, SharedString)>,
    /// 設定で選んだフォント（入っているものだけ）
    pub user_font: Option<SharedString>,
    /// 設定画面のフォント一覧（開いたときに読む）と絞り込み欄
    pub font_names: Option<Vec<SharedString>>,
    pub font_filter: Option<Entity<InputState>>,
    pub last_title: String,
    /// 入力欄の薄い案内文（チャンネルが変わったら変える）
    pub compose_hint: String,
    pub image_preview: Option<String>,
    pub video: Option<crate::video::Player>,
    /// 視聴中の画面共有の最新の 1 枚（どの配信の映像か, 画像）
    pub stream_image: Option<(String, Arc<RenderImage>)>,
    /// 進行中のダウンロード（URL, 結果の受け口）。保存先を選んでいる間も含む
    pub downloads: Vec<(String, std::sync::mpsc::Receiver<crate::download::Event>)>,
    /// 分割表示で、もう片方の区画のプロファイル（分割していなければ None）
    pub other: Option<Option<String>>,
    split_vertical: Rc<Cell<bool>>,
    pane_h: f32,
    /// この区画の印（押したメンション・絵文字をどの区画で押したか見分ける）
    pub pane_id: u64,
    pending_mentions: message::MentionQueue,
    /// 分割表示のときの区画の色・拡大率（一覧の行を作る前に戻す）。分割していなければ None
    pub pane_style: Option<PaneStyle>,
    /// チャンネルごとの書きかけ（本文・添付）。移ったらしまい、戻ったら出す
    pub drafts: HashMap<String, Draft>,
    /// いまの入力欄の中身がどのチャンネルのものか（None = まだ決めていない）
    draft_at: Option<Option<String>>,
    /// 入力欄の Esc が日本語入力の変換中に押された（返信・編集の取り消しに使わない）
    esc_composing: bool,
    /// 右の区画のプロファイルの lock（左はプロセスが起動時に取っている）
    _lock: Option<std::fs::File>,
    _subs: Vec<Subscription>,
}

impl Root {
    fn open_image_preview(&mut self, uri: String) {
        if self.image_preview.as_deref() != Some(uri.as_str()) {
            self.close_image_preview();
            self.image_preview = Some(uri);
        }
    }

    fn close_image_preview(&mut self) {
        if let Some(uri) = self.image_preview.take() {
            crate::images::forget_preview(&uri);
        }
    }

    /// 視聴中の画面共有の新しい 1 枚を画像にする（見ていなければ捨てる）
    fn poll_stream_image(&mut self) {
        let frame = self.app.voice.screen.lock().unwrap().take();
        let key = self.app.watching.as_ref().map(|(k, _)| k.as_str());
        // 視聴をやめた・別の人の配信に切り替えたら、前の映像は出さない
        if self
            .stream_image
            .as_ref()
            .is_some_and(|(k, _)| Some(k.as_str()) != key)
        {
            if let Some((_, old)) = self.stream_image.take() {
                crate::images::release(old);
            }
        }
        let (Some(key), Some(f)) = (key, frame) else {
            return;
        };
        let Some(buf) = image::RgbaImage::from_raw(f.width, f.height, f.data) else {
            return;
        };
        let frame = image::Frame::from_parts(
            buf,
            0,
            0,
            image::Delay::from_saturating_duration(Duration::ZERO),
        );
        if let Some((_, old)) = self
            .stream_image
            .replace((key.to_string(), Arc::new(RenderImage::new(vec![frame]))))
        {
            crate::images::release(old);
        }
    }
}

thread_local! {
    static WINDOW_TITLE: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// 区画から分割表示の入れ物（split::Shell）への頼みごと
pub enum PaneEvent {
    /// このプロファイル（None は既定）を横に並べて開く
    OpenSplit(Option<String>),
}

impl EventEmitter<PaneEvent> for Root {}

fn new_input(
    window: &mut Window,
    cx: &mut Context<Root>,
    placeholder: &str,
    masked: bool,
) -> Entity<InputState> {
    let p = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(p).masked(masked))
}

/// 入力欄の中身を文字列に写す（打ったとき）。limit = 文字数の上限
fn bind<M: gpui::component::input::InputModeKind + 'static>(
    cx: &mut Context<Root>,
    window: &mut Window,
    e: &Entity<gpui::base::input::InputBaseState<M>>,
    limit: Option<usize>,
    field: fn(&mut AppState) -> Option<&mut String>,
) -> Subscription {
    cx.subscribe_in(
        e,
        window,
        move |this: &mut Root, state, ev: &InputEvent, window, cx| {
            if let InputEvent::Change = ev {
                let logic = field(&mut this.app).map(|f| f.clone());
                let v = match &logic {
                    Some(l) => changed_value(state, l, window, cx),
                    None => Some(state.read(cx).value().to_string()),
                };
                // 変換中で、ロジック側の変更をまだ合わせられない（確定したら合わせる）
                let Some(mut v) = v else {
                    cx.notify();
                    return;
                };
                if let Some(l) = limit {
                    if v.chars().count() > l {
                        v = v.chars().take(l).collect();
                        let v2 = v.clone();
                        state.update(cx, |s, cx| s.set_value(v2, window, cx));
                    }
                }
                note_synced(state.entity_id(), &v);
                if let Some(f) = field(&mut this.app) {
                    *f = v;
                }
                cx.notify();
            }
        },
    )
}

thread_local! {
    /// 入力欄ごとの「欄とロジック側の文字列が最後にそろったときの中身」。
    /// 変換中に写せなかったロジック側の変更を、確定したあとに反映するために覚える
    static SYNCED: std::cell::RefCell<HashMap<EntityId, String>> = Default::default();
}

fn note_synced(id: EntityId, s: &str) {
    SYNCED.with_borrow_mut(|m| match m.get_mut(&id) {
        Some(v) if v == s => {}
        Some(v) => s.clone_into(v),
        None => {
            m.insert(id, s.to_string());
        }
    });
}

/// 変換中に写せなかったロジック側の変更と、確定後の欄の中身を合わせる。
/// prev = 最後にそろった中身、logic = ロジック側の今の中身、widget = 確定後の欄の中身。
/// ロジック側が後ろに足しただけ（貼り付け・プラグインの挿入）なら、確定した文の後ろに足す。
/// それ以外（送信で空にした・チャンネルを移った等）はロジック側を優先する
fn merge_pending(prev: &str, logic: &str, widget: &str) -> String {
    match logic.strip_prefix(prev) {
        Some(added) => format!("{widget}{added}"),
        None => logic.to_string(),
    }
}

/// 欄が変わったとき、ロジック側に写せていない変更があれば合わせた中身（無ければ None）
fn pending_merge(id: EntityId, logic: &str, widget: &str) -> Option<String> {
    if logic == widget {
        return None;
    }
    SYNCED.with_borrow(|m| {
        m.get(&id)
            .filter(|prev| prev.as_str() != logic)
            .map(|prev| merge_pending(prev, logic, widget))
    })
}

/// 欄が変わったときにロジック側へ写す中身。ロジック側の保留分があれば合わせて欄にも写す。
/// まだ変換中で合わせられないなら None（確定したときの変更でもう一度呼ばれる）
fn changed_value<M: gpui::component::input::InputModeKind + 'static>(
    e: &Entity<gpui::base::input::InputBaseState<M>>,
    logic: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<String> {
    use gpui::EntityInputHandler as _;
    let value = e.read(cx).value().to_string();
    let id = e.entity_id();
    let Some(merged) = pending_merge(id, logic, &value) else {
        note_synced(id, &value);
        return Some(value);
    };
    if e.update(cx, |st, cx| st.marked_text_range(window, cx).is_some()) {
        return None;
    }
    e.update(cx, |st, cx| {
        st.set_value(merged.clone(), window, cx);
        st.set_selected_range(merged.len()..merged.len(), cx);
    });
    note_synced(id, &merged);
    Some(merged)
}

/// 入力欄が日本語入力の変換中（未確定の文字がある）か
fn has_marked<M: gpui::component::input::InputModeKind + 'static>(
    e: &Entity<gpui::base::input::InputBaseState<M>>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    use gpui::EntityInputHandler as _;
    e.update(cx, |st, cx| st.marked_text_range(window, cx).is_some())
}

/// 文字列が（ロジック側で）変わっていたら入力欄に写す。
/// 日本語入力の変換中は、入力欄の中身に未確定の文字が入っているのに変更の通知（InputEvent::Change）が来ないので、
/// 食い違って見えても写さない（写すと変換が消え、複数行の欄ではカーソルが先頭に戻る）。
/// 写したらカーソルは末尾に置く（続きを打てるように）
fn sync<M: gpui::component::input::InputModeKind + 'static>(
    e: &Entity<gpui::base::input::InputBaseState<M>>,
    s: &str,
    window: &mut Window,
    cx: &mut App,
) {
    use gpui::EntityInputHandler as _;
    if e.read(cx).value().as_ref() == s {
        note_synced(e.entity_id(), s);
        return;
    }
    // 変換中は写さない。ロジック側の変更は、確定したときの変更の通知で合わせる（changed_value）
    let written = e.update(cx, |st, cx| {
        if st.marked_text_range(window, cx).is_some() {
            return false;
        }
        st.set_value(s.to_string(), window, cx);
        st.set_selected_range(s.len()..s.len(), cx);
        true
    });
    if written {
        note_synced(e.entity_id(), s);
    }
}

/// 表示言語に合う既定フォントのファミリー名。中国語なら簡体字の字形のフォント、
/// それ以外は日本語のフォント（英語も日本語の名前などを出すため）。起動時の言語で決める
fn default_family() -> SharedString {
    let zh = crate::i18n::lang() == crate::i18n::Lang::Zh;
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let pattern = if zh {
            "sans:lang=zh-cn"
        } else {
            "sans:lang=ja"
        };
        if let Ok(o) = std::process::Command::new("fc-match")
            .args(["-f", "%{family[0]}", pattern])
            .output()
        {
            if let Ok(s) = String::from_utf8(o.stdout) {
                if !s.trim().is_empty() {
                    return s.trim().to_string().into();
                }
            }
        }
        if zh {
            "Noto Sans CJK SC"
        } else {
            "Noto Sans CJK JP"
        }
        .into()
    }
    #[cfg(windows)]
    {
        // Yu Gothic UI には簡体字だけの字（们・设 など）が無い
        if zh {
            "Microsoft YaHei UI"
        } else {
            "Yu Gothic UI"
        }
        .into()
    }
    #[cfg(target_os = "macos")]
    {
        if zh { "PingFang SC" } else { "Hiragino Sans" }.into()
    }
}

impl Root {
    /// profile = この区画のアカウントのプロファイル。primary = ウィンドウの最初の区画
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        profile: Option<String>,
        primary: bool,
        lock: Option<std::fs::File>,
        split_vertical: Rc<Cell<bool>>,
    ) -> Self {
        // 別スレッドからの「描き直して」を受け取る。急がないものは期限までにまとめて1回にする
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<Option<Duration>>();
        let immediate_pending = Arc::new(AtomicBool::new(false));
        let sender_pending = immediate_pending.clone();
        let waker = Waker::new(move |d| {
            enqueue_wake(&tx, &sender_pending, d);
        });
        if primary {
            // 画像が届いたらこの区画を描き直す（ウィンドウごと描き直るので、もう片方の区画にも出る）
            crate::images::set_waker(waker.clone());
            // GitHub の main の新しいコミット（読むだけなのでプロファイルに関係なく）
            crate::commits::start(&waker);
            // 実行ファイルの置き換えが重ならないよう、更新は既定のプロファイルのウィンドウだけで行う
            if crate::profile::name().is_none() {
                crate::update::start(&waker);
            }
        }
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        cx.spawn(async move |this: WeakEntity<Root>, cx: &mut AsyncApp| {
            while let Some(d) = rx.next().await {
                match d {
                    None => {
                        // notify より前に解除する。描画中に届いたイベントも次の描画を起こせる。
                        immediate_pending.store(false, Ordering::Release);
                        // notify は次の描画までまとめられるので、続けて届いても描くのは1回
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                    Some(d) => {
                        let at = Instant::now() + d;
                        if deadline
                            .get()
                            .is_some_and(|t| t <= at && t > Instant::now())
                        {
                            continue;
                        }
                        deadline.set(Some(at));
                        let (this, deadline) = (this.clone(), deadline.clone());
                        cx.spawn(async move |cx: &mut AsyncApp| {
                            cx.background_executor().timer(d).await;
                            if deadline.get() == Some(at) {
                                deadline.set(None);
                            }
                            let _ = this.update(cx, |_, cx| cx.notify());
                        })
                        .detach();
                    }
                }
            }
        })
        .detach();

        let compose = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 12)
                .submit_on_enter(true)
        });
        let bio = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 8)
                .placeholder(tr!(
                    "自分について書いてみましょう",
                    "Write something about yourself",
                    "写点关于你自己的内容吧"
                ))
        });
        let inp = Inputs {
            compose,
            dm_filter: new_input(
                window,
                cx,
                tr!("会話を検索", "Search conversations", "搜索对话"),
                false,
            ),
            switcher: new_input(
                window,
                cx,
                tr!(
                    "会話・チャンネル・サーバーを検索",
                    "Search conversations, channels and servers",
                    "搜索对话、频道和服务器"
                ),
                false,
            ),
            email: new_input(window, cx, "", false),
            password: new_input(window, cx, "", true),
            mfa: new_input(window, cx, "123456", false),
            settings_query: new_input(window, cx, tr!("検索", "Search", "搜索"), false),
            name: new_input(window, cx, "", false),
            pronouns: new_input(
                window,
                cx,
                tr!("代名詞を追加", "Add pronouns", "添加代词"),
                false,
            ),
            bio,
            bg_image: new_input(
                window,
                cx,
                tr!(
                    "画像のパスか https:// の URL（空ならテーマの指定）",
                    "Image path or https:// URL (empty uses the theme's)",
                    "图片路径或 https:// URL（留空则使用主题设置）"
                ),
                false,
            ),
            emoji_query: new_input(
                window,
                cx,
                tr!("絵文字を検索", "Search emoji", "搜索表情"),
                false,
            ),
            search: new_input(
                window,
                cx,
                tr!(
                    "検索（from: in: has: before: after:）",
                    "Search (from: in: has: before: after:)",
                    "搜索（from: in: has: before: after:）"
                ),
                false,
            ),
            thread_name: new_input(
                window,
                cx,
                tr!("スレッド名", "Thread name", "子区名称"),
                false,
            ),
            role_name: new_input(
                window,
                cx,
                tr!("ロール名", "Role name", "身份组名称"),
                false,
            ),
            guild_name: new_input(
                window,
                cx,
                tr!("サーバー名", "Server name", "服务器名称"),
                false,
            ),
            guild_description: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 4)
                    .placeholder(tr!("サーバーの説明", "Server description", "服务器说明"))
            }),
            channel_member_id: new_input(
                window,
                cx,
                tr!("メンバー ID", "Member ID", "成员 ID"),
                false,
            ),
            friend_name: new_input(window, cx, tr!("ユーザー名", "Username", "用户名"), false),
            folder_name: new_input(
                window,
                cx,
                tr!("フォルダ名", "Folder name", "文件夹名称"),
                false,
            ),
            poll_question: new_input(window, cx, tr!("質問", "Question", "问题"), false),
            poll_answers: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 6)
                    .placeholder(tr!(
                        "選択肢を1行ずつ入力",
                        "Enter one answer per line",
                        "每行输入一个选项"
                    ))
            }),
            channel_name: new_input(window, cx, tr!("名前", "Name", "名称"), false),
            guild_nick: new_input(window, cx, tr!("ニックネーム", "Nickname", "昵称"), false),
            guild_pronouns: new_input(window, cx, tr!("代名詞", "Pronouns", "代词"), false),
            guild_bio: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 6)
                    .placeholder(tr!("自己紹介", "About Me", "个人简介"))
            }),
            event_name: new_input(
                window,
                cx,
                tr!("イベント名", "Event name", "活动名称"),
                false,
            ),
            event_start: new_input(
                window,
                cx,
                tr!(
                    "開始 YYYY-MM-DD HH:MM",
                    "Start YYYY-MM-DD HH:MM",
                    "开始 YYYY-MM-DD HH:MM"
                ),
                false,
            ),
            event_end: new_input(
                window,
                cx,
                tr!(
                    "終了 YYYY-MM-DD HH:MM",
                    "End YYYY-MM-DD HH:MM",
                    "结束 YYYY-MM-DD HH:MM"
                ),
                false,
            ),
            event_location: new_input(window, cx, tr!("場所", "Location", "地点"), false),
            event_description: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 5)
                    .placeholder(tr!(
                        "説明（任意）",
                        "Description (optional)",
                        "说明（可选）"
                    ))
            }),
            rule_name: new_input(window, cx, tr!("ルール名", "Rule name", "规则名称"), false),
            rule_keywords: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 6)
                    .placeholder(tr!(
                        "ブロックする語をカンマか改行で区切って入力",
                        "Enter words to block, separated by commas or newlines",
                        "输入要屏蔽的词语，用逗号或换行分隔"
                    ))
            }),
            post_title: new_input(window, cx, tr!("タイトル", "Title", "标题"), false),
            post_body: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 10)
                    .placeholder(tr!("本文を入力", "Enter a message", "输入正文"))
            }),
        };
        let mut subs = vec![
            bind(cx, window, &inp.dm_filter, None, |a| Some(&mut a.dm_filter)),
            bind(cx, window, &inp.switcher, None, |a| {
                a.switcher.as_mut().map(|s| &mut s.0)
            }),
            bind(cx, window, &inp.email, None, |a| Some(&mut a.login.email)),
            bind(cx, window, &inp.password, None, |a| {
                Some(&mut a.login.password)
            }),
            bind(cx, window, &inp.mfa, None, |a| Some(&mut a.login.mfa_code)),
            bind(cx, window, &inp.settings_query, None, |a| {
                Some(&mut a.settings_query)
            }),
            bind(cx, window, &inp.emoji_query, None, |a| {
                Some(&mut a.emoji_query)
            }),
            bind(cx, window, &inp.search, None, |a| Some(&mut a.search_query)),
            bind(cx, window, &inp.role_name, Some(100), |a| {
                a.role_admin
                    .as_mut()
                    .and_then(|a| a.draft.as_mut())
                    .map(|d| &mut d.name)
            }),
            bind(cx, window, &inp.guild_name, Some(100), |a| {
                a.role_admin
                    .as_mut()
                    .and_then(|a| a.guild_edit.as_mut())
                    .map(|d| &mut d.name)
            }),
            bind(cx, window, &inp.guild_description, Some(300), |a| {
                a.role_admin
                    .as_mut()
                    .and_then(|a| a.guild_edit.as_mut())
                    .map(|d| &mut d.description)
            }),
            bind(cx, window, &inp.channel_member_id, Some(20), |a| {
                a.role_admin
                    .as_mut()
                    .and_then(|a| a.channel_permissions.as_mut())
                    .map(|v| &mut v.member_id)
            }),
            bind(cx, window, &inp.friend_name, Some(100), |a| {
                Some(&mut a.friends.username)
            }),
            bind(cx, window, &inp.folder_name, Some(100), |a| {
                Some(&mut a.folder_name)
            }),
            bind(cx, window, &inp.poll_question, Some(300), |a| {
                a.poll_draft.as_mut().map(|d| &mut d.question)
            }),
            bind(cx, window, &inp.poll_answers, None, |a| {
                a.poll_draft.as_mut().map(|d| &mut d.answers)
            }),
            bind(cx, window, &inp.channel_name, Some(100), |a| {
                a.channel_draft.as_mut().map(|d| &mut d.name)
            }),
            bind(cx, window, &inp.guild_nick, Some(st::NICK_MAX), |a| {
                a.guild_profile.as_mut().map(|e| &mut e.draft.nick)
            }),
            bind(
                cx,
                window,
                &inp.guild_pronouns,
                Some(st::PRONOUNS_MAX),
                |a| a.guild_profile.as_mut().map(|e| &mut e.draft.pronouns),
            ),
            bind(cx, window, &inp.guild_bio, Some(st::BIO_MAX), |a| {
                a.guild_profile.as_mut().map(|e| &mut e.draft.bio)
            }),
            bind(cx, window, &inp.event_name, Some(100), |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.event_draft.as_mut())
                    .map(|d| &mut d.name)
            }),
            bind(cx, window, &inp.event_start, Some(16), |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.event_draft.as_mut())
                    .map(|d| &mut d.start)
            }),
            bind(cx, window, &inp.event_end, Some(16), |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.event_draft.as_mut())
                    .map(|d| &mut d.end)
            }),
            bind(cx, window, &inp.event_location, Some(100), |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.event_draft.as_mut())
                    .map(|d| &mut d.location)
            }),
            bind(cx, window, &inp.event_description, Some(1000), |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.event_draft.as_mut())
                    .map(|d| &mut d.description)
            }),
            bind(cx, window, &inp.rule_name, Some(100), |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.rule_draft.as_mut())
                    .map(|d| &mut d.name)
            }),
            bind(cx, window, &inp.rule_keywords, None, |a| {
                a.moderation
                    .as_mut()
                    .and_then(|m| m.rule_draft.as_mut())
                    .map(|d| &mut d.keywords)
            }),
            bind(cx, window, &inp.thread_name, Some(100), |a| {
                Some(&mut a.thread_name)
            }),
            bind(cx, window, &inp.post_title, Some(100), |a| {
                Some(&mut a.post_title)
            }),
            bind(cx, window, &inp.post_body, Some(2000), |a| {
                Some(&mut a.post_body)
            }),
            bind(cx, window, &inp.name, Some(st::PROFILE_NAME_MAX), |a| {
                a.profile_draft.as_mut().map(|d| &mut d.fields[0])
            }),
            bind(
                cx,
                window,
                &inp.pronouns,
                Some(st::PROFILE_PRONOUNS_MAX),
                |a| a.profile_draft.as_mut().map(|d| &mut d.fields[1]),
            ),
            bind(cx, window, &inp.bio, Some(st::PROFILE_BIO_MAX), |a| {
                a.profile_draft.as_mut().map(|d| &mut d.fields[2])
            }),
        ];
        // 入力欄: 文字を写すほか、Enter で送信する
        subs.push(cx.subscribe_in(
            &inp.channel_member_id,
            window,
            |this: &mut Root, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.app.verify_channel_member();
                    cx.notify();
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &inp.compose,
            window,
            |this: &mut Root, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    // 変換中にロジック側が書いた分（貼り付け・プラグインの挿入）は、確定してから合わせる
                    let Some(value) = changed_value(state, &this.app.input, window, cx) else {
                        cx.notify();
                        return;
                    };
                    let grew = value.len() > this.app.input.len();
                    this.app.input = value;
                    if grew {
                        this.app.compose_typed();
                        this.app.compose_key();
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { shift: false, .. } => {
                    this.submit_input(window, cx);
                }
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &inp.friend_name,
            window,
            |this: &mut Root, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.app.friend_action(crate::app::FriendAction::Request);
                    cx.notify();
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &inp.thread_name,
            window,
            |this: &mut Root, _, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.app.submit_thread_draft();
                    cx.notify();
                }
            },
        ));
        // 検索: 手を止めたら・Enter で探す
        subs.push(cx.subscribe_in(
            &inp.search,
            window,
            |this: &mut Root, _, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => {
                    this.app.search_typed();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    this.app.run_search();
                    cx.notify();
                }
                _ => {}
            },
        ));
        subs.push(cx.subscribe_in(
            &inp.emoji_query,
            window,
            |this: &mut Root, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.pick_first_emoji(window, cx);
                }
            },
        ));
        // 背景画像の欄は、入力を終えたときに保存する
        subs.push(cx.subscribe_in(
            &inp.bg_image,
            window,
            |this: &mut Root, state, ev: &InputEvent, _, cx| match ev {
                InputEvent::Change => {
                    let v = state.read(cx).value().to_string();
                    this.app.settings.background.image = Some(v).filter(|s| !s.is_empty());
                    cx.notify();
                }
                InputEvent::Blur | InputEvent::PressEnter { .. } => {
                    this.app.save_settings();
                    this.app.apply_theme(false);
                    cx.notify();
                }
                _ => {}
            },
        ));
        // ログイン画面の Enter
        for e in [&inp.email, &inp.password, &inp.mfa] {
            subs.push(
                cx.subscribe_in(e, window, |this: &mut Root, _, ev: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.login_submit(cx);
                    }
                }),
            );
        }
        subs.push(cx.subscribe_in(
            &inp.switcher,
            window,
            |this: &mut Root, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Change = ev {
                    if let Some(s) = this.app.switcher.as_mut() {
                        s.1 = 0;
                    }
                    cx.notify();
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &inp.dm_filter,
            window,
            |this: &mut Root, _, ev: &InputEvent, _, cx| {
                if let InputEvent::Blur = ev {
                    if this.app.dm_filter.trim().is_empty() {
                        this.app.dm_search = false;
                        cx.notify();
                    }
                }
            },
        ));
        // プッシュトゥトーク: 入力欄に割り当てのあるキーでも押下を拾えるよう、アクションより前に受ける
        {
            let (this, win) = (cx.weak_entity(), window.window_handle());
            subs.push(cx.intercept_keystrokes(move |ev, window, cx| {
                if window.window_handle() != win {
                    return;
                }
                let _ = this.update(cx, |r, cx| r.on_keystroke_intercept(ev, window, cx));
            }));
        }
        // ウィンドウを閉じたときの控えの保存は split::Shell が区画ごとに行う
        let app = AppState::new(waker.clone(), profile);
        prefetch_mono_family(waker);
        set_zoom(app.settings.zoom);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut root = Root {
            app,
            this: cx.weak_entity(),
            focus,
            inp,
            lists: HashMap::new(),
            list_key: (0, 0, false, 0, 0),
            resize_drag: None,
            member_list: ListState::new(0, ListAlignment::Top, px(400.0)),
            side_list: ListState::new(0, ListAlignment::Top, px(400.0)),
            side_key: (0, String::new(), 0),
            forum_list: ListState::new(0, ListAlignment::Top, px(600.0)),
            forum_key: (0, String::new(), 0),
            member_key: (String::new(), u64::MAX, 0),
            hover_msg: None,
            revealed: Default::default(),
            collapsed_media: Default::default(),
            body_cache: HashMap::new(),
            body_cache_order: Default::default(),
            menu: None,
            menu_guild: None,
            menu_from_header: false,
            guild_notify_expanded: false,
            guild_header_bounds: Rc::default(),
            menu_folder: None,
            menu_msg: None,
            header_more: Vec::new(),
            emoji_rows: None,
            emoji_ac_sel: (String::new(), 0),
            cmd_ac_sel: 0,
            cmd_inputs: HashMap::new(),
            cmd_subs: Vec::new(),
            cmd_inputs_key: String::new(),
            emoji_ac_dismissed: None,
            status_sub: false,
            sliders: HashMap::new(),
            color_inputs: HashMap::new(),
            plugin_inputs: HashMap::new(),
            plugin_filter_sub: None,
            plugins_only_enabled: false,
            log_tab: 0,
            log_order: None,
            confirm_profile_delete: None,
            settings_scroll: ScrollHandle::new(),
            font_family: default_family(),
            theme_font: None,
            user_font: None,
            font_names: None,
            font_filter: None,
            last_title: String::new(),
            compose_hint: String::new(),
            image_preview: None,
            video: None,
            stream_image: None,
            downloads: Vec::new(),
            other: None,
            split_vertical,
            pane_h: 0.0,
            pane_id: cx.entity_id().as_u64(),
            pending_mentions: Default::default(),
            pane_style: None,
            drafts: HashMap::new(),
            draft_at: None,
            esc_composing: false,
            _lock: lock,
            _subs: subs,
        };
        root.apply_fonts(cx);
        root.apply_component_theme(window, cx);
        root
    }

    pub fn push_sub(&mut self, s: Subscription) {
        self._subs.push(s);
    }

    /// テーマで指定したフォントを読み込む（無ければ日本語の既定フォント）
    fn apply_fonts(&mut self, cx: &mut Context<Self>) {
        let want = self.app.theme_applied.as_ref().and_then(|t| t.font.clone());
        match want {
            Some(path) if self.theme_font.as_ref().map(|f| &f.0) != Some(&path) => {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let before: std::collections::HashSet<String> =
                            cx.text_system().all_font_names().into_iter().collect();
                        if cx.text_system().add_fonts(vec![bytes.into()]).is_ok() {
                            let family = cx
                                .text_system()
                                .all_font_names()
                                .into_iter()
                                .find(|n| !before.contains(n));
                            if let Some(f) = family {
                                self.theme_font = Some((path, f.into()));
                            }
                        }
                    }
                    Err(e) => {
                        self.app.theme_error = Some(tr!(
                            "フォント {}: {e}",
                            "Font {}: {e}",
                            "字体 {}：{e}",
                            path.display()
                        ))
                    }
                }
            }
            Some(_) => {}
            None => self.theme_font = None,
        }
        // 設定で選んだフォントはテーマより優先。アンインストールされていたら使わない
        let name = self.app.settings.font.trim();
        self.user_font = (!name.is_empty())
            .then(|| {
                cx.text_system()
                    .all_font_names()
                    .into_iter()
                    .find(|n| n == name)
            })
            .flatten()
            .map(SharedString::from);
    }

    pub fn family(&self) -> SharedString {
        self.user_font
            .clone()
            .or_else(|| self.theme_font.as_ref().map(|f| f.1.clone()))
            .unwrap_or_else(|| self.font_family.clone())
    }

    /// gpui-component の部品（入力欄・スライダーなど）の色をテーマに合わせる
    fn apply_component_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let family = self.family();
        let t = gpui::component::Theme::global_mut(cx);
        t.font_family = family;
        t.font_size = z(16.0);
        t.background = opaque(CHAT()).into();
        t.foreground = TEXT().into();
        t.input = BORDER().into();
        t.border = BORDER().into();
        t.caret = TEXT().into();
        t.selection = BLURPLE().gamma_multiply(0.45).into();
        t.muted = WIDGET().into();
        t.muted_foreground = FAINT().into();
        t.primary = BLURPLE().into();
        t.primary_hover = BLURPLE().lerp_to_gamma(Color32::BLACK, 0.15).into();
        t.primary_active = BLURPLE().lerp_to_gamma(Color32::BLACK, 0.25).into();
        t.primary_foreground = Color32::WHITE.into();
        t.popover = SIDEBAR().into();
        t.popover_foreground = TEXT().into();
        t.accent = SELECTED().into();
        t.accent_foreground = WHITE().into();
        t.ring = BLURPLE().into();
        t.link = LINK().into();
        t.scrollbar = Color32::TRANSPARENT.into();
        t.scrollbar_thumb = WIDGET().into();
        t.scrollbar_thumb_hover = WIDGET_HOVER().into();
        t.slider_bar = BLURPLE().into();
        t.slider_thumb = Color32::WHITE.into();
        t.secondary = WIDGET().into();
        t.secondary_hover = WIDGET_HOVER().into();
        t.secondary_foreground = TEXT().into();
        t.list_hover = HOVER().into();
        t.list_active = SELECTED().into();
        t.danger = RED().into();
        // 部品の多く（ツールチップなど）は colors ではなく tokens を見るので、同じ色にそろえる
        t.tokens = gpui::component::theme::ThemeTokens::from(&t.colors);
        window.set_rem_size(z(16.0));
        window.refresh();
    }

    /// 毎回の描画の前にやること（通信の結果を取り込む・既読・テーマの反映など）
    fn before_render(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // スレッドを作った・投稿した・消えたスレッドから親へ戻る、などで頼まれた移動
        if let Some(id) = self.app.pending_open.take() {
            self.open_channel(id);
        }
        self.app.ensure_forum();
        self.app.tick_threads();
        self.app.tick_commands();
        let vp = window.viewport_size();
        if self.other.is_some() {
            // 色・拡大率はプロセスで1つなので、区画ごとに描く前に自分のものにする
            if let Some(t) = &self.app.theme_applied {
                crate::theme::set_colors(&t.colors);
                crate::theme::set_parts(&t.parts);
            }
            set_zoom(self.app.settings.zoom);
        } else {
            self.pane_style = None;
        }
        set_current_pane(self.pane_id);
        message::register_mention_queue(self.pane_id, &self.pending_mentions);
        let split = self.other.is_some();
        let vertical = split && self.split_vertical.get();
        let width = f32::from(vp.width);
        let height = f32::from(vp.height);
        self.app.screen_w = if split && !vertical {
            (width - split::DIVIDER).max(0.0) / 2.0
        } else {
            width
        } / zoom();
        self.pane_h = (if vertical {
            (height - split::DIVIDER).max(0.0) / 2.0
        } else {
            height
        }) / zoom()
            - if split { split::HEADER_H } else { 0.0 };
        self.pane_h = self.pane_h.max(0.0);
        if self
            .app
            .theme_watch
            .as_ref()
            .is_some_and(|rx| rx.try_iter().count() > 0)
        {
            self.app.apply_theme(true);
        }
        self.app.window_focused = self.pane_active(window, cx);
        self.app.poll();
        if self
            .video
            .as_ref()
            .is_some_and(|v| self.app.sel_channel.as_deref() != Some(v.channel.as_str()))
        {
            self.video = None;
        }
        if let Some(video) = &mut self.video {
            video.poll();
            if !video.paused && video.error.is_none() {
                self.app.repaint_after(Duration::from_millis(250));
            }
        }
        self.poll_stream_image();
        if !self.downloads.is_empty() {
            use std::sync::mpsc::TryRecvError;
            let app = &mut self.app;
            // 終わったもの（保存した・失敗した・やめた）と、何も言わずに止まったものは外す
            self.downloads.retain(|(_, rx)| loop {
                match rx.try_recv() {
                    Ok(crate::download::Event::Started) => {
                        app.show_toast(tr!("ダウンロードしています…", "Downloading…", "正在下载…"))
                    }
                    Ok(crate::download::Event::Finished(path)) => {
                        app.show_toast(&tr!(
                            "保存しました: {}",
                            "Saved: {}",
                            "已保存：{}",
                            path.display()
                        ));
                        break false;
                    }
                    Ok(crate::download::Event::Failed(error)) => {
                        app.show_toast(&error);
                        break false;
                    }
                    Ok(crate::download::Event::Cancelled) | Err(TryRecvError::Disconnected) => {
                        break false
                    }
                    Err(TryRecvError::Empty) => break true,
                }
            });
        }
        let clicked = self.app.notify_clicked.lock().unwrap().take();
        if let Some((gid, cid)) = clicked {
            // 押された通知のチャンネルを開いて、ウィンドウを前に出す
            self.app.settings_open = false;
            self.pick_guild(gid);
            self.open_channel(cid);
            window.activate_window();
        }
        // ClickableMentions: 押されたメンションの人のプロフィールを開く（この区画で押されたものだけ）
        let mentions: Vec<_> = self
            .pending_mentions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect();
        for uid in mentions {
            let user = self
                .app
                .messages
                .values()
                .flatten()
                .flat_map(|m| m.mentions.iter())
                .find(|u| u.id == uid)
                .cloned();
            if let Some(user) = user {
                let p = window.mouse_position();
                self.app
                    .open_profile(user, Bounds::new(p, size(px(0.), px(0.))), false);
            }
        }
        self.app.poll_file_pick();
        self.app.poll_uploads();
        if let Some(t) = self.app.clipboard_out.take() {
            cx.write_to_clipboard(ClipboardItem::new_string(t));
        }
        self.app.save_snapshot(false);
        if !self.app.in_home() {
            self.app.poll_login();
            // QR コードは画面を開いたら用意しておく（期限切れ・取り消しのあとは押されるまで作り直さない）
            if self.app.backend.is_none() && self.app.login.qr.is_none() {
                self.app.login.qr = Some(st::QrSession::start(&self.app.waker));
            }
        }
        if take_mono_changed() {
            // 等幅フォントの名前を調べ終えた（コードの行の高さが変わる）
            for l in self.lists.values() {
                l.state.remeasure();
            }
        }
        if self.app.font_dirty {
            self.app.font_dirty = false;
            self.apply_fonts(cx);
            self.app.theme_dirty = true;
            // 文字の幅が変わるので行の高さを測り直す
            for l in self.lists.values() {
                l.state.remeasure();
            }
        }
        if self.app.theme_dirty {
            self.app.theme_dirty = false;
            self.apply_component_theme(window, cx);
        }
        if self.other.is_some() {
            // 一覧の行は後（prepaint）で作られるので、そのときに戻す区画の値を控える
            let t = self.app.theme_applied.as_ref();
            self.pane_style = Some(PaneStyle::new(
                self.pane_id,
                t.map(|t| t.colors.clone()),
                t.map(|t| t.parts.clone()),
                zoom(),
            ));
        }
        let focused = self.pane_active(window, cx);
        self.app.sync_read_state(focused);
        if !self.app.settings_open {
            self.app.mic_test = None;
        }
        for img in crate::images::take_dropped() {
            cx.drop_image(img, Some(window));
        }
        // 行の高さに効く設定が変わったら測り直す
        let (ch_w, mem_w) = self.app.pane_widths();
        let key = (
            self.app.settings.font_size.to_bits(),
            zoom().to_bits(),
            self.app.settings.compact,
            ch_w as u32,
            mem_w as u32,
        );
        if key != self.list_key {
            self.list_key = key;
            for l in self.lists.values() {
                l.state.remeasure();
            }
            self.member_list.remeasure();
        }
        // 読み込んだメッセージを捨てたチャンネルの一覧も捨てる
        if self.lists.len() > self.app.messages.len() + 1 {
            let keep = &self.app.messages;
            self.lists.retain(|k, _| keep.contains_key(k));
        }
        // チャンネルを移っていたら書きかけを入れ替える（どの経路で移っても）
        self.swap_draft();
        // 入力欄（ロジック側で中身を変えたとき）
        sync(&self.inp.compose, &self.app.input, window, cx);
        sync(&self.inp.email, &self.app.login.email, window, cx);
        sync(&self.inp.password, &self.app.login.password, window, cx);
        sync(&self.inp.mfa, &self.app.login.mfa_code, window, cx);
        sync(&self.inp.dm_filter, &self.app.dm_filter, window, cx);
        sync(
            &self.inp.settings_query,
            &self.app.settings_query,
            window,
            cx,
        );
        sync(&self.inp.search, &self.app.search_query, window, cx);
        sync(&self.inp.thread_name, &self.app.thread_name, window, cx);
        if let Some(admin) = self.app.role_admin.as_ref() {
            if let Some(draft) = &admin.draft {
                sync(&self.inp.role_name, &draft.name, window, cx);
            }
            if let Some(draft) = &admin.guild_edit {
                sync(&self.inp.guild_name, &draft.name, window, cx);
                sync(&self.inp.guild_description, &draft.description, window, cx);
            }
            if let Some(view) = &admin.channel_permissions {
                sync(&self.inp.channel_member_id, &view.member_id, window, cx);
            }
        }
        sync(&self.inp.folder_name, &self.app.folder_name, window, cx);
        if let Some(d) = &self.app.channel_draft {
            sync(&self.inp.channel_name, &d.name, window, cx);
        }
        if let Some(e) = &self.app.guild_profile {
            sync(&self.inp.guild_nick, &e.draft.nick, window, cx);
            sync(&self.inp.guild_pronouns, &e.draft.pronouns, window, cx);
            sync(&self.inp.guild_bio, &e.draft.bio, window, cx);
        }
        if let Some(m) = self.app.moderation.as_ref() {
            if let Some(d) = &m.event_draft {
                sync(&self.inp.event_name, &d.name, window, cx);
                sync(&self.inp.event_start, &d.start, window, cx);
                sync(&self.inp.event_end, &d.end, window, cx);
                sync(&self.inp.event_location, &d.location, window, cx);
                sync(&self.inp.event_description, &d.description, window, cx);
            }
            if let Some(d) = &m.rule_draft {
                sync(&self.inp.rule_name, &d.name, window, cx);
                sync(&self.inp.rule_keywords, &d.keywords, window, cx);
            }
        }
        sync(
            &self.inp.friend_name,
            &self.app.friends.username,
            window,
            cx,
        );
        if let Some(d) = &self.app.poll_draft {
            sync(&self.inp.poll_question, &d.question, window, cx);
            sync(&self.inp.poll_answers, &d.answers, window, cx);
        }
        sync(&self.inp.post_title, &self.app.post_title, window, cx);
        sync(&self.inp.post_body, &self.app.post_body, window, cx);
        if let Some((q, _)) = &self.app.switcher {
            sync(&self.inp.switcher, q, window, cx);
        }
        if let Some(d) = &self.app.profile_draft {
            sync(&self.inp.name, &d.fields[0], window, cx);
            sync(&self.inp.pronouns, &d.fields[1], window, cx);
            sync(&self.inp.bio, &d.fields[2], window, cx);
        }
        if self.app.focus_input {
            self.app.focus_input = false;
            if self.app.in_home()
                && self
                    .app
                    .sel_channel
                    .as_ref()
                    .is_some_and(|id| !self.app.no_access.contains(id))
                && !self.app.settings_open
                && self.app.switcher.is_none()
                && self.app.pending_delete.is_none()
                && self.app.confirm_leave.is_none()
                && self.app.popup.is_none()
                && self.image_preview.is_none()
            {
                self.inp.compose.update(cx, |s, cx| s.focus(window, cx));
            }
        }
        // ウィンドウのタイトル（分割表示では操作中の区画のもの）
        let title = self.window_title();
        if self.other.is_none() || self.focus.contains_focused(window, cx) {
            // 区画どうしで「今ウィンドウに出しているタイトル」を共有する（操作する区画を変えたら付け替える）
            WINDOW_TITLE.with_borrow_mut(|t| {
                if *t != title {
                    window.set_window_title(&title);
                    t.clone_from(&title);
                }
            });
            self.last_title = title;
        }
    }

    /// ウィンドウが前にあり、分割表示ならこの区画を操作中か（既読・通知の判断に使う）
    pub fn pane_active(&self, window: &Window, cx: &App) -> bool {
        window.is_window_active()
            && (self.other.is_none() || self.focus.contains_focused(window, cx))
    }

    fn window_title(&self) -> String {
        let a = &self.app;
        let base = match (a.current_channel(), &a.sel_guild) {
            (Some(c), Some(gid)) => a
                .guilds
                .iter()
                .find(|g| &g.id == gid)
                .map(|g| {
                    format!(
                        "#{} - {} - Discord",
                        c.name.as_deref().unwrap_or_default(),
                        g.name
                    )
                })
                .unwrap_or_else(|| "Discord".into()),
            (Some(c), None) => format!("@{} - Discord", c.dm_label()),
            (None, _) => "Discord".into(),
        };
        // 複数アカウントのウィンドウを並べたときに見分けられるよう、ログイン中のアカウント名を付ける
        match &a.me {
            Some(me) => format!("{base} — {}", me.display()),
            None => base,
        }
    }

    /// Enter で入力欄の中身を送る（文字数の上限を超えていたら送らずに知らせる）
    fn submit_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 削除・退出の確認中の Enter は確認のほうに使う（送信と確定が両方起きないように）
        if self.app.pending_delete.is_some() || self.app.confirm_leave.is_some() {
            return;
        }
        let a = &mut self.app;
        a.input = self.inp.compose.read(cx).value().to_string();
        let limit = st::message_text_limit(a.me.as_ref().and_then(|u| u.premium_type));
        let len = st::message_text_len(&a.input);
        if len > limit && !a.can_send_long() {
            // 送っても Discord に断られるので、送らずに入力を残して知らせる
            a.compose_notice = Some((tr!("本文は{limit}文字までです（いま{len}文字）。{}文字減らすか、分けて送ってください。", "Messages can be up to {limit} characters (currently {len}). Remove {} characters or split it into several messages.", "正文最多 {limit} 个字符（当前 {len} 个）。请删减 {} 个字符，或分成多条发送。", len - limit), Instant::now()));
            a.repaint_after(st::COMPOSE_NOTICE_TTL);
        } else if a
            .sel_channel
            .as_ref()
            .is_some_and(|cid| !a.compose_submissions.contains_key(cid))
        {
            let content = std::mem::take(&mut a.input);
            if !content.trim().is_empty() || a.editing.is_some() || !a.uploads.is_empty() {
                if let Some(cid) = a.sel_channel.clone() {
                    a.submit_compose(cid, content);
                }
            }
        }
        sync(&self.inp.compose, &self.app.input, window, cx);
        cx.notify();
    }

    /// 返信・編集をやめる
    pub fn cancel_compose(&mut self) {
        if self.app.editing.take().is_some() {
            self.app.input.clear();
        }
        self.app.reply_to = None;
    }

    /// ↑ で自分の直前のメッセージを編集（本家と同じ）
    pub fn edit_last(&mut self) {
        let a = &mut self.app;
        let me = a.me.as_ref().map(|u| u.id.clone());
        let last = a
            .sel_channel
            .as_ref()
            .and_then(|c| a.messages.get(c))
            .and_then(|l| l.iter().rev().find(|m| Some(&m.author.id) == me.as_ref()))
            .cloned();
        if let Some(m) = last {
            a.input = crate::emoji::shorten(&m.content).0;
            a.editing = Some(m);
            a.reply_to = None;
        }
    }

    /// 場所を移したあとの片付け（戻る/進むの履歴・その場所に属さない返信や編集）
    pub fn after_move(&mut self, before: (Option<String>, Option<String>)) {
        let a = &mut self.app;
        if (&a.sel_guild, &a.sel_channel) != (&before.0, &before.1) {
            a.nav_back.push(before);
            if a.nav_back.len() > 100 {
                a.nav_back.remove(0);
            }
            a.nav_fwd.clear();
        }
        self.after_nav();
    }

    /// 書きかけをチャンネルごとに控える。移っていたら、前のチャンネルの本文・添付をしまい、
    /// 移った先のチャンネルの書きかけを出す（本文だけが別のチャンネルへ付いていかないように）
    fn swap_draft(&mut self) {
        let a = &mut self.app;
        let here = a.sel_channel.clone();
        let Some(at) = self.draft_at.as_ref() else {
            // 最初は今の中身をそのチャンネルのものとする
            self.draft_at = Some(here);
            return;
        };
        if *at == here {
            return;
        }
        let old = self.draft_at.replace(here.clone()).flatten();
        // 編集中の文は書きかけではないので、離れたら捨てる
        let text = std::mem::take(&mut a.input);
        let text = if a.editing.is_some() {
            String::new()
        } else {
            text
        };
        if a.editing
            .as_ref()
            .is_some_and(|m| Some(&m.channel_id) != here.as_ref())
        {
            a.editing = None;
        }
        let uploads = if old.is_some() && a.upload_channel == old {
            std::mem::take(&mut a.uploads)
        } else {
            Vec::new()
        };
        if let Some(old) = old {
            if text.is_empty() && uploads.is_empty() {
                self.drafts.remove(&old);
            } else {
                self.drafts.insert(old, Draft { text, uploads });
            }
        }
        a.uploads.clear();
        a.compose_notice = None;
        a.upload_channel = here.clone();
        if let Some(d) = here.as_ref().and_then(|c| self.drafts.remove(c)) {
            a.input = d.text;
            a.uploads = d.uploads;
        }
    }

    fn after_nav(&mut self) {
        // 返信先・添付を片付ける前に、書きかけ（本文・添付）をチャンネルごとにしまう
        self.swap_draft();
        let a = &mut self.app;
        let here = a.sel_channel.clone();
        if a.editing
            .as_ref()
            .is_some_and(|m| Some(&m.channel_id) != here.as_ref())
        {
            a.editing = None;
            a.input.clear();
        }
        if a.reply_to
            .as_ref()
            .is_some_and(|m| Some(&m.channel_id) != here.as_ref())
        {
            a.reply_to = None;
        }
        // コマンドの下書きは選んだチャンネルのものなので、移ったら捨てる
        if a.cmd_draft.is_some() && !a.command_draft_here() {
            a.cmd_draft = None;
        }
        // ピン一覧はそのチャンネルのものなので、移ったら閉じる
        if a.pins
            .as_ref()
            .is_some_and(|p| Some(&p.channel_id) != here.as_ref())
        {
            a.close_side();
        }
        if a.upload_channel != a.sel_channel {
            a.uploads.clear();
            a.compose_notice = None;
            a.upload_channel = a.sel_channel.clone();
        }
        a.take_picked_files();
        self.hover_msg = None;
    }

    /// 戻る（true）/ 進む
    pub fn navigate(&mut self, back: bool) {
        let a = &mut self.app;
        let before = (a.sel_guild.clone(), a.sel_channel.clone());
        let (from, to) = if back {
            (&mut a.nav_back, &mut a.nav_fwd)
        } else {
            (&mut a.nav_fwd, &mut a.nav_back)
        };
        if let Some((g, c)) = from.pop() {
            to.push(before);
            let friends_home = g.is_none() && c.is_none();
            a.select(g, c);
            a.friends.open = friends_home;
        }
        self.after_nav();
    }

    /// サーバーを選ぶ（None = DM）
    pub fn pick_guild(&mut self, g: Option<String>) {
        let before = (self.app.sel_guild.clone(), self.app.sel_channel.clone());
        self.app.pick_guild(g);
        self.after_move(before);
    }

    pub fn open_channel(&mut self, id: String) {
        self.app.friends.open = false;
        let before = (self.app.sel_guild.clone(), self.app.sel_channel.clone());
        self.app.open_channel(id);
        self.after_move(before);
    }

    /// 全体のキー操作（入力欄にフォーカスがあっても効くもの）
    fn on_key_capture(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &ev.keystroke;
        // プッシュトゥトークの押下とキーの登録は on_keystroke_intercept（アクションより前）で拾う
        // 入力欄の Esc が変換中のものなら（変換の取り消し）、返信・編集の取り消しやポップアップを閉じるのに使わない
        if k.key == "escape" && std::mem::take(&mut self.esc_composing) {
            return;
        }
        if !self.app.in_home() {
            return;
        }
        if self.image_preview.is_some() {
            if k.key == "escape" {
                self.close_image_preview();
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        if self.app.settings_open {
            if k.key == "escape" {
                self.settings_escape();
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        let m = &k.modifiers;
        // Ctrl / Cmd+F: メッセージ検索
        if cmd_or_ctrl(m) && !m.alt && !m.shift && k.key == "f" && self.app.sel_channel.is_some() {
            if self.app.side != crate::app::SidePanel::Search {
                self.app.toggle_search();
            }
            self.inp.search.update(cx, |s, cx| s.focus(window, cx));
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if cmd_or_ctrl(m) && !m.alt && !m.shift && k.key == "k" {
            self.app.switcher = if self.app.switcher.is_some() {
                None
            } else {
                Some((String::new(), 0))
            };
            if self.app.switcher.is_some() {
                self.inp.switcher.update(cx, |s, cx| s.focus(window, cx));
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.app.switcher.is_some() {
            if self.switcher_key(&k.key) {
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if let Some(gid) = self.app.confirm_leave.clone() {
            match k.key.as_str() {
                "enter" => {
                    self.app.confirm_leave = None;
                    self.app.api(crate::net::ApiCall::LeaveGuild(gid));
                }
                "escape" => self.app.confirm_leave = None,
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.app.pending_delete.is_some() {
            match k.key.as_str() {
                "enter" => {
                    if let Some(m) = self.app.pending_delete.take() {
                        self.app.send(crate::net::Cmd::Delete {
                            channel_id: m.channel_id,
                            id: m.id,
                        });
                    }
                }
                "escape" => self.app.pending_delete = None,
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if k.key == "escape" && (self.app.popup.is_some() || self.menu.is_some()) {
            self.app.popup = None;
            self.menu = None;
            self.menu_msg = None;
            self.status_sub = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // macOS の Option+矢印は入力欄で単語・段落の移動に使うので、文字の欄にいるときは奪わない
        let text_focused =
            !self.focus.is_focused(window) && self.focus.contains_focused(window, cx);
        let alt_nav = m.alt && !m.control && !(cfg!(target_os = "macos") && text_focused);
        if alt_nav && matches!(k.key.as_str(), "left" | "right" | "up" | "down") {
            match k.key.as_str() {
                "left" => self.navigate(true),
                "right" => self.navigate(false),
                _ => {
                    let order = self.app.sidebar_order();
                    if !order.is_empty() {
                        let d = if k.key == "up" { -1 } else { 1 };
                        let i = self
                            .app
                            .sel_channel
                            .as_ref()
                            .and_then(|c| order.iter().position(|x| x == c));
                        let n = match i {
                            Some(i) => (i as i32 + d).clamp(0, order.len() as i32 - 1) as usize,
                            None => 0,
                        };
                        self.open_channel(order[n].clone());
                    }
                }
            }
            // ほかの Alt+キーは下の Toggle Your Stuff などへ回す
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let compose_focused = self
            .inp
            .compose
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        // PasteAndSend: Ctrl+Shift+V で貼り付けてそのまま送る
        if compose_focused
            && cmd_or_ctrl(m)
            && m.shift
            && k.key.eq_ignore_ascii_case("v")
            && self.app.plugins.on("paste-and-send")
            // 変換中は欄の中身を書き換えられない（確定のときに貼った分が消える）ので送らない
            && !has_marked(&self.inp.compose, window, cx)
        {
            if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.app.input.push_str(&text);
                sync(&self.inp.compose, &self.app.input, window, cx);
                self.submit_input(window, cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Toggle Your Stuff: 決めたキーでプラグインを切り替える
        if self.app.toggle_hotkey(k) {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // Shift+L: 最新のメッセージへ。案内が出ているときだけで、文字の欄にいるときは L を打てるようにする
        // （空の入力欄で最初に打った L を奪わないよう、入力欄が空でも奪わない）
        if m.shift
            && !m.control
            && !m.alt
            && !m.platform
            && k.key.eq_ignore_ascii_case("l")
            && self.scrolled_up()
            && !text_focused
            && !compose_focused
        {
            self.jump_to_latest();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // 入力欄で: Esc は返信・編集をやめる（↑ で直前のメッセージを編集するのは MoveUp のアクションで拾う）
        if compose_focused
            && k.key == "escape"
            && (self.app.editing.is_some() || self.app.reply_to.is_some())
        {
            self.cancel_compose();
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// 入力欄が空のときの ↑（MoveUp）: 自分の直前のメッセージを編集する。拾ったら true
    fn compose_up(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let compose = self.inp.compose.read(cx);
        if !compose.focus_handle(cx).is_focused(window)
            || !compose.value().is_empty()
            || !self.app.input.is_empty()
            || self.app.editing.is_some()
            || has_marked(&self.inp.compose, window, cx)
        {
            return false;
        }
        self.edit_last();
        cx.stop_propagation();
        cx.notify();
        true
    }

    /// 入力欄の Esc（アクション）を入力欄より先に見る。変換中なら覚えておき、
    /// あとのキー処理で返信・編集を取り消さない（入力欄が変換を消してから Esc を素通しするため）
    fn compose_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let focused = self
            .inp
            .compose
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        self.esc_composing = focused && has_marked(&self.inp.compose, window, cx);
        self.esc_composing
    }

    /// どのキー押下もアクションより前に受ける（プッシュトゥトーク）。
    /// 入力欄にキーが割り当てられている（↑・Enter・Tab など）と capture_key_down には届かないため
    fn on_keystroke_intercept(
        &mut self,
        ev: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let k = &ev.keystroke;
        // 修飾キーだけの「押した」は離したときに合成されるので、押下として扱わない
        if k.is_modifier_key() {
            return;
        }
        // 分割表示では操作中の区画だけ
        if self.other.is_some() && !self.focus.contains_focused(window, cx) {
            return;
        }
        // プッシュトゥトークのキーを決めている間は、そのキーを覚える（入力欄などには渡さない）
        if self.app.binding_ptt {
            if k.key != "escape" {
                self.app.settings.ptt_key = Some(k.key.clone());
                self.app.save_settings();
            }
            self.app.binding_ptt = false;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.ptt(&k.key, true);
    }

    /// どこにもフォーカスが無いときに文字を打ったら入力欄へ（その文字ごと渡す）
    fn on_key_bubble(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let a = &self.app;
        if !a.in_home() || a.settings_open || a.switcher.is_some() || a.pending_delete.is_some() {
            return;
        }
        if !self.focus.is_focused(window)
            || !a
                .sel_channel
                .as_ref()
                .is_some_and(|id| !a.no_access.contains(id))
        {
            return;
        }
        let k = &ev.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match type_ahead(&k.key, k.key_char.as_deref()) {
            TypeAhead::Insert(ch) => {
                self.app.input.push_str(&ch);
                self.app.focus_input = true;
                cx.notify();
            }
            TypeAhead::Focus => {
                self.app.focus_input = true;
                cx.notify();
            }
            TypeAhead::Ignore => {}
        }
    }

    /// プッシュトゥトーク（ウィンドウにフォーカスがあるときだけ）
    fn ptt(&mut self, key: &str, down: bool) {
        let s = &self.app.settings;
        if !s.ptt || self.app.binding_ptt {
            return;
        }
        if s.ptt_key
            .as_deref()
            .is_some_and(|k| k.eq_ignore_ascii_case(key))
        {
            self.app.voice.ptt_down.store(down, Relaxed);
        }
    }

    /// 動画・画像を保存する（保存先を選ぶダイアログを出す）。いくつでも並べて保存できる。
    /// 同じものを保存している最中（保存先を選んでいる間も）は、続けて押しても何もしない（ダイアログを二重に出さない）
    pub fn start_download(&mut self, url: String, filename: String, kind: crate::download::Kind) {
        if self.downloads.iter().any(|(u, _)| *u == url) {
            return;
        }
        let rx = crate::download::start(url.clone(), filename, kind, self.app.waker.clone());
        self.downloads.push((url, rx));
    }

    /// チャンネル一覧・メンバー一覧の横幅を変える境界（ダブルクリックで既定の幅に戻す）
    pub fn pane_resizer(&self, target: ResizeTarget, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.resize_drag.is_some_and(|(t, _, _)| t == target);
        let (id, group) = match target {
            ResizeTarget::Channels => ("resize_channels", "resize_ch_grp"),
            ResizeTarget::Members => ("resize_members", "resize_mem_grp"),
        };
        let bar = div()
            .w(z(2.0))
            .h_full()
            .bg(if active {
                BLURPLE()
            } else {
                Color32::TRANSPARENT
            })
            .group_hover(group, |s| s.bg(BLURPLE()));
        let base = div()
            .id(id)
            .group(group)
            .absolute()
            .top_0()
            .bottom_0()
            .w(z(6.0))
            .flex()
            .cursor(CursorStyle::ResizeLeftRight)
            .occlude();
        let base = match target {
            ResizeTarget::Channels => base.right_0().justify_end(),
            ResizeTarget::Members => base.left_0().justify_start(),
        };
        // ドラッグ中だけ、ウィンドウ全体のマウスの動きを拾う
        let tracker = active.then(|| {
            let (this, this2) = (self.this.clone(), self.this.clone());
            gpui::canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                        if !phase.bubble() {
                            return;
                        }
                        let Some(e) = this.upgrade() else { return };
                        e.update(cx, |r, cx| {
                            let Some((t, start_x, start_w)) = r.resize_drag else {
                                return;
                            };
                            if t != target {
                                return;
                            }
                            if ev.pressed_button != Some(MouseButton::Left) {
                                r.resize_drag = None;
                                r.app.save_settings();
                                cx.notify();
                                return;
                            }
                            let delta = (f32::from(ev.position.x) - start_x) / zoom();
                            let changed = match target {
                                ResizeTarget::Channels => r.app.drag_pane(true, start_w + delta),
                                ResizeTarget::Members => r.app.drag_pane(false, start_w - delta),
                            };
                            if changed {
                                cx.notify();
                            }
                        });
                    });
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if !phase.bubble() {
                            return;
                        }
                        let Some(e) = this2.upgrade() else { return };
                        e.update(cx, |r, cx| {
                            if r.resize_drag.take().is_some() {
                                r.app.save_settings();
                                cx.notify();
                            }
                        });
                    });
                },
            )
            .absolute()
            .size_0()
        });
        base.child(bar).children(tracker).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if ev.click_count >= 2 {
                    this.resize_drag = None;
                    match target {
                        ResizeTarget::Channels => {
                            this.app.settings.channel_pane_width = st::PANE_DEFAULT
                        }
                        ResizeTarget::Members => {
                            this.app.settings.member_pane_width = st::PANE_DEFAULT
                        }
                    }
                    this.app.save_settings();
                    cx.notify();
                    return;
                }
                let (ch, mem) = this.app.pane_widths();
                let start_w = match target {
                    ResizeTarget::Channels => ch,
                    ResizeTarget::Members => mem,
                };
                this.resize_drag = Some((target, f32::from(ev.position.x), start_w));
                cx.notify();
            }),
        )
    }

    /// テーマで「ウィンドウ全体」に敷く背景画像
    fn window_background(&self) -> Option<AnyElement> {
        let bg = self
            .app
            .theme_applied
            .as_ref()
            .and_then(|t| t.background.as_ref())
            .filter(|b| b.window)?;
        Some(background(bg, opaque(BG())).into_any_element())
    }
}

/// 色の透明度を外す（透明な色でも背景を暗くする色として使えるように）
pub fn opaque(c: Color32) -> Color32 {
    Color32::from_rgb(c.r(), c.g(), c.b())
}

/// 背景画像を親いっぱいに（縦横比を保って切り抜いて）敷き、上から dim の色を重ねて暗くする
pub fn background(bg: &crate::theme::BackgroundImage, dim: Color32) -> Div {
    // 地の色の上に、見え具合ぶんだけ透かした画像を置く（= 画像に地の色を重ねて暗くしたのと同じ）
    let d = div().absolute().inset_0().bg(dim);
    match loaded(&bg.uri) {
        Some(i) => d.child(
            img(i)
                .absolute()
                .inset_0()
                .size_full()
                .object_fit(ObjectFit::Cover)
                .opacity(bg.opacity),
        ),
        None => d,
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.before_render(window, cx);
        let content = if !self.app.in_home() {
            self.login_view(window, cx).into_any_element()
        } else {
            self.main_view(window, cx).into_any_element()
        };
        div()
            .id("root")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(opaque(BG()))
            .text_color(TEXT())
            .font_family(self.family())
            .text_size(z(16.0))
            .line_height(relative(1.375))
            .capture_key_down(cx.listener(Self::on_key_capture))
            // :名前: の補完中は、入力欄より先にキーを受け取る（補完していなければ素通し）
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::MoveUp, window, cx| {
                    // 補完していなければ、空の入力欄の ↑ で直前のメッセージを編集
                    if !this.command_ac_key(emoji::AcKey::Up, window, cx)
                        && !this.emoji_ac_key(emoji::AcKey::Up, window, cx)
                    {
                        this.compose_up(window, cx);
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::MoveDown, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Down, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Down, window, cx);
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::IndentInline, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Confirm, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Confirm, window, cx);
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::Enter, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Confirm, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Confirm, window, cx);
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::Escape, window, cx| {
                    // 変換中の Esc は変換の取り消しだけ（補完・コマンド・返信はそのまま）
                    if this.compose_escape(window, cx) {
                        return;
                    }
                    if !this.command_ac_key(emoji::AcKey::Dismiss, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Dismiss, window, cx);
                    }
                }),
            )
            .on_key_down(cx.listener(Self::on_key_bubble))
            .capture_key_up(
                cx.listener(|this, ev: &KeyUpEvent, _, _| this.ptt(&ev.keystroke.key, false)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _, cx| {
                    if this.app.in_home() && !this.app.settings_open {
                        this.navigate(true);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, _, cx| {
                    if this.app.in_home() && !this.app.settings_open {
                        this.navigate(false);
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                let a = &this.app;
                if a.in_home()
                    && a.sel_channel
                        .as_ref()
                        .is_some_and(|id| !a.no_access.contains(id))
                    && a.editing.is_none()
                {
                    this.app.attach_paths(paths.paths().to_vec());
                    cx.notify();
                }
            }))
            .children(self.window_background())
            .child(content)
    }
}

/// どこにもフォーカスが無いときに打ったキーを、入力欄へどう渡すか
#[derive(Debug, PartialEq)]
enum TypeAhead {
    /// その文字を入れて入力欄へ移る
    Insert(String),
    /// 文字は入れずに入力欄へ移るだけ（日本語入力などを通すべき文字・変換のキー）
    Focus,
    Ignore,
}

/// フォーカスの無いところで打った文字は入力欄の日本語入力（IME）を通っていない。
/// ASCII の文字だけをそのまま渡し、それ以外（かな入力の文字など）や変換・かなのキーは
/// 入力欄へ移るだけにする（続きは入力欄で IME を通して打てる）
fn type_ahead(key: &str, key_char: Option<&str>) -> TypeAhead {
    const IME_KEYS: &[&str] = &[
        "henkan",
        "muhenkan",
        "kana",
        "eisu",
        "hiragana",
        "katakana",
        "hiragana_katakana",
        "zenkaku",
        "hankaku",
        "zenkaku_hankaku",
        "kanji",
        "convert",
        "nonconvert",
        "lang1",
        "lang2",
        "hangul",
        "hanja",
        "hangul_hanja",
        "romaji",
    ];
    let lower = key.to_ascii_lowercase();
    if IME_KEYS.contains(&lower.as_str()) {
        return TypeAhead::Focus;
    }
    match key_char.filter(|c| !c.is_empty() && !c.chars().any(char::is_control)) {
        Some(c) if c.is_ascii() => TypeAhead::Insert(c.to_string()),
        Some(_) => TypeAhead::Focus,
        None => TypeAhead::Ignore,
    }
}

#[cfg(test)]
mod ui_input_tests {
    use super::{merge_pending, type_ahead, TypeAhead};

    #[test]
    fn type_ahead_passes_only_ascii() {
        assert_eq!(type_ahead("a", Some("a")), TypeAhead::Insert("a".into()));
        assert_eq!(type_ahead("A", Some("A")), TypeAhead::Insert("A".into()));
        // かな入力の文字は IME を通っていないので入れない
        assert_eq!(type_ahead("a", Some("ち")), TypeAhead::Focus);
        assert_eq!(type_ahead("henkan", None), TypeAhead::Focus);
        assert_eq!(type_ahead("zenkaku_hankaku", None), TypeAhead::Focus);
        assert_eq!(type_ahead("escape", None), TypeAhead::Ignore);
        assert_eq!(type_ahead("tab", Some("\t")), TypeAhead::Ignore);
    }

    #[test]
    fn pending_logic_edit_is_merged_after_commit() {
        // 変換中にプラグインが後ろに足した分は、確定した文の後ろに付ける
        assert_eq!(merge_pending("abc", "abc!!", "abc日本"), "abc日本!!");
        // 送信で空にした・別の中身にしたなら、ロジック側を優先する
        assert_eq!(merge_pending("abc", "", "abc日本"), "");
        assert_eq!(merge_pending("abc", "xyz", "abc日本"), "xyz");
    }
}

/// Ctrl、macOS では Cmd も。ほかの OS の Super / Win キーは OS のショートカットと重なるので使わない
fn cmd_or_ctrl(m: &gpui::Modifiers) -> bool {
    m.control || (cfg!(target_os = "macos") && m.platform)
}
