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
    /// 本文・チャンネル・プラグインの状態（revision）の目印。変わったら表示加工をやり直す
    pub src: u64,
    /// on_display を通した後の本文
    pub displayed: String,
    /// メンションの名前を入れた後の本文の目印（チャンネル名・ロール名が変わったら解析し直す）
    pub resolved: u64,
    pub blocks: Rc<[crate::markdown::Block]>,
}

/// 本文の控えの上限。
pub const BODY_CACHE_MAX: usize = 2048;

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
    pub member_key: (String, u64),
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
    /// メッセージ ID ごとの本文の控え（BodyCache）
    pub body_cache: HashMap<String, BodyCache>,
    /// 最初に挿入した順。既存項目の更新では順序を変えない。
    pub body_cache_order: std::collections::VecDeque<String>,
    pub menu: Option<(PopMenu, Point<Pixels>)>,
    /// PopMenu::Guild の対象のサーバー
    pub menu_guild: Option<String>,
    /// サーバーのメニューを、チャンネル一覧の上のサーバー名から開いた（見出しの下に出す）
    pub menu_from_header: bool,
    /// サーバー名の見出しの位置（描いたときに覚える。メニューを真下に出すため）
    pub guild_header_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    pub menu_folder: Option<String>,
    /// PopMenu::Message の対象のメッセージ
    pub menu_msg: Option<crate::model::Message>,
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
    /// MessageLogger のログ一覧で選んでいる種類（0 = 削除、1 = 編集、2 = ゴーストピン）
    pub log_tab: usize,
    /// 「別のアカウント」で削除を押したプロファイル（もう一度押すと消す）
    pub confirm_profile_delete: Option<String>,
    pub settings_scroll: ScrollHandle,
    pub font_family: SharedString,
    /// テーマで指定したフォントを読み込んだもの（パス, ファミリー名）
    pub theme_font: Option<(std::path::PathBuf, SharedString)>,
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
    /// 右の区画のプロファイルの lock（左はプロセスが起動時に取っている）
    _lock: Option<std::fs::File>,
    _subs: Vec<Subscription>,
}

impl Root {
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
                let mut v = state.read(cx).value().to_string();
                if let Some(l) = limit {
                    if v.chars().count() > l {
                        v = v.chars().take(l).collect();
                        let v2 = v.clone();
                        state.update(cx, |s, cx| s.set_value(v2, window, cx));
                    }
                }
                if let Some(f) = field(&mut this.app) {
                    *f = v;
                }
                cx.notify();
            }
        },
    )
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
        return;
    }
    e.update(cx, |st, cx| {
        if st.marked_text_range(window, cx).is_some() {
            return;
        }
        st.set_value(s.to_string(), window, cx);
        st.set_selected_range(s.len()..s.len(), cx);
    });
}

