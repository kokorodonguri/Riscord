//! UI に依存しない既存動作の回帰テスト。
use super::*;

fn bench_app(n_msgs: usize) -> App {
    let mut app = App::default();
    let msgs = (0..n_msgs)
        .map(|i| {
            serde_json::from_value(serde_json::json!({
                "id": format!("{}", 100000 + i), "channel_id": "c",
                "author": { "id": "1", "username": "u" },
                "content": format!("message {i}"), "timestamp": "2026-11-01T10:00:00+00:00"
            }))
            .unwrap()
        })
        .collect();
    app.messages.insert("c".into(), msgs);
    app.touch_channel("c");
    app.sel_channel = Some("c".into());
    app
}

#[test]
fn repeated_delete_preserves_logged_deleted_message() {
    let mut app = bench_app(1);
    app.messages.get_mut("c").unwrap()[0].log = Some(Box::new(crate::model::LogMark {
        deleted_at: Some("2026-11-01T10:01:00+00:00".into()),
        edits: vec![],
    }));
    app.on_message_delete("c", "100000");
    assert_eq!(app.messages["c"].len(), 1);
    assert!(app.messages["c"][0]
        .log
        .as_ref()
        .unwrap()
        .deleted_at
        .is_some());
}

#[test]
fn member_chunks_preserve_order_and_latest_updates() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let member = |id: &str, name: &str| Member {
        user: Some(User {
            id: id.into(),
            username: name.into(),
            global_name: None,
            avatar: None,
            premium_type: None,
            bot: false,
            avatar_decoration_data: None,
        }),
        nick: None,
        avatar: None,
        avatar_decoration_data: None,
        roles: Vec::new(),
    };
    tx.send(Event::GuildMembers(
        "a".into(),
        vec![member("1", "old"), member("2", "two")],
    ))
    .unwrap();
    tx.send(Event::GuildMembers("b".into(), vec![member("1", "other")]))
        .unwrap();
    tx.send(Event::GuildMembers(
        "a".into(),
        vec![
            member("1", "new"),
            member("3", "three"),
            Member {
                user: None,
                nick: None,
                avatar: None,
                avatar_decoration_data: None,
                roles: vec![],
            },
        ],
    ))
    .unwrap();
    app.poll();
    let a: Vec<_> = app.members["a"]
        .iter()
        .map(|m| (m.user.as_ref().unwrap().id.as_str(), m.display()))
        .collect();
    assert_eq!(a, [("1", "new"), ("2", "two"), ("3", "three")]);
    assert_eq!(app.members["b"][0].display(), "other");
}

#[test]
fn member_index_reuses_then_expires_and_handles_interleaved_guilds() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let member = |id: &str, name: &str| Member {
        user: Some(User {
            id: id.into(),
            username: name.into(),
            global_name: None,
            avatar: None,
            premium_type: None,
            bot: false,
            avatar_decoration_data: None,
        }),
        nick: None,
        avatar: None,
        avatar_decoration_data: None,
        roles: vec![],
    };
    tx.send(Event::GuildMembers("a".into(), vec![member("1", "old")]))
        .unwrap();
    app.poll();
    assert_eq!(app.member_index.as_ref().unwrap().guild_id, "a");
    tx.send(Event::GuildMembers(
        "a".into(),
        vec![member("1", "new"), member("2", "two")],
    ))
    .unwrap();
    app.poll();
    assert_eq!(
        app.members["a"]
            .iter()
            .map(Member::display)
            .collect::<Vec<_>>(),
        ["new", "two"]
    );
    tx.send(Event::GuildMembers("b".into(), vec![member("1", "other")]))
        .unwrap();
    app.poll();
    assert_eq!(app.member_index.as_ref().unwrap().guild_id, "b");
    tx.send(Event::GuildMembers(
        "a".into(),
        vec![member("2", "updated")],
    ))
    .unwrap();
    app.poll();
    assert_eq!(
        app.members["a"]
            .iter()
            .map(Member::display)
            .collect::<Vec<_>>(),
        ["new", "updated"]
    );
    app.member_index.as_mut().unwrap().updated -= MEMBER_INDEX_TTL;
    app.poll();
    assert!(app.member_index.is_none());
    assert_eq!(app.members["a"].len(), 2);
}

#[test]
fn cancelled_file_picker_does_not_block_another_channel() {
    let mut app = App::default();
    let (tx, rx) = std::sync::mpsc::channel();
    app.file_pick = Some(("original".into(), rx));
    app.sel_channel = Some("other".into());
    tx.send(Vec::new()).unwrap();
    app.poll_file_pick();
    assert!(app.file_pick.is_none());
    assert!(app.picked_files.is_none());
}

#[test]
fn successful_refresh_clears_only_its_own_error_status() {
    let mut app = bench_app(1);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    tx.send(Event::MessagesFailed("c".into())).unwrap();
    app.poll();
    assert_eq!(app.status, MESSAGE_RELOAD_ERROR);
    tx.send(Event::Messages("c".into(), Vec::new())).unwrap();
    app.poll();
    assert!(app.status.is_empty());
    tx.send(Event::MessagesFailed("c".into())).unwrap();
    tx.send(Event::Status("別の状態".into())).unwrap();
    tx.send(Event::Messages("c".into(), Vec::new())).unwrap();
    app.poll();
    assert_eq!(app.status, "別の状態");
}

#[test]
fn loading_messages_replays_gateway_changes_and_prevents_duplicate_requests() {
    let mut app = bench_app(3);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let snapshot = app.messages["c"].clone();
    app.load_messages("c".into());
    app.load_messages("c".into());
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadMessages(id)) if id == "c"));
    assert!(rx.try_recv().is_err());
    let mut new = snapshot[0].clone();
    new.id = "100003".into();
    tx.send(Event::MessageCreate(new)).unwrap();
    tx.send(Event::MessageUpdate {
        channel_id: "c".into(),
        id: snapshot[0].id.clone(),
        content: Some("edited".into()),
        embeds: None,
        pinned: None,
    })
    .unwrap();
    tx.send(Event::MessageDelete {
        channel_id: "c".into(),
        id: snapshot[1].id.clone(),
    })
    .unwrap();
    app.poll();
    tx.send(Event::Messages("c".into(), snapshot)).unwrap();
    app.poll();
    let list = &app.messages["c"];
    assert_eq!(
        list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["100000", "100002", "100003"]
    );
    assert_eq!(list[0].content, "edited");
    assert!(!app.pending_message_loads.contains_key("c"));
    app.load_messages("c".into());
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadMessages(_))));
    tx.send(Event::MessagesFailed("c".into())).unwrap();
    app.poll();
    assert!(!app.pending_message_loads.contains_key("c"));
    app.load_messages("c".into());
    assert!(matches!(rx.try_recv(), Ok(Cmd::LoadMessages(_))));
}

#[test]
fn initial_message_load_keeps_arrivals_and_own_send_response() {
    let mut app = bench_app(1);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let snapshot = app.messages.remove("c").unwrap();
    app.load_messages("c".into());
    let _ = rx.try_recv();
    let mut incoming = snapshot[0].clone();
    incoming.id = "100001".into();
    tx.send(Event::MessageCreate(incoming)).unwrap();
    app.submit_compose("c".into(), "sent".into());
    let mut sent = snapshot[0].clone();
    sent.id = "100002".into();
    sent.content = "sent".into();
    tx.send(Event::ComposeResult {
        channel_id: "c".into(),
        result: Ok(Some(sent)),
    })
    .unwrap();
    app.poll();
    tx.send(Event::Messages("c".into(), snapshot)).unwrap();
    app.poll();
    assert_eq!(
        app.messages["c"]
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["100000", "100001", "100002"]
    );
}

#[test]
fn compose_failure_retains_text_reply_and_attachment_until_restored() {
    let mut app = bench_app(1);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.reply_to = Some(app.messages["c"][0].clone());
    app.uploads.push(clip::Upload {
        name: "test.txt".into(),
        data: vec![1, 2],
        thumb: None,
    });
    app.submit_compose("c".into(), "hello".into());
    assert!(
        matches!(rx.try_recv(), Ok(Cmd::Send { content, files, reply_to, .. }) if content == "hello" && files[0].1 == [1, 2] && reply_to.is_some())
    );
    app.submit_compose("c".into(), "duplicate".into());
    assert!(rx.try_recv().is_err());
    tx.send(Event::ComposeResult {
        channel_id: "c".into(),
        result: Err(()),
    })
    .unwrap();
    app.poll();
    app.input = "new draft".into();
    assert!(!app.restore_compose("c"));
    assert_eq!(app.input, "new draft");
    app.input.clear();
    assert!(app.restore_compose("c"));
    assert_eq!(app.input, "hello");
    assert_eq!(app.uploads[0].data, [1, 2]);
    assert!(app.reply_to.is_some());
    assert!(!app.compose_submissions.contains_key("c"));
}

