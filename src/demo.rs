//! 画面確認用のデモ接続（RISCORD_DEMO=1 で起動したとき）。Discord には一切つながず、
//! 手元で作ったサーバー・DM・メッセージ・メンバーを返す。実アカウントに触れずに見た目を確かめるため。
use std::sync::mpsc::{self, Receiver};

use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::model::{Channel, Guild, Member, Message, Presence, Role, User};
use crate::net::{self, Cmd, Event};
use crate::wake::Waker;

pub fn enabled() -> bool {
    std::env::var_os("RISCORD_DEMO").is_some_and(|v| !v.is_empty())
}

fn user(id: &str, name: &str) -> User {
    User { id: id.into(), username: name.into(), global_name: Some(name.into()), avatar: None, premium_type: None }
}

fn channel(id: &str, kind: u8, name: &str, parent: Option<&str>, position: i32) -> Channel {
    serde_json::from_value(serde_json::json!({
        "id": id, "type": kind, "name": name, "parent_id": parent, "position": position,
        "last_message_id": "1300",
    }))
    .unwrap()
}

fn message(id: u64, channel: &str, author: &User, content: &str, minutes_ago: i64) -> Message {
    let at = chrono::Utc::now() - chrono::Duration::minutes(minutes_ago);
    serde_json::from_value(serde_json::json!({
        "id": id.to_string(), "channel_id": channel,
        "author": { "id": author.id, "username": author.username, "global_name": author.global_name },
        "content": content, "timestamp": at.to_rfc3339(),
        "mentions": [{ "id": "2", "username": "sakura", "global_name": "さくら" }],
    }))
    .unwrap()
}

pub fn spawn(waker: Waker) -> (UnboundedSender<Cmd>, Receiver<Event>) {
    let (cmd_tx, mut cmd_rx) = unbounded_channel();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let send = |e: Event| {
            let _ = tx.send(e);
            waker.wake();
        };
        let me = user("1", "どんぐり");
        let others = [user("2", "さくら"), user("3", "たろう"), user("4", "とても長い名前のユーザーさんです")];
        let guild: Guild = serde_json::from_value(serde_json::json!({ "id": "100", "name": "デモサーバー", "permissions": "8" })).unwrap();
        let guild2: Guild = serde_json::from_value(serde_json::json!({ "id": "101", "name": "ゲーム部", "permissions": "8" })).unwrap();
        let roles: Vec<Role> = serde_json::from_value(serde_json::json!([
            { "id": "100", "name": "@everyone", "color": 0, "position": 0, "permissions": "8" },
            { "id": "500", "name": "管理者", "color": 0xe67e22, "position": 2, "hoist": true, "permissions": "8" },
        ]))
        .unwrap();
        let dm: Channel = serde_json::from_value(serde_json::json!({
            "id": "900", "type": 1, "last_message_id": "1300",
            "recipients": [{ "id": "2", "username": "sakura", "global_name": "さくら" }],
        }))
        .unwrap();
        send(Event::Me(me.clone()));
        send(Event::Guilds(vec![guild, guild2]));
        send(Event::Dms(vec![dm]));
        send(Event::Presences(vec![
            ("1".into(), Presence { status: "online".into(), activity: None }),
            ("2".into(), Presence { status: "idle".into(), activity: Some("プレイ中: パズル".into()) }),
            ("3".into(), Presence { status: "dnd".into(), activity: None }),
        ]));
        // サーバーアイコンのメンション数の印を見るための未読メンション
        send(Event::ReadyExtra(serde_json::json!({
            "channel_guild": { "10011": "100", "10111": "101" },
            "read_state": { "entries": [
                { "id": "10011", "mention_count": 3 },
                { "id": "10111", "mention_count": 12 },
                { "id": "900", "mention_count": 1 },
            ] },
        })));
        send(Event::GatewayReady);
        send(Event::Status("接続済み".into()));
        let mut next = 2000u64;
        while let Some(c) = cmd_rx.blocking_recv() {
            match c {
                Cmd::LoadGuildChannels(gid) => {
                    let p = |s: &str| format!("{gid}{s}");
                    let chans = vec![
                        channel(&p("10"), 4, "テキストチャンネル", None, 0),
                        channel(&p("11"), 0, "一般", Some(&p("10")), 0),
                        channel(&p("12"), 0, "雑談", Some(&p("10")), 1),
                        channel(&p("20"), 4, "ボイスチャンネル", None, 1),
                        channel(&p("21"), 2, "ロビー", Some(&p("20")), 0),
                    ];
                    send(Event::GuildRoles(gid.clone(), roles.clone()));
                    send(Event::GuildChannels(gid, chans));
                }
                Cmd::LoadMessages(cid) => {
                    let mut list = vec![
                        message(1200, &cid, &others[0], "おはようございます！ **今日** もよろしくお願いします", 60 * 26),
                        message(1201, &cid, &others[1], "<@2> 昨日のリンク https://example.com/page です", 60 * 25),
                        message(1202, &cid, &me, "ありがとう！", 30),
                        message(1203, &cid, &me, "あとで見ます", 29),
                        message(1204, &cid, &others[2], "名前が長い人のメッセージ。折り返しの確認用に少し長めの文章を書いておきます。ここまで書けば十分に長いはずです。", 5),
                    ];
                    for (i, m) in list.iter_mut().enumerate() {
                        m.id = format!("{}", 1200 + i);
                    }
                    send(Event::Messages(cid, list));
                }
                Cmd::LoadOlder { channel_id, before } => send(Event::OlderMessages { channel_id, before, messages: Some(vec![]) }),
                Cmd::SubscribeMembers { guild_id, .. } => {
                    let mut all = vec![me.clone()];
                    all.extend(others.iter().cloned());
                    let members: Vec<Member> = all
                        .iter()
                        .map(|u| Member { user: Some(u.clone()), nick: None, roles: if u.id == "2" { vec!["500".into()] } else { vec![] } })
                        .collect();
                    send(Event::GuildMembers(guild_id.clone(), members));
                    send(Event::MemberList {
                        guild_id,
                        list_id: "demo".into(),
                        groups: vec![("500".into(), 1), ("online".into(), 2), ("offline".into(), 1)],
                        ops: vec![net::ListOp::Sync {
                            range: [0, 99],
                            items: vec![
                                net::ListItem::Group("500".into(), 0),
                                net::ListItem::Member("2".into()),
                                net::ListItem::Group("online".into(), 0),
                                net::ListItem::Member("1".into()),
                                net::ListItem::Member("3".into()),
                                net::ListItem::Group("offline".into(), 0),
                                net::ListItem::Member("4".into()),
                            ],
                        }],
                    });
                }
                Cmd::Send { channel_id, content, .. } => {
                    next += 1;
                    let m = message(next, &channel_id, &me, &content, 0);
                    send(Event::ComposeResult { channel_id, result: Ok(Some(m)) });
                }
                Cmd::Delete { channel_id, id } => {
                    send(Event::MessageDelete { channel_id, id });
                }
                _ => {}
            }
        }
    });
    (cmd_tx, rx)
}