/// fc-match で日本語の既定フォントのファミリー名を調べる
fn default_family() -> SharedString {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(o) = std::process::Command::new("fc-match")
            .args(["-f", "%{family[0]}", "sans:lang=ja"])
            .output()
        {
            if let Ok(s) = String::from_utf8(o.stdout) {
                if !s.trim().is_empty() {
                    return s.trim().to_string().into();
                }
            }
        }
        "Noto Sans CJK JP".into()
    }
    #[cfg(windows)]
    {
        "Yu Gothic UI".into()
    }
    #[cfg(target_os = "macos")]
    {
        "Hiragino Sans".into()
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
                .placeholder("自分について書いてみましょう")
        });
        let inp = Inputs {
            compose,
            dm_filter: new_input(window, cx, "会話を検索", false),
            switcher: new_input(window, cx, "会話・チャンネル・サーバーを検索", false),
            email: new_input(window, cx, "", false),
            password: new_input(window, cx, "", true),
            mfa: new_input(window, cx, "123456", false),
            settings_query: new_input(window, cx, "検索", false),
            name: new_input(window, cx, "", false),
            pronouns: new_input(window, cx, "代名詞を追加", false),
            bio,
            bg_image: new_input(
                window,
                cx,
                "画像のパスか https:// の URL（空ならテーマの指定）",
                false,
            ),
            emoji_query: new_input(window, cx, "絵文字を検索", false),
            search: new_input(window, cx, "検索（from: in: has: before: after:）", false),
            thread_name: new_input(window, cx, "スレッド名", false),
            role_name: new_input(window, cx, "ロール名", false),
            guild_name: new_input(window, cx, "サーバー名", false),
            guild_description: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 4)
                    .placeholder("サーバーの説明")
            }),
            channel_member_id: new_input(window, cx, "メンバー ID", false),
            friend_name: new_input(window, cx, "ユーザー名", false),
            folder_name: new_input(window, cx, "フォルダ名", false),
            poll_question: new_input(window, cx, "質問", false),
            poll_answers: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 6)
                    .placeholder("選択肢を1行ずつ入力")
            }),
            channel_name: new_input(window, cx, "名前", false),
            guild_nick: new_input(window, cx, "ニックネーム", false),
            guild_pronouns: new_input(window, cx, "代名詞", false),
            guild_bio: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 6)
                    .placeholder("自己紹介")
            }),
            event_name: new_input(window, cx, "イベント名", false),
            event_start: new_input(window, cx, "開始 YYYY-MM-DD HH:MM", false),
            event_end: new_input(window, cx, "終了 YYYY-MM-DD HH:MM", false),
            event_location: new_input(window, cx, "場所", false),
            event_description: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 5)
                    .placeholder("説明（任意）")
            }),
            rule_name: new_input(window, cx, "ルール名", false),
            rule_keywords: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(2, 6)
                    .placeholder("ブロックする語をカンマか改行で区切って入力")
            }),
            post_title: new_input(window, cx, "タイトル", false),
            post_body: cx.new(|cx| {
                TextareaState::new(window, cx)
                    .auto_grow(3, 10)
                    .placeholder("本文を入力")
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
                    this.app.input = state.read(cx).value().to_string();
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
        // ウィンドウを閉じたときの控えの保存は split::Shell が区画ごとに行う
        let app = AppState::new(waker, profile);
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
            member_key: (String::new(), u64::MAX),
            hover_msg: None,
            revealed: Default::default(),
            body_cache: HashMap::new(),
            body_cache_order: Default::default(),
            menu: None,
            menu_guild: None,
            menu_from_header: false,
            guild_header_bounds: Rc::default(),
            menu_folder: None,
            menu_msg: None,
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
            log_tab: 0,
            confirm_profile_delete: None,
            settings_scroll: ScrollHandle::new(),
            font_family: default_family(),
            theme_font: None,
            last_title: String::new(),
            compose_hint: String::new(),
            image_preview: None,
            video: None,
            stream_image: None,
            downloads: Vec::new(),
            other: None,
            split_vertical,
            pane_h: 0.0,
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
                        self.app.theme_error = Some(format!("フォント {}: {e}", path.display()))
                    }
                }
            }
            Some(_) => {}
            None => self.theme_font = None,
        }
    }

    pub fn family(&self) -> SharedString {
        self.theme_font
            .as_ref()
            .map(|f| f.1.clone())
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
        }
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
                        app.show_toast("ダウンロードしています…")
                    }
                    Ok(crate::download::Event::Finished(path)) => {
                        app.show_toast(&format!("保存しました: {}", path.display()));
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
        self.app.poll_file_pick();
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
        if self.app.font_dirty {
            self.app.font_dirty = false;
            self.apply_fonts(cx);
            self.app.theme_dirty = true;
        }
        if self.app.theme_dirty {
            self.app.theme_dirty = false;
            self.apply_component_theme(window, cx);
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
        let a = &mut self.app;
        a.input = self.inp.compose.read(cx).value().to_string();
        let limit = st::message_text_limit(a.me.as_ref().and_then(|u| u.premium_type));
        let len = st::message_text_len(&a.input);
        if len > limit {
            // 送っても Discord に断られるので、送らずに入力を残して知らせる
            a.compose_notice = Some((format!("本文は{limit}文字までです（いま{len}文字）。{}文字減らすか、分けて送ってください。", len - limit), Instant::now()));
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

    fn after_nav(&mut self) {
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
        // プッシュトゥトークのキーを決めている間は、そのキーを覚える
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
        if !self.app.in_home() {
            return;
        }
        if self.image_preview.is_some() {
            if k.key == "escape" {
                self.image_preview = None;
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        let m = &k.modifiers;
        // Ctrl+F: メッセージ検索
        if m.control && !m.alt && !m.shift && k.key == "f" && self.app.sel_channel.is_some() {
            if self.app.side != crate::app::SidePanel::Search {
                self.app.toggle_search();
            }
            self.inp.search.update(cx, |s, cx| s.focus(window, cx));
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if m.control && !m.alt && !m.shift && k.key == "k" {
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
        if self.app.settings_open {
            if k.key == "escape" {
                self.settings_escape();
                cx.stop_propagation();
                cx.notify();
            }
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
        if m.alt && !m.control {
            match k.key.as_str() {
                "left" => self.navigate(true),
                "right" => self.navigate(false),
                "up" | "down" => {
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
                _ => return,
            }
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
        // Shift+L: 最新のメッセージへ。案内が出ているときだけで、入力中の文字の L は打てるようにする
        if m.shift
            && !m.control
            && !m.alt
            && !m.platform
            && k.key.eq_ignore_ascii_case("l")
            && self.scrolled_up()
            && (!compose_focused || self.app.input.is_empty())
        {
            self.jump_to_latest();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        // 入力欄で: Esc は返信・編集をやめる、↑ は直前のメッセージを編集
        if compose_focused {
            if k.key == "escape" && (self.app.editing.is_some() || self.app.reply_to.is_some()) {
                self.cancel_compose();
                cx.stop_propagation();
                cx.notify();
            } else if k.key == "up"
                && !m.modified()
                && self.app.input.is_empty()
                && self.app.editing.is_none()
            {
                self.edit_last();
                cx.stop_propagation();
                cx.notify();
            }
        }
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
        if k.modifiers.control || k.modifiers.alt {
            return;
        }
        if let Some(ch) = k
            .key_char
            .as_ref()
            .filter(|c| !c.chars().any(char::is_control))
        {
            self.app.input.push_str(ch);
            self.app.focus_input = true;
            cx.notify();
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
                    if !this.command_ac_key(emoji::AcKey::Up, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Up, window, cx)
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::MoveDown, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Down, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Down, window, cx)
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::IndentInline, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Confirm, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Confirm, window, cx)
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::Enter, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Confirm, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Confirm, window, cx)
                    }
                }),
            )
            .capture_action(
                cx.listener(|this, _: &gpui::base::input::Escape, window, cx| {
                    if !this.command_ac_key(emoji::AcKey::Dismiss, window, cx) {
                        this.emoji_ac_key(emoji::AcKey::Dismiss, window, cx)
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