#[test]
fn compose_success_merges_gateway_and_response_in_order() {
    let mut app = bench_app(2);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.submit_compose("c".into(), "sent".into());
    let mut sent = app.messages["c"][0].clone();
    sent.id = "100002".into();
    sent.content = "sent".into();
    let mut later = sent.clone();
    later.id = "100003".into();
    tx.send(Event::MessageCreate(later)).unwrap();
    tx.send(Event::ComposeResult {
        channel_id: "c".into(),
        result: Ok(Some(sent.clone())),
    })
    .unwrap();
    tx.send(Event::MessageCreate(sent)).unwrap();
    app.poll();
    assert_eq!(
        app.messages["c"]
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["100000", "100001", "100002", "100003"]
    );
    assert!(!app.compose_submissions.contains_key("c"));
}

#[test]
fn failed_edit_restores_original_target_and_success_keeps_newer_gateway_update() {
    let mut app = bench_app(1);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let original = app.messages["c"][0].clone();
    app.editing = Some(original.clone());
    app.submit_compose("c".into(), "changed".into());
    tx.send(Event::ComposeResult {
        channel_id: "c".into(),
        result: Err(()),
    })
    .unwrap();
    app.poll();
    assert!(app.restore_compose("c"));
    assert_eq!(app.editing.as_ref().unwrap().id, original.id);
    assert_eq!(app.input, "changed");
    app.input.clear();
    app.submit_compose("c".into(), "changed".into());
    let mut response = original.clone();
    response.content = "changed".into();
    tx.send(Event::MessageUpdate {
        channel_id: "c".into(),
        id: original.id,
        content: Some("newer".into()),
        embeds: None,
        pinned: None,
    })
    .unwrap();
    tx.send(Event::ComposeResult {
        channel_id: "c".into(),
        result: Ok(Some(response)),
    })
    .unwrap();
    app.poll();
    assert_eq!(app.messages["c"][0].content, "newer");
}

#[test]
fn old_channels_are_evicted() {
    let mut app = App::default();
    for i in 0..(MAX_CHANNELS + 5) {
        let id = format!("c{i}");
        app.messages.insert(id.clone(), Vec::new());
        app.touch_channel(&id);
    }
    assert_eq!(app.messages.len(), MAX_CHANNELS);
    assert!(!app.messages.contains_key("c0"));
    // 開き直したチャンネルは最新扱いになり残る
    app.touch_channel("c5");
    app.messages.insert("new".into(), Vec::new());
    app.touch_channel("new");
    assert!(app.messages.contains_key("c5"));
    assert!(!app.messages.contains_key("c6"));
}

#[test]
fn voice_index_matches_states_across_mixed_updates() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    for batch in 0..100 {
        let mut updates = Vec::new();
        for i in 0..12 {
            let n = (batch * 7 + i * 3) % 29;
            updates.push(VoiceState {
                user_id: format!("u{n}"),
                guild_id: Some("g".into()),
                channel_id: ((batch + i) % 5 != 0).then(|| format!("c{}", (batch + n) % 4)),
                self_mute: (batch + i) % 2 == 0,
                self_deaf: i % 3 == 0,
                self_stream: false,
                user: None,
            });
        }
        tx.send(Event::VoiceStates(updates)).unwrap();
        app.poll();
        let mut expected: HashMap<String, Vec<String>> = HashMap::new();
        for (id, state) in &app.voice_states {
            if let Some(channel) = &state.channel_id {
                expected
                    .entry(channel.clone())
                    .or_default()
                    .push(id.clone());
            }
        }
        for ids in expected.values_mut() {
            ids.sort();
        }
        assert_eq!(app.voice_index, expected, "batch {batch}");
    }
}

#[test]
fn unread_guilds_matches_per_guild_scan() {
    let mut app = App::default();
    app.unread.insert("a".into(), Some("g1".into()));
    app.unread.insert("b".into(), None);
    app.unread.insert("c".into(), Some("g1".into()));
    let set = app.unread_guilds();
    assert!(set.contains(&Some("g1")) && set.contains(&None) && !set.contains(&Some("g2")));
    app.unread.remove("b");
    assert!(!app.unread_guilds().contains(&None));
}

#[test]
fn emoji_split() {
    let v = split_emoji("hi <:pog:123> and <a:dance:456>!<bad> <@789>");
    let k: Vec<String> = v
        .iter()
        .map(|x| match x {
            Seg::Text(t) => format!("T({t})"),
            Seg::Emoji { name, id, animated } => format!("E({name},{id},{animated})"),
        })
        .collect();
    assert_eq!(
        k,
        [
            "T(hi )",
            "E(pog,123,false)",
            "T( and )",
            "E(dance,456,true)",
            "T(!<bad> <@789>)"
        ]
    );
}

#[test]
fn late_message_response_does_not_restore_evicted_channel() {
    let mut app = bench_app(1);
    let old_messages = app.messages.remove("c").unwrap();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.touch_channel("c");
    for i in 0..MAX_CHANNELS {
        let id = format!("c{i}");
        app.messages.insert(id.clone(), Vec::new());
        app.touch_channel(&id);
    }
    tx.send(Event::Messages("c".into(), old_messages.clone()))
        .unwrap();
    app.poll();
    assert!(!app.messages.contains_key("c"));
    assert_eq!(app.messages.len(), MAX_CHANNELS);
    app.touch_channel("c");
    tx.send(Event::Messages("c".into(), old_messages)).unwrap();
    app.poll();
    assert_eq!(app.messages["c"].len(), 1);
    assert_eq!(app.messages.len(), MAX_CHANNELS);
}

#[test]
fn replaced_and_trimmed_messages_release_stickers() {
    for replace in [true, false] {
        let mut app = bench_app(MAX_MSGS);
        let messages = app.messages.get_mut("c").unwrap();
        for (m, id) in messages.iter_mut().take(2).zip(["old", "shared"]) {
            m.sticker_items.push(model::StickerItem {
                id: id.into(),
                name: id.into(),
                format_type: 1,
            });
        }
        app.stickers.insert("old".into(), StickerState::Loading);
        app.stickers.insert("shared".into(), StickerState::Loading);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        if replace {
            tx.send(Event::Messages("c".into(), app.messages["c"][1..].to_vec()))
                .unwrap();
        } else {
            // 表示中のチャンネルは遡った履歴を消さないので、別のチャンネルを見ている状態にする
            app.sel_channel = None;
            let mut new = app.messages["c"][2].clone();
            new.id = "new".into();
            tx.send(Event::MessageCreate(new)).unwrap();
        }
        app.poll();
        assert!(!app.stickers.contains_key("old"), "replace={replace}");
        assert!(app.stickers.contains_key("shared"), "replace={replace}");
    }
}

