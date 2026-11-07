//! UI に依存しない既存動作の回帰テスト。
use super::*;

fn bench_app(n_msgs: usize) -> App {
    let mut app = App::default();
    let msgs = (0..n_msgs).map(|i| serde_json::from_value(serde_json::json!({
        "id": format!("{}", 100000 + i), "channel_id": "c",
        "author": { "id": "1", "username": "u" },
        "content": format!("message {i}"), "timestamp": "2026-11-01T10:00:00+00:00"
    })).unwrap()).collect();
    app.messages.insert("c".into(), msgs);
    app.touch_channel("c");
    app.sel_channel = Some("c".into());
    app
}

    #[test]
    fn member_chunks_preserve_order_and_latest_updates() {
        let mut app = App::default();
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        let member = |id: &str, name: &str| Member {
            user: Some(User { id: id.into(), username: name.into(), global_name: None, avatar: None, premium_type: None }),
            nick: None, roles: Vec::new(),
        };
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "old"), member("2", "two")])).unwrap();
        tx.send(Event::GuildMembers("b".into(), vec![member("1", "other")])).unwrap();
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "new"), member("3", "three"), Member { user: None, nick: None, roles: vec![] }])).unwrap();
        app.poll();
        let a: Vec<_> = app.members["a"].iter().map(|m| (m.user.as_ref().unwrap().id.as_str(), m.display())).collect();
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
            user: Some(User { id: id.into(), username: name.into(), global_name: None, avatar: None, premium_type: None }),
            nick: None, roles: vec![],
        };
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "old")])).unwrap();
        app.poll();
        assert_eq!(app.member_index.as_ref().unwrap().guild_id, "a");
        tx.send(Event::GuildMembers("a".into(), vec![member("1", "new"), member("2", "two")])).unwrap();
        app.poll();
        assert_eq!(app.members["a"].iter().map(Member::display).collect::<Vec<_>>(), ["new", "two"]);
        tx.send(Event::GuildMembers("b".into(), vec![member("1", "other")])).unwrap();
        app.poll();
        assert_eq!(app.member_index.as_ref().unwrap().guild_id, "b");
        tx.send(Event::GuildMembers("a".into(), vec![member("2", "updated")])).unwrap();
        app.poll();
        assert_eq!(app.members["a"].iter().map(Member::display).collect::<Vec<_>>(), ["new", "updated"]);
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
        tx.send(Event::MessageUpdate { channel_id: "c".into(), id: snapshot[0].id.clone(), content: Some("edited".into()), embeds: None }).unwrap();
        tx.send(Event::MessageDelete { channel_id: "c".into(), id: snapshot[1].id.clone() }).unwrap();
        app.poll();
        tx.send(Event::Messages("c".into(), snapshot)).unwrap();
        app.poll();
        let list = &app.messages["c"];
        assert_eq!(list.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["100000", "100002", "100003"]);
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
        tx.send(Event::ComposeResult { channel_id: "c".into(), result: Ok(Some(sent)) }).unwrap();
        app.poll();
        tx.send(Event::Messages("c".into(), snapshot)).unwrap();
        app.poll();
        assert_eq!(app.messages["c"].iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["100000", "100001", "100002"]);
    }


    #[test]
    fn compose_failure_retains_text_reply_and_attachment_until_restored() {
        let mut app = bench_app(1);
        let (tx, ev) = std::sync::mpsc::channel();
        let (cmd, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.backend = Some(Backend { cmd, ev });
        app.reply_to = Some(app.messages["c"][0].clone());
        app.uploads.push(clip::Upload { name: "test.txt".into(), data: vec![1, 2], thumb: None });
        app.submit_compose("c".into(), "hello".into());
        assert!(matches!(rx.try_recv(), Ok(Cmd::Send { content, files, reply_to, .. }) if content == "hello" && files[0].1 == [1, 2] && reply_to.is_some()));
        app.submit_compose("c".into(), "duplicate".into());
        assert!(rx.try_recv().is_err());
        tx.send(Event::ComposeResult { channel_id: "c".into(), result: Err(()) }).unwrap();
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
        tx.send(Event::ComposeResult { channel_id: "c".into(), result: Ok(Some(sent.clone())) }).unwrap();
        tx.send(Event::MessageCreate(sent)).unwrap();
        app.poll();
        assert_eq!(app.messages["c"].iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["100000", "100001", "100002", "100003"]);
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
        tx.send(Event::ComposeResult { channel_id: "c".into(), result: Err(()) }).unwrap();
        app.poll();
        assert!(app.restore_compose("c"));
        assert_eq!(app.editing.as_ref().unwrap().id, original.id);
        assert_eq!(app.input, "changed");
        app.input.clear();
        app.submit_compose("c".into(), "changed".into());
        let mut response = original.clone();
        response.content = "changed".into();
        tx.send(Event::MessageUpdate { channel_id: "c".into(), id: original.id, content: Some("newer".into()), embeds: None }).unwrap();
        tx.send(Event::ComposeResult { channel_id: "c".into(), result: Ok(Some(response)) }).unwrap();
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
                    user_id: format!("u{n}"), guild_id: Some("g".into()),
                    channel_id: ((batch + i) % 5 != 0).then(|| format!("c{}", (batch + n) % 4)),
                    self_mute: (batch + i) % 2 == 0, self_deaf: i % 3 == 0, user: None,
                });
            }
            tx.send(Event::VoiceStates(updates)).unwrap();
            app.poll();
            let mut expected: HashMap<String, Vec<String>> = HashMap::new();
            for (id, state) in &app.voice_states {
                if let Some(channel) = &state.channel_id {
                    expected.entry(channel.clone()).or_default().push(id.clone());
                }
            }
            for ids in expected.values_mut() { ids.sort(); }
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
        assert_eq!(k, ["T(hi )", "E(pog,123,false)", "T( and )", "E(dance,456,true)", "T(!<bad> <@789>)"]);
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
        tx.send(Event::Messages("c".into(), old_messages.clone())).unwrap();
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
                m.sticker_items.push(model::StickerItem { id: id.into(), name: id.into(), format_type: 1 });
            }
            app.stickers.insert("old".into(), StickerState::Loading);
            app.stickers.insert("shared".into(), StickerState::Loading);
            let (tx, ev) = std::sync::mpsc::channel();
            let (cmd, _rx) = tokio::sync::mpsc::unbounded_channel();
            app.backend = Some(Backend { cmd, ev });
            if replace {
                tx.send(Event::Messages("c".into(), app.messages["c"][1..].to_vec())).unwrap();
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
    let file = |name: &str, size: usize| clip::Upload { name: name.into(), data: vec![0; size], thumb: None };
    app.add_uploads(vec![file("small.png", MIB as usize), file("large.mp4", (11 * MIB) as usize)], 0, Vec::new());
    assert_eq!(app.uploads.iter().map(|u| u.name.as_str()).collect::<Vec<_>>(), ["small.png"]);
    assert!(app.compose_notice.take().unwrap().0.contains("large.mp4"));

    app.uploads.clear();
    app.sel_guild = Some("guild".into());
    app.add_uploads(vec![file("large.mp4", (11 * MIB) as usize)], 0, Vec::new());
    assert_eq!(app.uploads.len(), 1);
    assert!(app.compose_notice.take().unwrap().0.contains("サーバーのブースト"));

    app.uploads.clear();
    app.add_uploads((0..MAX_UPLOADS + 2).map(|i| file(&format!("file{i}"), 1)).collect(), 0, Vec::new());
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
    assert!(!app.voice.gate(), "refocusing must not restore an old key press");
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
    let mut draft = ProfileDraft { fields: Default::default(), base: Default::default(),
        avatar: Some(AvatarEdit::Set { uri: "data:image/png;base64,".into(), tex: thumbnail.clone() }) };
    draft.avatar = Some(AvatarEdit::Remove);
    assert!(images::release_queued(&thumbnail));
}