#[test]
fn file_picker_result_stays_with_its_original_channel() {
    let mut app = App::default();
    let path = std::env::temp_dir().join(format!("riscord-file-pick-{}", std::process::id()));
    std::fs::write(&path, b"selected file").unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    app.file_pick = Some(("original".into(), rx));
    app.sel_channel = Some("other".into());
    tx.send(vec![path.clone()]).unwrap();
    app.poll_file_pick();
    app.take_picked_files();
    assert!(app.uploads.is_empty());
    assert_eq!(app.picked_files.as_ref().unwrap().0, "original");
    assert!(app.status.contains("元のチャンネル"));
    app.sel_channel = Some("original".into());
    app.take_picked_files();
    assert_eq!(app.uploads.len(), 1);
    assert_eq!(app.uploads[0].data, b"selected file");
    assert!(app.picked_files.is_none());
    assert!(app.status.is_empty());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn upload_limits_keep_valid_files_and_explain_rejections() {
    let mut app = bench_app(0);
    let file = |name: &str, size: usize| clip::Upload {
        name: name.into(),
        data: vec![0; size],
        thumb: None,
    };
    app.add_uploads(
        vec![
            file("small.png", MIB as usize),
            file("large.mp4", (11 * MIB) as usize),
        ],
        0,
        Vec::new(),
    );
    assert_eq!(
        app.uploads
            .iter()
            .map(|u| u.name.as_str())
            .collect::<Vec<_>>(),
        ["small.png"]
    );
    assert!(app.compose_notice.take().unwrap().0.contains("large.mp4"));

    app.uploads.clear();
    app.sel_guild = Some("guild".into());
    app.add_uploads(vec![file("large.mp4", (11 * MIB) as usize)], 0, Vec::new());
    assert_eq!(app.uploads.len(), 1);
    assert!(app
        .compose_notice
        .take()
        .unwrap()
        .0
        .contains("サーバーのブースト"));

    app.uploads.clear();
    app.add_uploads(
        (0..MAX_UPLOADS + 2)
            .map(|i| file(&format!("file{i}"), 1))
            .collect(),
        0,
        Vec::new(),
    );
    assert_eq!(app.uploads.len(), MAX_UPLOADS);
    assert!(app.compose_notice.unwrap().0.contains("ほか1件"));
}

#[test]
fn push_to_talk_closes_when_window_loses_focus() {
    let mut app = App::default();
    app.settings.ptt = true;
    app.apply_voice_settings();
    app.voice.ptt_down.store(true, Relaxed);
    assert!(app.voice.gate());
    app.sync_read_state(false);
    assert!(!app.voice.gate());
    app.sync_read_state(true);
    assert!(
        !app.voice.gate(),
        "refocusing must not restore an old key press"
    );
}

#[test]
fn switching_voice_modes_clears_stale_push_to_talk() {
    let mut app = App::default();
    app.settings.ptt = true;
    app.apply_voice_settings();
    app.voice.ptt_down.store(true, Relaxed);
    app.settings.ptt = false;
    app.apply_voice_settings();
    app.settings.ptt = true;
    app.apply_voice_settings();
    assert!(!app.voice.gate());
}

#[test]
fn replacing_avatar_releases_previous_preview() {
    let thumbnail = images::render_image(image::RgbaImage::new(1, 1));
    let mut draft = ProfileDraft {
        fields: Default::default(),
        base: Default::default(),
        avatar: Some(AvatarEdit::Set {
            uri: "data:image/png;base64,".into(),
            tex: thumbnail.clone(),
        }),
    };
    draft.avatar = Some(AvatarEdit::Remove);
    assert!(images::release_queued(&thumbnail));
}

#[test]
fn reset_releases_sticker_images() {
    let image = images::render_image(image::RgbaImage::new(1, 1));
    let mut app = App::default();
    app.stickers
        .insert("sticker".into(), StickerState::Ready(image.clone()));
    app.reset();
    assert!(images::release_queued(&image));
}

#[test]
fn pruning_releases_only_unused_sticker_images() {
    let old = images::render_image(image::RgbaImage::new(1, 1));
    let shared = images::render_image(image::RgbaImage::new(1, 1));
    let mut app = bench_app(1);
    app.messages.get_mut("c").unwrap()[0]
        .sticker_items
        .push(model::StickerItem {
            id: "shared".into(),
            name: "shared".into(),
            format_type: 1,
        });
    app.stickers
        .insert("old".into(), StickerState::Ready(old.clone()));
    app.stickers
        .insert("shared".into(), StickerState::Ready(shared.clone()));
    app.prune_stickers();
    assert!(images::release_queued(&old));
    assert!(!images::release_queued(&shared));
}

#[test]
fn reset_forgets_cached_channel_images() {
    let path = std::env::temp_dir().join(format!("riscord-reset-image-{}.png", std::process::id()));
    image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
        .save(&path)
        .unwrap();
    let uri = format!("file://{}", path.display());
    assert!(matches!(images::get(&uri), images::Img::Pending));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let image = loop {
        match images::peek(&uri) {
            images::Img::Ready(image) => break image,
            images::Img::Failed => panic!("test image could not be decoded"),
            images::Img::Pending if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            images::Img::Pending => panic!("test image timed out"),
        }
    };
    let mut app = App::default();
    app.channel_images
        .entry("a".into())
        .or_default()
        .insert(uri.clone());
    app.channel_images
        .entry("b".into())
        .or_default()
        .insert(uri.clone());
    app.reset();
    assert!(images::release_queued(&image));
    assert!(matches!(images::get(&uri), images::Img::Pending));
    images::forget(&uri);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn read_ack_is_retried_if_command_channel_is_closed() {
    let mut app = bench_app(1);
    app.unread.insert("c".into(), None);
    let (cmd, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let (_events, ev) = std::sync::mpsc::channel();
    app.backend = Some(Backend { cmd, ev });
    app.sync_read_state(true);
    assert!(!app.acked.contains_key("c"));
    assert!(app.unread.contains_key("c"));
    assert!(app.last_ack.is_none());
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (_events, ev) = std::sync::mpsc::channel();
    app.backend = Some(Backend { cmd, ev });
    app.sync_read_state(true);
    assert!(
        matches!(rx.try_recv(), Ok(Cmd::Ack { channel_id, message_id }) if channel_id == "c" && message_id == "100000")
    );
    assert_eq!(app.acked.get("c").map(String::as_str), Some("100000"));
    assert!(!app.unread.contains_key("c"));
    app.sync_read_state(true);
    assert!(rx.try_recv().is_err());
}

#[test]
fn viewing_channel_acks_latest_message() {
    let mut app = bench_app(3);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let acks = |rx: &mut tokio::sync::mpsc::UnboundedReceiver<Cmd>| {
        let mut v = Vec::new();
        while let Ok(c) = rx.try_recv() {
            if let Cmd::Ack {
                channel_id,
                message_id,
            } = c
            {
                v.push((channel_id, message_id));
            }
        }
        v
    };
    app.sync_read_state(true);
    assert_eq!(acks(&mut rx), vec![("c".to_string(), "100002".to_string())]);
    app.sync_read_state(true);
    assert!(acks(&mut rx).is_empty(), "同じものは送り直さない");

    let mut m = app.messages["c"][0].clone();
    m.id = "100003".into();
    tx.send(Event::MessageCreate(m.clone())).unwrap();
    app.poll();
    app.sync_read_state(true);
    assert!(acks(&mut rx).is_empty(), "間隔を空ける");
    app.last_ack = Some(std::time::Instant::now() - ACK_INTERVAL);
    app.sync_read_state(false);
    assert!(
        acks(&mut rx).is_empty(),
        "見ていない（フォーカスがない）ときは送らない"
    );
    app.sync_read_state(true);
    assert_eq!(acks(&mut rx), vec![("c".to_string(), "100003".to_string())]);
}

#[test]
fn failed_read_ack_can_retry_without_overwriting_newer_ack() {
    let mut app = bench_app(1);
    let (events, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.sync_read_state(true);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Ack { .. })));
    events
        .send(Event::AckFailed {
            channel_id: "c".into(),
            message_id: "old".into(),
        })
        .unwrap();
    app.poll();
    assert_eq!(app.acked.get("c").map(String::as_str), Some("100000"));
    events
        .send(Event::AckFailed {
            channel_id: "c".into(),
            message_id: "100000".into(),
        })
        .unwrap();
    app.poll();
    assert!(!app.acked.contains_key("c"));
    app.last_ack = Some(std::time::Instant::now() - ACK_INTERVAL);
    app.sync_read_state(true);
    assert!(rx.try_recv().is_err(), "API 失敗後は連続して再試行しない");
    app.ack_retry_at.insert(
        "c".into(),
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    );
    app.sync_read_state(true);
    assert!(
        matches!(rx.try_recv(), Ok(Cmd::Ack { channel_id, message_id }) if channel_id == "c" && message_id == "100000")
    );
}

#[test]
fn mentions_are_shown_by_name() {
    let m: Message = serde_json::from_value(serde_json::json!({
        "id": "1", "channel_id": "c", "author": { "id": "9", "username": "a" }, "timestamp": "2026-11-01T00:00:00+00:00",
        "content": "", "mentions": [{ "id": "5", "username": "taro", "global_name": "太郎" }],
    })).unwrap();
    let mut channels = HashMap::new();
    let ch: Channel =
        serde_json::from_value(serde_json::json!({ "id": "77", "type": 0, "name": "雑談" }))
            .unwrap();
    channels.insert("g".to_string(), vec![ch]);
    let roles: Vec<Role> = serde_json::from_value(
        serde_json::json!([{ "id": "3", "name": "管理者", "color": 0, "position": 1 }]),
    )
    .unwrap();
    let got = resolve_mentions(
        "<#77> へ <@5> <@!5> <@&3> <#88> <:e:1> <a> 1 < 2",
        &m,
        &channels,
        &roles,
    );
    assert_eq!(
        got,
        "#雑談 へ @太郎 @太郎 @管理者 #不明なチャンネル <:e:1> <a> 1 < 2"
    );
    assert!(matches!(
        resolve_mentions("普通の文", &m, &channels, &roles),
        std::borrow::Cow::Borrowed(_)
    ));
}

/// 見出しの行に人数が無くても、一緒に届いた groups の人数で「ロール名 — 人数」を出す
#[test]
fn server_member_list_headers_use_group_counts() {
    let mut list = ServerMemberList {
        channel_id: "c".into(),
        ..Default::default()
    };
    let items = vec![
        net::ListItem::Group("online".into(), 0),
        net::ListItem::Member("m0".into()),
        net::ListItem::Member("m1".into()),
    ];
    assert!(list.apply(
        "l",
        vec![("online".into(), 2)],
        vec![net::ListOp::Sync {
            range: [0, 99],
            items
        }]
    ));
    let members: Vec<Member> = (0..2)
        .map(|i| Member {
            user: Some(User {
                id: format!("m{i}"),
                username: format!("u{i}"),
                global_name: None,
                avatar: None,
                premium_type: None,
                bot: false,
                avatar_decoration_data: None,
            }),
            nick: None,
            avatar: None,
            avatar_decoration_data: None,
            roles: vec![],
        })
        .collect();
    let cache = MemberListCache::from_server("g", 1, &list, &members, &[]);
    assert!(matches!(&cache.rows[0], MemberRow::Header(h) if h == "オンライン — 2"));
}

/// 操作のお知らせを出しても接続の状態は覚えておき、閉じたらそこへ戻せる。DM 一覧は届いたかどうかを区別する
#[test]
fn status_notice_keeps_gateway_state_and_dms_loaded_is_tracked() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    assert!(!app.dms_loaded);
    tx.send(Event::Status("再接続中…".into())).unwrap();
    tx.send(Event::Dms(vec![])).unwrap();
    app.poll();
    assert_eq!(app.gateway_status, "再接続中…");
    assert!(app.dms_loaded && app.dms.is_empty());
    app.set_compose_notice("x".into());
    assert!(app.compose_notice.is_some());
    app.show_toast("コピーしました");
    assert_eq!(
        app.toast.as_ref().map(|t| t.0.as_str()),
        Some("コピーしました")
    );
}

/// 前回の控え：50件まで残し、次の起動ではログイン前から出す。開いたら読み直し、読み直すまで既読は送らない
#[test]
fn snapshot_restores_recent_conversation_and_refreshes_it() {
    let mut app = bench_app(80);
    app.token_owner = Some("1".into());
    app.me = Some(User {
        id: "1".into(),
        username: "me".into(),
        global_name: None,
        avatar: None,
        premium_type: None,
        bot: false,
        avatar_decoration_data: None,
    });
    let s = app.build_snapshot().expect("snapshot");
    assert_eq!(s.messages.len(), 1);
    assert_eq!(s.messages[0].1.len(), cache::MESSAGES_PER_CHANNEL);
    assert_eq!(
        s.messages[0].1.last().unwrap().id,
        "100079",
        "keeps the newest messages"
    );
    let bytes = serde_json::to_vec(&s).unwrap();
    let s: cache::Snapshot = serde_json::from_slice(&bytes).unwrap();

    let mut next = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    next.backend = Some(Backend { cmd, ev });
    next.restore_snapshot(s);
    assert!(
        next.me.is_some(),
        "main view is shown before login finishes"
    );
    assert_eq!(next.sel_channel.as_deref(), Some("c"));
    assert_eq!(next.messages["c"].len(), cache::MESSAGES_PER_CHANNEL);
    assert!(
        matches!(rx.try_recv(), Ok(Cmd::LoadMessages(id)) if id == "c"),
        "cached channel is reloaded"
    );
    next.sync_read_state(true);
    assert!(
        rx.try_recv().is_err(),
        "no ack for a cached (maybe old) list"
    );

    let fresh = (0..3).map(|i| serde_json::from_value(serde_json::json!({
        "id": format!("{}", 200000 + i), "channel_id": "c",
        "author": { "id": "2", "username": "o" }, "content": "new", "timestamp": "2026-11-02T10:00:00+00:00"
    })).unwrap()).collect();
    tx.send(Event::Messages("c".into(), fresh)).unwrap();
    next.poll();
    assert!(next.stale_channels.is_empty());
    assert_eq!(next.messages["c"].len(), 3);
    next.sync_read_state(true);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Ack { message_id, .. }) if message_id == "200002"));
}

/// 控えを出したまま接続に失敗しても画面は残し、送信待ちは失敗扱いにしてつなぎ直しを予約する
#[test]
fn login_failure_with_snapshot_keeps_view_and_retries() {
    let mut app = bench_app(3);
    app.me = Some(User {
        id: "1".into(),
        username: "me".into(),
        global_name: None,
        avatar: None,
        premium_type: None,
        bot: false,
        avatar_decoration_data: None,
    });
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.submit_compose("c".into(), "hello".into());
    tx.send(Event::LoginFailed).unwrap();
    app.poll();
    assert!(app.me.is_some());
    assert!(app.login_error.is_none());
    assert!(app.login_retry_at.is_some());
    assert!(
        app.compose_submissions["c"].failed,
        "queued send is returned to the input"
    );
    assert!(app.restore_compose("c"));
    assert_eq!(app.input, "hello");
}

/// 控えは持ち主のアカウントに結び付く：同じ人なら入り直して変わったトークンでも使い、別の人・読めないトークンでは使わない
#[test]
fn snapshot_is_tied_to_account() {
    use base64::Engine as _;
    let tok = |id: &str, rest: &str| {
        format!(
            "{}.{rest}",
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(id)
        )
    };
    assert_eq!(
        cache::token_owner(&tok("123456789012345678", "a.b")).as_deref(),
        Some("123456789012345678")
    );
    assert_eq!(cache::token_owner("not a token"), None);
    let mut app = bench_app(2);
    app.me = Some(User {
        id: "123456789012345678".into(),
        username: "me".into(),
        global_name: None,
        avatar: None,
        premium_type: None,
        bot: false,
        avatar_decoration_data: None,
    });
    app.token_owner = cache::token_owner(&tok("123456789012345678", "old.sig"));
    let bytes = serde_json::to_vec(&app.build_snapshot().unwrap()).unwrap();
    cache::write(None, &bytes).unwrap();
    assert!(
        cache::load(None, &tok("123456789012345678", "new.sig")).is_some(),
        "same account, new token"
    );
    assert!(
        cache::load(None, &tok("999", "x.y")).is_none(),
        "other account"
    );
    assert!(cache::load(None, "garbage").is_none());
    // 分割表示のもう片方（別のプロファイル）からは見えない
    assert!(
        cache::load(Some("split-test"), &tok("123456789012345678", "new.sig")).is_none(),
        "other profile"
    );
    cache::remove(None);
    assert!(cache::load(None, &tok("123456789012345678", "new.sig")).is_none());
}

/// 分割表示で並べた2つの区画は、トークン・設定・記録・プラグインのデータを別々の場所に持つ
#[test]
fn split_panes_keep_separate_storage() {
    let mut a = App::default();
    a.set_profile(None);
    let mut b = App::default();
    b.set_profile(Some("split-store".into()));
    assert_ne!(a.token_path(), b.token_path());
    assert_ne!(a.message_log.path, b.message_log.path);
    assert_ne!(a.plugins.data_root, b.plugins.data_root);
    assert!(b
        .token_path()
        .starts_with(crate::profile::account_dir(Some("split-store"))));
    b.settings.zoom = 1.75;
    b.save_settings();
    assert_eq!(crate::app::load_settings(Some("split-store")).zoom, 1.75);
    assert_ne!(crate::app::load_settings(None).zoom, 1.75);
    // reset（ログアウト）してもプロファイルは変わらない
    b.reset();
    assert_eq!(b.profile.as_deref(), Some("split-store"));
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some("split-store")));
}