#[test]
fn reset_releases_sticker_images() {
    let image = images::render_image(image::RgbaImage::new(1, 1));
    let mut app = App::default();
    app.stickers.insert("sticker".into(), StickerState::Ready(image.clone()));
    app.reset();
    assert!(images::release_queued(&image));
}

#[test]
fn pruning_releases_only_unused_sticker_images() {
    let old = images::render_image(image::RgbaImage::new(1, 1));
    let shared = images::render_image(image::RgbaImage::new(1, 1));
    let mut app = bench_app(1);
    app.messages.get_mut("c").unwrap()[0].sticker_items.push(model::StickerItem {
        id: "shared".into(), name: "shared".into(), format_type: 1,
    });
    app.stickers.insert("old".into(), StickerState::Ready(old.clone()));
    app.stickers.insert("shared".into(), StickerState::Ready(shared.clone()));
    app.prune_stickers();
    assert!(images::release_queued(&old));
    assert!(!images::release_queued(&shared));
}

#[test]
fn reset_forgets_cached_channel_images() {
    let path = std::env::temp_dir().join(format!("riscord-reset-image-{}.png", std::process::id()));
    image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])).save(&path).unwrap();
    let uri = format!("file://{}", path.display());
    assert!(matches!(images::get(&uri), images::Img::Pending));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let image = loop {
        match images::peek(&uri) {
            images::Img::Ready(image) => break image,
            images::Img::Failed => panic!("test image could not be decoded"),
            images::Img::Pending if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
            images::Img::Pending => panic!("test image timed out"),
        }
    };
    let mut app = App::default();
    app.channel_images.entry("a".into()).or_default().insert(uri.clone());
    app.channel_images.entry("b".into()).or_default().insert(uri.clone());
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
    assert!(matches!(rx.try_recv(), Ok(Cmd::Ack { channel_id, message_id }) if channel_id == "c" && message_id == "100000"));
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
                if let Cmd::Ack { channel_id, message_id } = c { v.push((channel_id, message_id)); }
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
        assert!(acks(&mut rx).is_empty(), "見ていない（フォーカスがない）ときは送らない");
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
    events.send(Event::AckFailed { channel_id: "c".into(), message_id: "old".into() }).unwrap();
    app.poll();
    assert_eq!(app.acked.get("c").map(String::as_str), Some("100000"));
    events.send(Event::AckFailed { channel_id: "c".into(), message_id: "100000".into() }).unwrap();
    app.poll();
    assert!(!app.acked.contains_key("c"));
    app.last_ack = Some(std::time::Instant::now() - ACK_INTERVAL);
    app.sync_read_state(true);
    assert!(rx.try_recv().is_err(), "API 失敗後は連続して再試行しない");
    app.ack_retry_at.insert("c".into(), std::time::Instant::now() - std::time::Duration::from_secs(1));
    app.sync_read_state(true);
    assert!(matches!(rx.try_recv(), Ok(Cmd::Ack { channel_id, message_id }) if channel_id == "c" && message_id == "100000"));
}