/// つなぎ始めたら「ログイン中」画面ではなくホーム画面を出す
#[test]
fn home_is_shown_while_connecting() {
    let mut app = App::default();
    assert!(!app.in_home());
    let (_tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    assert!(app.me.is_none());
    assert!(app.in_home());
}

/// 自分のアクティビティ（「Riscord をプレイ中」など）は SESSIONS_REPLACE のまとめから自分の欄に入る
#[test]
fn own_presence_is_stored_under_my_id() {
    let mut app = App::default();
    app.me = Some(User {
        id: "1".into(),
        username: "me".into(),
        global_name: None,
        avatar: None,
        premium_type: None,
        bot: false,
        avatar_decoration_data: None,
    });
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    tx.send(Event::OwnPresence(Presence {
        status: "online".into(),
        activity: Some("プレイ中: Riscord".into()),
    }))
    .unwrap();
    app.poll();
    assert_eq!(
        app.presences["1"].activity.as_deref(),
        Some("プレイ中: Riscord")
    );
}

/// 描画側は msg_rev が同じなら行の目印を作り直さない。メッセージが変わる通知では必ず進み、関係ない通知では進まないこと
#[test]
fn message_revision_advances_only_on_message_changes() {
    let mut app = bench_app(3);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let r0 = app.msg_rev;
    tx.send(Event::Status("接続済み".into())).unwrap();
    app.poll();
    assert_eq!(
        app.msg_rev, r0,
        "unrelated events must keep the cached row keys"
    );
    tx.send(Event::MessageUpdate {
        channel_id: "c".into(),
        id: "100001".into(),
        content: Some("edited".into()),
        embeds: None,
        pinned: None,
    })
    .unwrap();
    app.poll();
    let r1 = app.msg_rev;
    assert_ne!(r1, r0);
    tx.send(Event::MessageDelete {
        channel_id: "c".into(),
        id: "100002".into(),
    })
    .unwrap();
    app.poll();
    assert_ne!(app.msg_rev, r1);
    // 作り直した App でも前の番号に戻らない（ウィンドウ側のキャッシュが古い目印を使わないように）
    let mut fresh = App::default();
    fresh.bump_msgs();
    assert!(fresh.msg_rev > app.msg_rev);
}

/// @自分・@everyone・自分のロールへのメンションを数え、見ているチャンネル・自分の発言・DM 以外の普通の発言は数えない
#[test]
fn mentions_are_counted_per_server() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.me = Some(User {
        id: "me".into(),
        username: "me".into(),
        global_name: None,
        avatar: None,
        premium_type: None,
        bot: false,
        avatar_decoration_data: None,
    });
    tx.send(Event::ReadyExtra(serde_json::json!({
        "my_roles": { "g": ["r1"] },
        "channel_guild": { "c1": "g" },
        "read_state": { "entries": [{ "id": "c1", "mention_count": 2 }, { "id": "c2", "mention_count": 0 }] },
    })))
    .unwrap();
    app.poll();
    assert_eq!(app.mention_count(Some("g")), 2);
    let msg = |id: &str,
               ch: &str,
               guild: Option<&str>,
               author: &str,
               extra: serde_json::Value|
     -> Message {
        let mut v = serde_json::json!({
            "id": id, "channel_id": ch, "guild_id": guild,
            "author": { "id": author, "username": author },
            "content": "hi", "timestamp": "2026-11-01T10:00:00+00:00"
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    };
    let me_user = serde_json::json!([{ "id": "me", "username": "me" }]);
    tx.send(Event::MessageCreate(msg(
        "1",
        "c1",
        Some("g"),
        "x",
        serde_json::json!({ "mentions": me_user }),
    )))
    .unwrap();
    tx.send(Event::MessageCreate(msg(
        "2",
        "c3",
        Some("g"),
        "x",
        serde_json::json!({ "mention_roles": ["r1"] }),
    )))
    .unwrap();
    tx.send(Event::MessageCreate(msg(
        "3",
        "c3",
        Some("g"),
        "x",
        serde_json::json!({ "mention_everyone": true }),
    )))
    .unwrap();
    tx.send(Event::MessageCreate(msg(
        "4",
        "c3",
        Some("g"),
        "x",
        serde_json::json!({ "mention_roles": ["other"] }),
    )))
    .unwrap();
    tx.send(Event::MessageCreate(msg(
        "5",
        "c3",
        Some("g"),
        "me",
        serde_json::json!({ "mentions": me_user }),
    )))
    .unwrap();
    tx.send(Event::MessageCreate(msg(
        "6",
        "d1",
        None,
        "x",
        serde_json::json!({}),
    )))
    .unwrap();
    app.poll();
    assert_eq!(app.mention_count(Some("g")), 5);
    assert_eq!(app.mention_count(None), 1);
    // 開いたチャンネルの分は消える
    app.open_channel("c3".into());
    assert_eq!(app.mention_count(Some("g")), 3);
    // 見ているチャンネルに来たメンションは数えない
    app.window_focused = true;
    tx.send(Event::MessageCreate(msg(
        "7",
        "c3",
        Some("g"),
        "x",
        serde_json::json!({ "mentions": me_user }),
    )))
    .unwrap();
    app.poll();
    assert_eq!(app.mention_count(Some("g")), 3);
    app.mark_guild_read("g");
    assert_eq!(app.mention_count(Some("g")), 0);
}

fn guild(id: &str) -> Guild {
    serde_json::from_value(serde_json::json!({ "id": id, "name": id })).unwrap()
}

fn guild_ids(app: &App) -> Vec<&str> {
    app.guilds.iter().map(|g| g.id.as_str()).collect()
}

/// ドラッグで下へ動かすと相手の後ろ、上へ動かすと相手の前に入り、並びが settings に残る
#[test]
fn move_guild_reorders_and_records_order() {
    let mut app = App::default();
    app.guilds = ["a", "b", "c", "d"].map(guild).to_vec();
    assert!(app.move_guild("a", "c"));
    assert_eq!(guild_ids(&app), ["b", "c", "a", "d"]);
    assert!(app.move_guild("d", "b"));
    assert_eq!(guild_ids(&app), ["d", "b", "c", "a"]);
    assert_eq!(app.settings.guild_order, ["d", "b", "c", "a"]);
    // 自分自身や、ないサーバーへは動かさない
    assert!(!app.move_guild("b", "b"));
    assert!(!app.move_guild("x", "b"));
    assert_eq!(guild_ids(&app), ["d", "b", "c", "a"]);
}

/// 届いた一覧を保存した並びに並べ直す。知らないサーバーは後ろに元の順で、今は一覧にないサーバーの位置も忘れない
#[test]
fn guild_order_is_applied_to_new_lists() {
    let mut app = App::default();
    app.settings.guild_order = vec!["c".into(), "gone".into(), "a".into()];
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    tx.send(Event::Guilds(
        ["a", "new1", "b", "c", "new2"].map(guild).to_vec(),
    ))
    .unwrap();
    app.poll();
    assert_eq!(guild_ids(&app), ["c", "a", "new1", "b", "new2"]);
    app.move_guild("b", "c");
    assert_eq!(
        app.settings.guild_order,
        ["b", "c", "a", "new1", "new2", "gone"]
    );
}

#[test]
fn voice_call_timer_tracks_connection_transitions() {
    let mut app = App::default();
    app.voice_status = Some(VoiceStatus::Connecting {
        guild_id: Some("g".into()),
        channel_id: "one".into(),
    });
    app.update_voice_call_started();
    assert!(app.voice_call_started.is_none());
    app.voice_status = Some(VoiceStatus::Connected {
        guild_id: Some("g".into()),
        channel_id: "one".into(),
    });
    app.update_voice_call_started();
    let first = std::time::Instant::now() - std::time::Duration::from_secs(12);
    app.voice_call_started.as_mut().unwrap().2 = first;
    app.update_voice_call_started();
    assert_eq!(app.voice_call_started.as_ref().unwrap().2, first);
    app.voice_status = Some(VoiceStatus::Connected {
        guild_id: None,
        channel_id: "two".into(),
    });
    app.update_voice_call_started();
    let (guild, channel, started) = app.voice_call_started.as_ref().unwrap();
    assert!(guild.is_none());
    assert_eq!(channel, "two");
    assert!(*started > first);
    app.leave_voice();
    assert!(app.voice_call_started.is_none());
}

#[test]
fn voice_events_start_and_stop_call_timer() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    tx.send(Event::Voice(VoiceStatus::Connected {
        guild_id: None,
        channel_id: "dm".into(),
    }))
    .unwrap();
    app.poll();
    assert_eq!(app.voice_call_started.as_ref().unwrap().1, "dm");
    tx.send(Event::Voice(VoiceStatus::Disconnected)).unwrap();
    app.poll();
    assert!(app.voice_call_started.is_none());
}

#[test]
fn translated_outgoing_is_sent_only_for_current_successful_request() {
    let mut app = App::default();
    app.plugins.reload(&["translator".into()]);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let _tx = tx;
    app.compose_submissions.insert(
        "c".into(),
        ComposeSubmission {
            content: "$en こんにちは".into(),
            reply: None,
            editing: None,
            uploads: Vec::new(),
            failed: false,
            rejected: None,
        },
    );
    app.translator.outgoing.insert("c".into(), 2);
    let generation = app.plugins.generation;
    let target = |request_id| crate::translator::Target::Outgoing {
        channel: "c".into(),
        request_id,
        generation,
    };
    app.translator.inject(crate::translator::ResultItem {
        target: target(1),
        result: Ok("old".into()),
    });
    app.poll();
    assert!(rx.try_recv().is_err());
    app.translator.inject(crate::translator::ResultItem {
        target: target(2),
        result: Ok("hello".into()),
    });
    app.poll();
    assert!(matches!(rx.try_recv(), Ok(Cmd::Send { content, .. }) if content == "hello"));
    assert_eq!(app.compose_submissions["c"].content, "hello");
}

#[test]
fn failed_translation_retains_original_draft() {
    let mut app = App::default();
    app.plugins.reload(&["translator".into()]);
    app.compose_submissions.insert(
        "c".into(),
        ComposeSubmission {
            content: "$en 原文".into(),
            reply: None,
            editing: None,
            uploads: Vec::new(),
            failed: false,
            rejected: None,
        },
    );
    app.translator.outgoing.insert("c".into(), 4);
    app.translator.inject(crate::translator::ResultItem {
        target: crate::translator::Target::Outgoing {
            channel: "c".into(),
            request_id: 4,
            generation: app.plugins.generation,
        },
        result: Err("通信エラー".into()),
    });
    app.poll();
    assert!(app.compose_submissions["c"].failed);
    assert!(app.restore_compose("c"));
    assert_eq!(app.input, "$en 原文");
}

#[test]
fn unread_dms_lists_only_dms_with_counts_in_dm_order_until_opened() {
    let mut app = App::default();
    let dm = |id: &str| -> Channel {
        serde_json::from_value(serde_json::json!({ "id": id, "type": 1, "recipients": [{ "id": format!("u{id}"), "username": "u" }] })).unwrap()
    };
    app.dms = vec![dm("3"), dm("2"), dm("1")];
    app.mention_counts.insert("1".into(), (None, 2));
    app.mention_counts.insert("2".into(), (None, 0));
    app.mention_counts.insert("3".into(), (None, 5));
    // サーバーのチャンネルと、一覧にない DM は並べない
    app.mention_counts
        .insert("g".into(), (Some("100".into()), 4));
    app.mention_counts.insert("9".into(), (None, 1));
    let list: Vec<(&str, u32)> = app
        .unread_dms()
        .into_iter()
        .map(|(c, n)| (c.id.as_str(), n))
        .collect();
    assert_eq!(list, [("3", 5), ("1", 2)]);
    assert_eq!(app.mention_count(None), 8);
    app.open_channel("3".into());
    let list: Vec<(&str, u32)> = app
        .unread_dms()
        .into_iter()
        .map(|(c, n)| (c.id.as_str(), n))
        .collect();
    assert_eq!(list, [("1", 2)]);
}

#[test]
fn viewing_already_acked_channel_clears_leftover_unread_marks() {
    let mut app = bench_app(1);
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let _tx = tx;
    app.sync_read_state(true);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Ack { .. })));
    // 最新は既読のまま、件数だけが入り直した（つなぎ直しの READY や、届いてすぐ消されたメッセージ）
    app.mention_counts.insert("c".into(), (None, 1));
    app.unread.insert("c".into(), None);
    app.sync_read_state(false);
    assert!(app.mention_counts.contains_key("c"), "見ていない間は残す");
    app.sync_read_state(true);
    assert!(!app.mention_counts.contains_key("c") && !app.unread.contains_key("c"));
    assert!(rx.try_recv().is_err(), "既読は送り直さない");
}

#[test]
fn poll_vote_waits_for_refresh_and_rejects_duplicate_clicks() {
    use crate::net::ApiCall;
    let mut app = bench_app(1);
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.messages.get_mut("c").unwrap()[0].poll = Some(
        serde_json::from_value(serde_json::json!({
            "question": {"text":"Q"}, "answers": [{"answer_id":1,"poll_media":{"text":"A"}}],
            "results": {"answer_counts":[]}
        }))
        .unwrap(),
    );
    app.toggle_poll_vote("c", "100000", 1);
    app.toggle_poll_vote("c", "100000", 1);
    let Cmd::Api {
        id,
        call: call @ ApiCall::VotePoll { .. },
    } = rx.try_recv().unwrap()
    else {
        panic!("expected vote")
    };
    assert!(matches!(&call, ApiCall::VotePoll { answer_ids, .. } if answer_ids == &[1]));
    assert!(rx.try_recv().is_err());
    app.on_api_result(id, call, Ok(serde_json::Value::Null));
    let Cmd::Api {
        id,
        call: call @ ApiCall::GetMessage { .. },
    } = rx.try_recv().unwrap()
    else {
        panic!("expected refresh")
    };
    assert!(app
        .pending_poll_votes
        .contains(&("c".into(), "100000".into())));
    let mut fresh = serde_json::to_value(&app.messages["c"][0]).unwrap();
    fresh["poll"]["results"]["answer_counts"] =
        serde_json::json!([{"id":1,"count":1,"me_voted":true}]);
    app.on_api_result(id, call, Ok(fresh));
    assert!(app.pending_poll_votes.is_empty());
    assert!(
        app.messages["c"][0]
            .poll
            .as_ref()
            .unwrap()
            .results
            .as_ref()
            .unwrap()
            .answer_counts[0]
            .me_voted
    );
}

#[test]
fn poll_gateway_update_is_replayed_during_history_load() {
    let mut app = bench_app(1);
    app.me = Some(serde_json::from_value(serde_json::json!({"id":"me","username":"me"})).unwrap());
    app.messages.get_mut("c").unwrap()[0].poll = Some(serde_json::from_value(serde_json::json!({
        "question":{"text":"Q"},"answers":[{"answer_id":1,"poll_media":{"text":"A"}}],"results":{"answer_counts":[]}
    })).unwrap());
    let old = app.messages["c"].clone();
    app.pending_message_loads.insert("c".into(), Vec::new());
    app.on_gateway_event(
        "MESSAGE_POLL_VOTE_ADD",
        serde_json::json!({"channel_id":"c","message_id":"100000","answer_id":1,"user_id":"me"}),
    );
    let count = &app.messages["c"][0]
        .poll
        .as_ref()
        .unwrap()
        .results
        .as_ref()
        .unwrap()
        .answer_counts[0];
    assert_eq!((count.count, count.me_voted), (1, true));
    let mut loaded = old;
    for change in app.pending_message_loads.remove("c").unwrap() {
        change.apply(&mut loaded);
    }
    let count = &loaded[0]
        .poll
        .as_ref()
        .unwrap()
        .results
        .as_ref()
        .unwrap()
        .answer_counts[0];
    assert_eq!((count.count, count.me_voted), (1, true));
}

#[test]
fn poll_creation_validates_and_preserves_draft_after_failure() {
    let mut app = bench_app(0);
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.poll_draft = Some(PollDraft {
        channel_id: "c".into(),
        question: "Lunch?".into(),
        answers: "One".into(),
        duration: 24,
        ..Default::default()
    });
    app.submit_poll();
    assert!(rx.try_recv().is_err());
    assert!(app.poll_draft.as_ref().unwrap().error.is_some());
    app.poll_draft.as_mut().unwrap().answers = "One\nTwo".into();
    app.submit_poll();
    app.submit_poll();
    let Cmd::Api {
        id,
        call: call @ net::ApiCall::SendMessage { .. },
    } = rx.try_recv().unwrap()
    else {
        panic!("expected poll")
    };
    assert!(rx.try_recv().is_err());
    assert!(
        matches!(&call, net::ApiCall::SendMessage { body, .. } if body["poll"]["answers"].as_array().unwrap().len() == 2 && body["poll"]["duration"] == 24)
    );
    app.on_api_result(id, call, Err("forbidden".into()));
    let draft = app.poll_draft.as_ref().unwrap();
    assert!(draft.pending.is_none());
    assert_eq!(draft.answers, "One\nTwo");
    assert!(draft.error.is_some());
}

#[test]
fn poll_end_only_author_and_recovers_from_disconnection() {
    let mut app = bench_app(1);
    app.me =
        Some(serde_json::from_value(serde_json::json!({"id":"other","username":"other"})).unwrap());
    app.messages.get_mut("c").unwrap()[0].poll = Some(serde_json::from_value(serde_json::json!({
        "question":{"text":"Q"}, "answers":[{"answer_id":1,"poll_media":{"text":"A"}}], "results":{"answer_counts":[]}
    })).unwrap());
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.end_poll("c", "100000");
    assert!(rx.try_recv().is_err());
    app.me.as_mut().unwrap().id = "1".into();
    app.end_poll("c", "100000");
    app.end_poll("c", "100000");
    let Cmd::Api {
        id,
        call: call @ net::ApiCall::EndPoll { .. },
    } = rx.try_recv().unwrap()
    else {
        panic!("expected end poll")
    };
    assert!(rx.try_recv().is_err());
    drop(rx);
    app.on_api_result(id, call, Ok(serde_json::Value::Null));
    assert!(app.pending_poll_votes.is_empty());
    app.end_poll("c", "100000");
    assert!(app.pending_poll_votes.is_empty());
}

#[test]
fn guild_folders_preserve_membership_order_and_round_trip_settings() {
    let mut app = App::default();
    app.guilds = ["a", "b", "c", "d"].map(guild).to_vec();
    let id = app.create_guild_folder("b").unwrap();
    assert!(app.move_guild_to_folder("d", &id));
    assert!(!app.move_guild_to_folder("missing", &id));
    assert!(!app.move_guild_to_folder("a", "missing"));
    assert!(app.drop_guild_on_guild("a", "b"));
    assert!(app.settings.guild_folders[0]
        .guild_ids
        .contains(&"a".to_string()));
    assert!(app.drop_guild_on_guild("a", "c"));
    assert!(!app.settings.guild_folders[0]
        .guild_ids
        .contains(&"a".to_string()));
    app.settings.guild_folders[0].name = "Friends".into();
    app.settings.guild_folders[0].color = 0xed4245;
    app.settings.guild_folders[0].collapsed = true;
    let encoded = serde_json::to_vec(&app.settings).unwrap();
    let decoded: Settings = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded.guild_folders[0].name, "Friends");
    assert_eq!(decoded.guild_folders[0].color, 0xed4245);
    assert!(decoded.guild_folders[0].collapsed);
    assert!(decoded.guild_folders[0]
        .guild_ids
        .contains(&"b".to_string()));
    assert!(app.move_folder_to_guild(&id, "c"));
    let ids = guild_ids(&app);
    let b = ids.iter().position(|id| *id == "b").unwrap();
    let d = ids.iter().position(|id| *id == "d").unwrap();
    assert_eq!(b.abs_diff(d), 1);
    assert!(app.settings.guild_order.contains(&"b".into()));
    let old: Settings = serde_json::from_str("{}").unwrap();
    assert!(old.guild_folders.is_empty());
}

#[test]
fn friend_refresh_does_not_overwrite_new_gateway_events() {
    let mut app = App::default();
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.fetch_friends();
    let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
        panic!("expected request")
    };
    app.on_gateway_event(
        "RELATIONSHIP_ADD",
        serde_json::json!({"id":"1","type":1,"user":{"id":"1","username":"alice"}}),
    );
    app.on_api_result(id, call, Ok(serde_json::json!([])));
    assert_eq!(app.friends.relationships.len(), 1);
    let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
        panic!("expected retry")
    };
    app.on_api_result(
        id,
        call,
        Ok(serde_json::json!([{"id":"1","type":1,"user":{"id":"1","username":"alice"}}])),
    );
    assert!(app.friends.loaded);
    assert_eq!(app.friends.relationships[0].user.username, "alice");
}