#[test]
fn mentions_are_shown_by_name() {
    let m: Message = serde_json::from_value(serde_json::json!({
        "id": "1", "channel_id": "c", "author": { "id": "9", "username": "a" }, "timestamp": "2026-11-01T00:00:00+00:00",
        "content": "", "mentions": [{ "id": "5", "username": "taro", "global_name": "太郎" }],
    })).unwrap();
    let mut channels = HashMap::new();
    let ch: Channel = serde_json::from_value(serde_json::json!({ "id": "77", "type": 0, "name": "雑談" })).unwrap();
    channels.insert("g".to_string(), vec![ch]);
    let roles: Vec<Role> = serde_json::from_value(serde_json::json!([{ "id": "3", "name": "管理者", "color": 0, "position": 1 }])).unwrap();
    let got = resolve_mentions("<#77> へ <@5> <@!5> <@&3> <#88> <:e:1> <a> 1 < 2", &m, &channels, &roles);
    assert_eq!(got, "#雑談 へ @太郎 @太郎 @管理者 #不明なチャンネル <:e:1> <a> 1 < 2");
    assert!(matches!(resolve_mentions("普通の文", &m, &channels, &roles), std::borrow::Cow::Borrowed(_)));
}

/// 見出しの行に人数が無くても、一緒に届いた groups の人数で「ロール名 — 人数」を出す
#[test]
fn server_member_list_headers_use_group_counts() {
    let mut list = ServerMemberList { channel_id: "c".into(), ..Default::default() };
    let items = vec![net::ListItem::Group("online".into(), 0), net::ListItem::Member("m0".into()), net::ListItem::Member("m1".into())];
    assert!(list.apply("l", vec![("online".into(), 2)], vec![net::ListOp::Sync { range: [0, 99], items }]));
    let members: Vec<Member> = (0..2)
        .map(|i| Member { user: Some(User { id: format!("m{i}"), username: format!("u{i}"), global_name: None, avatar: None, premium_type: None }), nick: None, roles: vec![] })
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
    assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("コピーしました"));
}