#[test]
fn friend_actions_validate_state_and_retain_data_on_error() {
    let mut app = App::default();
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.friends_ready(
        &serde_json::json!([{"id":"1","type":3,"user":{"id":"1","username":"alice"}}]),
    );
    app.friend_action(FriendAction::Accept("2".into()));
    assert!(rx.try_recv().is_err());
    app.friend_action(FriendAction::Accept("1".into()));
    app.friend_action(FriendAction::Accept("1".into()));
    let Cmd::Api {
        id,
        call: call @ net::ApiCall::AddFriend(_),
    } = rx.try_recv().unwrap()
    else {
        panic!("expected accept")
    };
    assert!(rx.try_recv().is_err());
    app.on_api_result(id, call, Err("403 Forbidden".into()));
    assert_eq!(app.friends.relationships[0].kind, 3);
    assert!(app.friends.pending.is_empty());
    assert!(app.friends.error.is_some());
    app.friends.username = "bob".into();
    app.friend_action(FriendAction::Request);
    let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
        panic!("expected request")
    };
    app.on_api_result(id, call, Err("400 Bad Request".into()));
    assert_eq!(app.friends.username, "bob");
}

#[test]
fn friend_dm_response_is_validated_and_does_not_hijack_navigation() {
    let mut app = App::default();
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.friends_ready(
        &serde_json::json!([{"id":"1","type":1,"user":{"id":"1","username":"alice"}}]),
    );
    app.friends.open = true;
    app.friend_action(FriendAction::OpenDm("1".into()));
    let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
        panic!("expected DM creation")
    };
    app.friends.open = false;
    app.on_api_result(
        id,
        call,
        Ok(serde_json::json!({"id":"42","type":1,"recipients":[{"id":"1","username":"alice"}]})),
    );
    assert!(app.pending_open.is_none());
    assert_eq!(app.dms[0].id, "42");
    app.friend_action(FriendAction::OpenDm("1".into()));
    assert_eq!(app.pending_open.as_deref(), Some("42"));
}

#[test]
fn friend_success_during_refresh_triggers_a_fresh_snapshot() {
    let mut app = App::default();
    let (_, ev) = std::sync::mpsc::channel();
    let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    app.friends_ready(
        &serde_json::json!([{"id":"1","type":3,"user":{"id":"1","username":"alice"}}]),
    );
    app.fetch_friends();
    let Cmd::Api {
        id: read_id,
        call: read,
    } = rx.try_recv().unwrap()
    else {
        panic!("expected refresh")
    };
    app.friend_action(FriendAction::Accept("1".into()));
    let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
        panic!("expected accept")
    };
    app.on_api_result(id, call, Ok(serde_json::Value::Null));
    app.on_api_result(
        read_id,
        read,
        Ok(serde_json::json!([{"id":"1","type":3,"user":{"id":"1","username":"alice"}}])),
    );
    let Cmd::Api { id, call } = rx.try_recv().unwrap() else {
        panic!("expected fresh refresh")
    };
    app.on_api_result(
        id,
        call,
        Ok(serde_json::json!([{"id":"1","type":1,"user":{"id":"1","username":"alice"}}])),
    );
    assert_eq!(app.friends.relationships[0].kind, 1);
}

#[test]
fn category_collapse_hides_normal_channels_and_keeps_selected_or_unread() {
    let profile = "category-collapse-test";
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
    let mut app = App::default();
    app.set_profile(Some(profile.into()));
    app.sel_guild = Some("g1".into());
    let ch = |id: &str, kind: u8, pos: i32, parent: Option<&str>| -> Channel {
        serde_json::from_value(serde_json::json!({
            "id": id, "type": kind, "name": id, "position": pos, "parent_id": parent,
        }))
        .unwrap()
    };
    let (c1, c2, v1) = (
        ch("c1", 0, 0, Some("cat1")),
        ch("c2", 0, 1, Some("cat1")),
        ch("v1", 2, 0, Some("cat1")),
    );
    app.channels.insert(
        "g1".into(),
        vec![
            ch("cat1", 4, 0, None),
            c1.clone(),
            c2.clone(),
            ch("c3", 0, 2, Some("cat1")),
            v1.clone(),
        ],
    );
    assert_eq!(app.sidebar_order(), ["c1", "c2", "c3"]);
    app.toggle_category("cat1");
    assert!(app.is_category_collapsed("cat1"));
    assert!(load_settings(Some(profile))
        .collapsed_categories
        .contains("cat1"));
    assert!(app.sidebar_order().is_empty());
    assert!(!app.channel_visible_in_collapsed_category(&c1, None));
    assert!(!app.channel_visible_in_collapsed_category(&v1, None));
    // 選択中・未読あり・人がいるボイスチャンネルは閉じていても出す
    app.sel_channel = Some("c1".into());
    assert!(app.channel_visible_in_collapsed_category(&c1, None));
    app.unread.insert("c2".into(), None);
    assert!(app.channel_visible_in_collapsed_category(&c2, None));
    assert_eq!(app.sidebar_order(), ["c1", "c2"]);
    assert!(app.channel_visible_in_collapsed_category(&v1, Some("v1")));
    app.voice_states.insert(
        "u1".into(),
        model::VoiceState {
            user_id: "u1".into(),
            channel_id: Some("v1".into()),
            guild_id: Some("g1".into()),
            user: None,
            self_mute: false,
            self_deaf: false,
            self_stream: false,
        },
    );
    app.reindex_voice();
    assert!(app.channel_visible_in_collapsed_category(&v1, None));
    app.toggle_category("cat1");
    assert!(!app.is_category_collapsed("cat1"));
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
}

#[test]
fn switching_guilds_restores_last_channel_or_read_state_channel() {
    let mut app = App::default();
    let (tx, ev) = std::sync::mpsc::channel();
    let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.backend = Some(Backend { cmd, ev });
    let ch = |id: &str, kind: u8, pos: i32| -> Channel {
        serde_json::from_value(
            serde_json::json!({ "id": id, "type": kind, "name": id, "position": pos }),
        )
        .unwrap()
    };
    // g1 では c2 が c1 より最近見られている（ボイスは選ばない）
    tx.send(Event::ReadyExtra(serde_json::json!({
        "channel_guild": { "c1": "g1", "c2": "g1", "v1": "g1" },
        "read_state": { "entries": [
            { "id": "c1", "last_viewed": 3900, "last_message_id": "100" },
            { "id": "c2", "last_viewed": 3910, "last_message_id": "200" },
            { "id": "v1", "last_viewed": 3920, "last_message_id": "300" }
        ] }
    })))
    .unwrap();
    app.poll();
    // チャンネル一覧が届く前に切り替えたら、届いたときに開く
    app.pick_guild(Some("g1".into()));
    assert_eq!(app.sel_guild.as_deref(), Some("g1"));
    assert_eq!(app.sel_channel, None);
    tx.send(Event::GuildChannels(
        "g1".into(),
        vec![ch("c1", 0, 0), ch("c2", 0, 1), ch("v1", 2, 0)],
    ))
    .unwrap();
    app.poll();
    assert_eq!(app.sel_channel.as_deref(), Some("c2"));
    app.open_channel("c1".into());
    assert_eq!(
        app.settings.last_channels.get("g1").map(String::as_str),
        Some("c1")
    );
    assert!(app.settings_dirty, "チャンネルを替えるたびには書かない");
    // read_state のないサーバーは一番上のテキストチャンネル
    tx.send(Event::GuildChannels(
        "g2".into(),
        vec![ch("g2_c1", 0, 0), ch("g2_c2", 0, 1)],
    ))
    .unwrap();
    app.poll();
    app.pick_guild(Some("g2".into()));
    assert_eq!(app.sel_channel.as_deref(), Some("g2_c1"));
    app.open_channel("g2_c2".into());
    app.pick_guild(Some("g1".into()));
    assert_eq!(app.sel_channel.as_deref(), Some("c1"));
    app.pick_guild(Some("g2".into()));
    assert_eq!(app.sel_channel.as_deref(), Some("g2_c2"));
    // 同じサーバーをもう一度押しても、開いているチャンネルは変えない
    app.open_channel("g2_c1".into());
    app.pick_guild(Some("g2".into()));
    assert_eq!(app.sel_channel.as_deref(), Some("g2_c1"));
    // 消えたチャンネルを覚えていたら、他のチャンネルを選ぶ
    app.settings
        .last_channels
        .insert("g1".into(), "gone".into());
    assert_eq!(app.default_guild_channel("g1").as_deref(), Some("c2"));
}

#[test]
fn pane_widths_leave_room_for_chat_and_persist() {
    // 900px で両方 480px に広げても、チャット欄は MIN_CHAT_W 残る
    let (ch, mem) = fit_panes(900.0, 480.0, Some(480.0));
    assert!(900.0 - RAIL_W - ch - mem >= MIN_CHAT_W - 0.01, "{ch} {mem}");
    assert_eq!(mem, MEMBER_PANE_RANGE.0, "先にメンバー一覧を縮める");
    // 広い画面では保存した幅のまま。範囲外・壊れた値は直す
    assert_eq!(fit_panes(1920.0, 300.0, Some(280.0)), (300.0, 280.0));
    assert_eq!(fit_panes(1920.0, 9999.0, None), (480.0, 0.0));
    assert_eq!(fit_panes(1920.0, f32::NAN, None).0, PANE_DEFAULT);
    assert_eq!(fit_panes(1920.0, 0.0, None).0, PANE_DEFAULT);
    // とても狭い（重ねて出す）画面では幅を取り合わない
    assert_eq!(fit_panes(600.0, 400.0, None), (400.0, 0.0));

    let profile = "pane-width-test";
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
    let mut app = App::default();
    app.set_profile(Some(profile.into()));
    app.screen_w = 900.0;
    app.sel_guild = Some("g1".into());
    app.show_members = true;
    assert!(app.members_inline());
    // ドラッグでも、もう一方のペインとチャット欄の分は残す（既定の幅でほぼ埋まっている）
    assert!(!app.drag_pane(true, 1000.0));
    // メンバー一覧を縮めれば、空いた分だけチャンネル一覧を広げられる
    assert!(app.drag_pane(false, 100.0));
    assert_eq!(app.pane_widths().1, MEMBER_PANE_RANGE.0);
    assert!(app.drag_pane(true, 1000.0));
    let (ch, mem) = app.pane_widths();
    assert_eq!(ch + mem, 900.0 - RAIL_W - MIN_CHAT_W);
    app.drag_pane(false, 1000.0);
    let (ch, mem) = app.pane_widths();
    assert!(900.0 - RAIL_W - ch - mem >= MIN_CHAT_W - 0.01, "{ch} {mem}");
    app.save_settings();
    let loaded = load_settings(Some(profile));
    assert_eq!(loaded.channel_pane_width, app.settings.channel_pane_width);
    assert_eq!(loaded.member_pane_width, app.settings.member_pane_width);
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
}

#[test]
fn avatar_and_decoration_urls_resolve_user_and_guild_overrides() {
    let user: User = serde_json::from_value(serde_json::json!({
        "id": "10", "username": "u", "avatar": "a_abc",
        "avatar_decoration_data": {"asset": "a_userdeco"}
    }))
    .unwrap();
    let member: Member = serde_json::from_value(serde_json::json!({
        "nick": "n", "avatar": "guildav", "avatar_decoration_data": {"asset": "guilddeco"}
    }))
    .unwrap();
    // サーバー別のアバターとデコレーションが優先。どちらも静止画で取る
    assert_eq!(
        member_avatar_url("1", &user, Some(&member)),
        "https://cdn.discordapp.com/guilds/1/users/10/avatars/guildav.png?size=80"
    );
    assert!(decoration_for(&user, Some(&member))
        .unwrap()
        .contains("/avatar-decoration-presets/guilddeco.png"));
    // サーバー別がなければユーザーのもの
    assert_eq!(member_avatar_url("1", &user, None), user_avatar_url(&user));
    let plain: Member = serde_json::from_value(serde_json::json!({"roles": []})).unwrap();
    assert!(decoration_for(&user, Some(&plain))
        .unwrap()
        .contains("a_userdeco.png?size=96&passthrough=false"));
    // URL に入れられない値は使わない
    let bad: Member = serde_json::from_value(
        serde_json::json!({"avatar": "../x", "avatar_decoration_data": {"asset": "a/b"}}),
    )
    .unwrap();
    assert_eq!(
        member_avatar_url("1", &user, Some(&bad)),
        user_avatar_url(&user)
    );
    let nobody: User =
        serde_json::from_value(serde_json::json!({"id": "11", "username": "x"})).unwrap();
    assert!(decoration_for(&nobody, Some(&bad)).is_none());
}

#[test]
fn muted_channels_are_read_and_hidden_when_asked() {
    let (gid, s) = GuildSettings::parse(&serde_json::json!({
        "guild_id": "g1",
        "channel_overrides": [
            {"channel_id": "c1", "muted": true},
            {"channel_id": "cat", "muted": true, "mute_config": {"end_time": "2999-01-01T00:00:00+00:00"}},
            {"channel_id": "old", "muted": true, "mute_config": {"end_time": "2000-01-01T00:00:00+00:00"}},
            {"channel_id": "c2", "muted": false}
        ]
    }))
    .unwrap();
    assert_eq!(gid, "g1");
    let mut muted: Vec<&str> = s.muted_channels.iter().map(String::as_str).collect();
    muted.sort();
    assert_eq!(
        muted,
        ["c1", "cat"],
        "期限切れとミュートでないものは入れない"
    );
    // オブジェクトの形でも読める
    let (_, s2) = GuildSettings::parse(&serde_json::json!({
        "guild_id": "g1",
        "channel_overrides": {"c9": {"channel_id": "c9", "muted": true}}
    }))
    .unwrap();
    assert!(s2.muted_channels.contains("c9"));

    let profile = "hide-muted-test";
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
    let mut app = App::default();
    app.set_profile(Some(profile.into()));
    app.sel_guild = Some("g1".into());
    app.guild_settings.insert("g1".into(), s);
    let ch = |id: &str, kind: u8, parent: Option<&str>| -> Channel {
        serde_json::from_value(
            serde_json::json!({"id": id, "type": kind, "name": id, "parent_id": parent}),
        )
        .unwrap()
    };
    let (c1, c2, in_cat, v) = (
        ch("c1", 0, None),
        ch("c2", 0, None),
        ch("c3", 0, Some("cat")),
        ch("v", 2, Some("cat")),
    );
    app.channels.insert(
        "g1".into(),
        vec![
            c1.clone(),
            c2.clone(),
            ch("cat", 4, None),
            in_cat.clone(),
            v.clone(),
        ],
    );
    assert_eq!(
        app.sidebar_order(),
        ["c1", "c2", "c3"],
        "切り替えるまでは隠さない"
    );
    app.toggle_hide_muted("g1");
    assert!(load_settings(Some(profile))
        .hide_muted_guilds
        .contains("g1"));
    // ミュートしたカテゴリの中のチャンネルも隠す
    assert_eq!(app.sidebar_order(), ["c2"]);
    assert!(app.hidden_as_muted("g1", &v, None));
    // 選んでいるチャンネルと、入っているボイスチャンネルは隠さない
    app.sel_channel = Some("c1".into());
    assert!(!app.hidden_as_muted("g1", &c1, None));
    assert!(!app.hidden_as_muted("g1", &v, Some("v")));
    app.toggle_hide_muted("g1");
    assert!(!app.hidden_as_muted("g1", &in_cat, None));
    let _ = std::fs::remove_dir_all(crate::profile::account_dir(Some(profile)));
}

/// チャンネルのメンションは、メッセージのサーバーを先に見て、無ければ他のサーバーからも探す
#[test]
fn channel_mentions_prefer_own_guild_then_fall_back() {
    let m: Message = serde_json::from_value(serde_json::json!({
        "id": "1", "channel_id": "c", "guild_id": "g1", "author": { "id": "9", "username": "a" },
        "timestamp": "2026-11-01T00:00:00+00:00", "content": "",
    }))
    .unwrap();
    let ch = |id: &str, name: &str| -> Channel {
        serde_json::from_value(serde_json::json!({ "id": id, "type": 0, "name": name })).unwrap()
    };
    let mut channels = HashMap::new();
    channels.insert("g1".to_string(), vec![ch("10", "一般")]);
    channels.insert("g2".to_string(), vec![ch("20", "よそ")]);
    let got = resolve_mentions("<#10> <#20> <#30>", &m, &channels, &[]);
    assert_eq!(got, "#一般 #よそ #不明なチャンネル");
}

/// 右クリックの対象は押されたときに ID で引く（チャンネルの一覧・ピン・検索結果のどれからでも）
#[test]
fn find_message_looks_in_list_pins_and_search() {
    let mut app = bench_app(3);
    assert_eq!(
        app.find_message("c", "100001").map(|m| m.content),
        Some("message 1".to_string())
    );
    let pinned: Message = serde_json::from_value(serde_json::json!({
        "id": "500", "channel_id": "other", "author": { "id": "1", "username": "u" },
        "timestamp": "2026-11-01T00:00:00+00:00", "content": "pinned",
    }))
    .unwrap();
    let mut searched = pinned.clone();
    searched.id = "600".into();
    searched.content = "found".into();
    let mut pins = pins::PinsView::default();
    pins.items.push(pins::PinItem {
        pinned_at: String::new(),
        message: pinned,
    });
    app.pins = Some(pins);
    let mut search = search::SearchView::default();
    search.hits.push(search::SearchHit {
        message: searched,
        place: String::new(),
    });
    app.search = Some(search);
    assert_eq!(app.find_message("other", "500").unwrap().content, "pinned");
    assert_eq!(app.find_message("other", "600").unwrap().content, "found");
    assert!(app.find_message("c", "999").is_none());
}
