    use super::*;
    use super::inflate::Inflater;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMsg;

    #[tokio::test]
    async fn delete_notifies_only_after_http_success() {
        use std::io::{Read, Write};
        for (status, success) in [("204 No Content", true), ("500 Internal Server Error", false)] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = [0; 2048];
                let n = socket.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).starts_with("DELETE /channels/c/messages/m HTTP/1.1\r\n"));
                write!(socket, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            let api = Api { base_url: Some(format!("http://{addr}")), http: reqwest::Client::builder().no_proxy().build().unwrap(), token: "test".into(), roles: Default::default(), guild_perms: Default::default() };
            let (sender, events) = std::sync::mpsc::channel();
            let tx = Tx { tx: sender, ctx: crate::wake::Waker::default() };
            let result = delete_message(&api, &tx, "c".into(), "m".into()).await;
            server.join().unwrap();
            assert_eq!(result.is_ok(), success);
            if success {
                assert!(matches!(events.try_recv(), Ok(Event::MessageDelete { channel_id, id }) if channel_id == "c" && id == "m"));
            } else {
                assert!(events.try_recv().is_err());
            }
        }
    }

    #[tokio::test]
    async fn ack_http_error_notifies_backend_and_success_does_not() {
        use std::io::{Read, Write};
        for (status, fails) in [("500 Internal Server Error", true), ("204 No Content", false)] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut request = [0; 2048];
                let n = socket.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..n]);
                assert!(request.starts_with("POST /channels/c/messages/m/ack HTTP/1.1\r\n"), "{request}");
                write!(socket, "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            });
            let api = Api { base_url: Some(format!("http://{addr}")), http: reqwest::Client::builder().no_proxy().build().unwrap(), token: "test".into(), roles: Default::default(), guild_perms: Default::default() };
            let (sender, events) = std::sync::mpsc::channel();
            let tx = Tx { tx: sender, ctx: crate::wake::Waker::default() };
            let result = send_ack(&api, &tx, "c".into(), "m".into()).await;
            server.join().unwrap();
            assert_eq!(result.is_err(), fails);
            if fails {
                assert!(matches!(events.try_recv(), Ok(Event::AckFailed { channel_id, message_id }) if channel_id == "c" && message_id == "m"));
            } else {
                assert!(events.try_recv().is_err());
            }
        }
    }

    #[tokio::test]
    async fn api_read_timeout_releases_a_stalled_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, done) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (_socket, _) = listener.accept().unwrap();
            let _ = done.recv_timeout(Duration::from_secs(5));
        });
        let client = api_client(Duration::from_secs(1), Duration::from_millis(100)).unwrap();
        let result = client.get(format!("http://{addr}/")).send().await;
        let _ = release.send(());
        server.join().unwrap();
        assert!(result.unwrap_err().is_timeout());
    }

    #[test]
    fn rate_limit_delay_validates_untrusted_values() {
        assert_eq!(retry_delay(&json!({})).unwrap(), Duration::from_secs(1));
        assert_eq!(retry_delay(&json!({ "retry_after": 1.5 })).unwrap(), Duration::from_millis(1500));
        assert_eq!(retry_delay(&json!({ "retry_after": 0 })).unwrap(), Duration::from_millis(100));
        for invalid in [json!(-1), json!(1e100), json!("bad"), json!([])] {
            assert!(retry_delay(&json!({ "retry_after": invalid })).is_err());
        }
    }

    /// 大きいサーバーの GUILD_MEMBERS_CHUNK（1000人＋オンライン状態）相当
    fn members_chunk() -> Value {
        let members: Vec<Value> = (0..1000)
            .map(|i| json!({ "user": { "id": format!("{}", 10_000 + i), "username": format!("user{i}"), "global_name": format!("ユーザー {i}"), "avatar": "0123456789abcdef0123456789abcdef" }, "nick": null, "roles": ["1", "2", "3"], "joined_at": "2024-01-01T00:00:00+00:00" }))
            .collect();
        let presences: Vec<Value> = (0..1000)
            .map(|i| json!({ "user": { "id": format!("{}", 10_000 + i) }, "status": "online", "activities": [{ "type": 0, "name": "game" }] }))
            .collect();
        json!({ "op": 0, "s": 5, "t": "GUILD_MEMBERS_CHUNK", "d": { "guild_id": "42", "members": members, "presences": presences } })
    }

    struct Sink {
        tx: Tx,
        rx: std::sync::mpsc::Receiver<Event>,
        v_tx: UnboundedSender<VoiceMsg>,
    }

    /// 自分のセッション一覧からは、Discord がまとめた "all" の見え方を使う
    #[test]
    fn sessions_replace_reports_merged_own_presence() {
        let s = sink();
        let d = json!([
            { "session_id": "abcd", "status": "idle", "activities": [] },
            { "session_id": "all", "status": "online", "activities": [{ "type": 0, "name": "Riscord" }] },
        ]);
        dispatch("SESSIONS_REPLACE", d, &s.tx, &s.v_tx);
        let events: Vec<Event> = s.rx.try_iter().collect();
        assert!(matches!(&events[..], [Event::OwnPresence(p)] if p.status == "online" && p.activity.as_deref() == Some("プレイ中: Riscord")));
    }

    fn sink() -> Sink {
        let (tx, rx) = std::sync::mpsc::channel();
        let (v_tx, _) = unbounded_channel();
        Sink { tx: Tx { tx, ctx: crate::wake::Waker::default() }, rx, v_tx }
    }

    /// Discord と同じく、1接続で1つの圧縮文脈から Sync フラッシュで区切って送る
    fn zlib_stream(msgs: &[String]) -> Vec<Vec<u8>> {
        let mut z = flate2::Compress::new(flate2::Compression::default(), true);
        msgs.iter()
            .map(|m| {
                let mut out = Vec::with_capacity(m.len() + 1024);
                let mut input = m.as_bytes();
                loop {
                    if out.len() == out.capacity() { out.reserve(64 * 1024); }
                    let before = z.total_in();
                    z.compress_vec(input, &mut out, flate2::FlushCompress::Sync).unwrap();
                    input = &input[(z.total_in() - before) as usize..];
                    if input.is_empty() && out.len() < out.capacity() { break; }
                }
                out
            })
            .collect()
    }

    #[test]
    fn zlib_stream_roundtrip_with_fragments() {
        let big = members_chunk().to_string();
        let msgs = vec![
            json!({"op": 10, "d": {"heartbeat_interval": 41250}}).to_string(),
            big.clone(),
            json!({"op": 11}).to_string(),
            big,
            json!({"op": 0, "t": "MESSAGE_CREATE", "d": {"content": "日本語 ✓"}}).to_string(),
        ];
        let frames = zlib_stream(&msgs);
        assert!(frames.iter().all(|f| f.ends_with(&Inflater::SUFFIX)));
        let mut inf = Inflater::new();
        for (i, (f, m)) in frames.iter().zip(&msgs).enumerate() {
            // 分割受信: 途中までは None、最後の断片で1件そろう
            let parts: Vec<&[u8]> = if i % 2 == 1 { f.chunks(f.len() / 3 + 1).collect() } else { vec![&f[..]] };
            for (k, part) in parts.iter().enumerate() {
                let r = inf.push(part).unwrap().map(String::from);
                if k + 1 < parts.len() { assert!(r.is_none()); } else { assert_eq!(r.as_deref(), Some(m.as_str())); }
            }
        }
        // 大きいメッセージの後は作業領域を取り直している
        assert!(inf.buf.capacity() <= Inflater::KEEP && inf.out.capacity() <= Inflater::KEEP * 4);
        // 壊れたデータはエラー（呼び出し側で再接続する）
        let mut inf = Inflater::new();
        assert!(inf.push(&[1, 2, 3, 4, 0, 0, 0xff, 0xff]).is_err());
    }

    #[test]
    fn decode_frame_text_and_binary() {
        let mut inf = Inflater::new();
        let v = decode_frame(&WsMsg::Text(r#"{"op":11}"#.into()), &mut inf).unwrap().unwrap();
        assert_eq!(v["op"], 11);
        let f = zlib_stream(&[r#"{"op":10,"d":{"heartbeat_interval":5}}"#.to_string()]).remove(0);
        let v = decode_frame(&WsMsg::Binary(f.into()), &mut inf).unwrap().unwrap();
        assert_eq!(v["d"]["heartbeat_interval"], 5);
        assert!(decode_frame(&WsMsg::Ping(Vec::new().into()), &mut inf).unwrap().is_none());
    }

    #[test]
    fn decode_frame_releases_large_output_and_keeps_stream_context() {
        let text = json!({"op": 0, "d": "x".repeat(2_000_000)}).to_string();
        let frames = zlib_stream(&[text, r#"{"op":11}"#.into()]);
        let mut inflater = Inflater::new();
        let first = decode_frame(&WsMsg::Binary(frames[0].clone().into()), &mut inflater).unwrap().unwrap();
        assert_eq!(first["d"].as_str().unwrap().len(), 2_000_000);
        assert_eq!(inflater.out.capacity(), 0);
        let next = decode_frame(&WsMsg::Binary(frames[1].clone().into()), &mut inflater).unwrap().unwrap();
        assert_eq!(next["op"], 11);
    }

    #[test]
    fn decode_frame_releases_large_invalid_json_and_continues() {
        let frames = zlib_stream(&["x".repeat(2_000_000), r#"{"op":11}"#.into()]);
        let mut inflater = Inflater::new();
        assert!(decode_frame(&WsMsg::Binary(frames[0].clone().into()), &mut inflater).unwrap().is_none());
        assert_eq!(inflater.out.capacity(), 0);
        let next = decode_frame(&WsMsg::Binary(frames[1].clone().into()), &mut inflater).unwrap().unwrap();
        assert_eq!(next["op"], 11);
    }

    #[test]
    fn zlib_stream_decodes_independent_python_vectors() {
        // Python zlib.compressobj(), Z_SYNC_FLUSH; second packet reuses the first context.
        let first = [120,156,170,86,202,47,80,178,50,52,208,81,74,81,178,170,86,202,72,77,44,42,73,74,77,44,137,207,204,43,73,45,42,75,204,81,178,50,49,53,48,48,168,173,5,0,0,0,255,255];
        let second = [170,134,168,52,172,5,0,0,0,255,255];
        for _ in 0..2 {
            let mut inf = Inflater::new();
            for (packet, expected) in [(&first[..], r#"{"op":10,"d":{"heartbeat_interval":45000}}"#), (&second[..], r#"{"op":11}"#)] {
                for (i, byte) in packet.iter().enumerate() {
                    let got = inf.push(std::slice::from_ref(byte)).unwrap();
                    if i + 1 == packet.len() { assert_eq!(got, Some(expected)); } else { assert!(got.is_none()); }
                }
            }
        }
    }

    #[test]
    fn compressed_websocket_frames_work_after_reconnect() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            for _ in 0..2 {
                let (client_io, server_io) = tokio::io::duplex(4096);
                let mut client = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    client_io, tokio_tungstenite::tungstenite::protocol::Role::Client, None,
                ).await;
                let mut server = tokio_tungstenite::WebSocketStream::from_raw_socket(
                    server_io, tokio_tungstenite::tungstenite::protocol::Role::Server, None,
                ).await;
                let frames = zlib_stream(&[
                    r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into(),
                    r#"{"op":0,"t":"MESSAGE_UPDATE","d":{"channel_id":"1","id":"2","content":"test"}}"#.into(),
                ]);
                let split = frames[0].len() - 2;
                server.send(WsMsg::Binary(frames[0][..split].to_vec().into())).await.unwrap();
                server.send(WsMsg::Binary(frames[0][split..].to_vec().into())).await.unwrap();
                server.send(WsMsg::Binary(frames[1].clone().into())).await.unwrap();
                let mut inf = Inflater::new();
                assert!(decode_frame(&client.next().await.unwrap().unwrap(), &mut inf).unwrap().is_none());
                let hello = decode_frame(&client.next().await.unwrap().unwrap(), &mut inf).unwrap().unwrap();
                assert_eq!(hello["d"]["heartbeat_interval"], 45000);
                let event = decode_frame(&client.next().await.unwrap().unwrap(), &mut inf).unwrap().unwrap();
                let (kind, data) = split_payload(event);
                let s = sink();
                dispatch(&kind, data, &s.tx, &s.v_tx);
                assert!(matches!(s.rx.try_recv().unwrap(), Event::MessageUpdate { content: Some(c), .. } if c == "test"));
            }
        });
    }

    #[test]
    fn zlib_stream_enforces_compressed_and_expanded_limits() {
        let text = "a".repeat(128);
        let packet = zlib_stream(&[text.clone()]).remove(0);
        let mut exact = Inflater::new();
        assert_eq!(exact.push_with_limit(&packet, 128).unwrap(), Some(text.as_str()));
        let mut expanded = Inflater::new();
        assert!(expanded.push_with_limit(&packet, 127).is_err());
        let mut compressed = Inflater::new();
        assert!(compressed.push_with_limit(&[1; 8], 10).unwrap().is_none());
        assert!(compressed.push_with_limit(&[2; 3], 10).is_err());
    }

    #[test]
    fn zlib_stream_continues_after_output_buffer_boundaries() {
        let messages: Vec<String> = [65_535, 65_536, 65_537, 131_072, 1]
            .into_iter().map(|len| "x".repeat(len)).collect();
        let packets = zlib_stream(&messages);
        let mut inflater = Inflater::new();
        for (packet, expected) in packets.iter().zip(&messages) {
            assert_eq!(inflater.push(packet).unwrap(), Some(expected.as_str()));
        }
    }

    #[test]
    fn zlib_stream_releases_large_input_buffer_after_decoding() {
        let mut seed = 0x12345678u32;
        let text: String = (0..2_500_000).map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            char::from(b'!' + (seed % 90) as u8)
        }).collect();
        let packet = zlib_stream(std::slice::from_ref(&text)).remove(0);
        assert!(packet.len() > Inflater::KEEP);
        let mut inflater = Inflater::new();
        assert_eq!(inflater.push(&packet).unwrap(), Some(text.as_str()));
        assert_eq!(inflater.buf.capacity(), 0);
    }

    #[test]
    #[ignore]
    fn zlib_stream_actual_size_limit() {
        let limit = Inflater::MAX_MESSAGE;
        let exact = "x".repeat(limit);
        let packets = zlib_stream(&[exact, "next".into()]);
        let mut inflater = Inflater::new();
        assert_eq!(inflater.push(&packets[0]).unwrap().unwrap().len(), limit);
        assert_eq!(inflater.push(&packets[1]).unwrap(), Some("next"));
        drop(inflater);
        drop(packets);
        let over = "x".repeat(limit + 1);
        let packet = zlib_stream(std::slice::from_ref(&over)).remove(0);
        let mut inflater = Inflater::new();
        assert!(inflater.push(&packet).is_err());
    }

    #[test]
    #[ignore]
    fn bench_zlib_limit_allocation() {
        let limit = 2 << 20;
        let packet = zlib_stream(&["a".repeat(limit + 1)]).remove(0);
        let mut inf = Inflater::new();
        assert!(inf.push_with_limit(&packet, limit).is_err());
        println!("limit={limit}, output_capacity={}", inf.out.capacity());
    }

    /// 本物のゲートウェイに（トークン無しで）つなぎ、圧縮された Hello を展開できるか確かめる。identify は送らない
    /// cargo test --release live_gateway_hello -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gateway_hello() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (ws, _) = tokio_tungstenite::connect_async(GATEWAY).await.unwrap();
            let (mut write, mut read) = ws.split();
            let mut inf = Inflater::new();
            let mut binary = false;
            let hello = loop {
                let m = read.next().await.unwrap().unwrap();
                binary |= matches!(m, WsMsg::Binary(_));
                if let Some(v) = decode_frame(&m, &mut inf).unwrap() { break v; }
            };
            let _ = write.send(WsMsg::Close(None)).await;
            println!("LIVE binary={binary} hello={hello}");
            assert!(binary && hello["op"] == 10 && hello["d"]["heartbeat_interval"].as_u64().is_some());
        });
    }

    /// 圧縮率と展開にかかる時間
    #[test]
    #[ignore]
    fn bench_zlib_stream() {
        let text = members_chunk().to_string();
        let frames = zlib_stream(&vec![text.clone(); 20]);
        let comp: usize = frames.iter().map(Vec::len).sum();
        let mut inf = Inflater::new();
        let t = std::time::Instant::now();
        for f in &frames { assert_eq!(inf.push(f).unwrap().unwrap().len(), text.len()); }
        println!("ZLIB {} KB -> {} KB/件 ({:.1}%), 展開 {:.3} ms/件", text.len() / 1024, comp / 20 / 1024, comp as f64 / 20.0 / text.len() as f64 * 100.0, t.elapsed().as_secs_f64() * 1000.0 / 20.0);
    }

    #[test]
    #[ignore]
    fn bench_large_decode_buffer_release() {
        let large = json!({"op": 0, "d": "x".repeat(2_000_000)}).to_string();
        let messages: Vec<String> = (0..20).flat_map(|_| [large.clone(), r#"{"op":11}"#.into()]).collect();
        let frames: Vec<WsMsg> = zlib_stream(&messages).into_iter().map(|f| WsMsg::Binary(f.into())).collect();
        for early_release in [false, true, true, false] {
            let mut inf = Inflater::new();
            let start = std::time::Instant::now();
            let mut retained = 0;
            for (i, frame) in frames.iter().enumerate() {
                let value: Value = if early_release {
                    decode_frame(frame, &mut inf).unwrap().unwrap()
                } else if let WsMsg::Binary(bytes) = frame {
                    serde_json::from_str(inf.push(bytes).unwrap().unwrap()).unwrap()
                } else { unreachable!() };
                std::hint::black_box(value);
                if i % 2 == 0 { retained = inf.out.capacity(); }
            }
            println!("{}: {:.3} ms/pair, retained after large={} bytes", if early_release { "early" } else { "previous" }, start.elapsed().as_secs_f64() * 50.0, retained);
        }
    }

    #[test]
    #[ignore]
    fn bench_gzip_json() {
        use std::io::{Read, Write};
        let input = members_chunk().to_string();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(input.as_bytes()).unwrap();
        let gzip = encoder.finish().unwrap();
        let plain_start = std::time::Instant::now();
        for _ in 0..200 { std::hint::black_box(serde_json::from_slice::<Value>(input.as_bytes()).unwrap()); }
        let plain_ms = plain_start.elapsed().as_secs_f64() * 5.0;
        let start = std::time::Instant::now();
        for _ in 0..200 {
            let mut output = Vec::new();
            flate2::read::GzDecoder::new(gzip.as_slice()).read_to_end(&mut output).unwrap();
            assert_eq!(output, input.as_bytes());
            std::hint::black_box(serde_json::from_slice::<Value>(&output).unwrap());
        }
        println!("gzip JSON: {} -> {} bytes, plain parse {:.3} ms, decode+parse {:.3} ms", input.len(), gzip.len(), plain_ms, start.elapsed().as_secs_f64() * 5.0);
    }

    /// 保存済みトークンで実際の REST を測る。gzip なし／ありのクライアントを交互に呼んで同じ条件で比べる
    /// cargo test --release live_rest_bench -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_rest_bench() {
        let token = std::fs::read_to_string(dirs::config_dir().unwrap().join("riscord").join("token")).unwrap().trim().to_string();
        let ua = "Mozilla/5.0 (X11; Linux x86_64) riscord/0.1";
        let plain = Api { base_url: None, http: reqwest::Client::builder().user_agent(ua).no_gzip().build().unwrap(), token: token.clone(), roles: Default::default(), guild_perms: Default::default() };
        let gzip = Api { base_url: None, http: reqwest::Client::builder().user_agent(ua).build().unwrap(), token, roles: Default::default(), guild_perms: Default::default() };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let guilds: Vec<Guild> = gzip.get("/users/@me/guilds").await.unwrap();
            let dms: Vec<Channel> = gzip.get("/users/@me/channels").await.unwrap();
            let g = &guilds[0].id;
            let dm = dms.iter().max_by_key(|c| c.last_id()).unwrap().id.clone();
            let paths = [
                ("guilds", "/users/@me/guilds".to_string()),
                ("dms", "/users/@me/channels".to_string()),
                ("messages(50)", format!("/channels/{dm}/messages?limit=50")),
                ("guild channels", format!("/guilds/{g}/channels")),
            ];
            let stat = |v: &mut Vec<f64>| {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let n = v.len();
                format!("平均 {:6.1} p50 {:6.1} p95 {:6.1} p99 {:6.1}", v.iter().sum::<f64>() / n as f64, v[n / 2], v[(n * 95 / 100).min(n - 1)], v[(n * 99 / 100).min(n - 1)])
            };
            let (mut all_p, mut all_g) = (Vec::new(), Vec::new());
            for (name, path) in &paths {
                let (mut vp, mut vg) = (Vec::new(), Vec::new());
                for i in 0..20 {
                    let order: [(&Api, bool); 2] = if i % 2 == 0 { [(&plain, false), (&gzip, true)] } else { [(&gzip, true), (&plain, false)] };
                    for (api, gz) in order {
                        let t = std::time::Instant::now();
                        let r = api.request(reqwest::Method::GET, path, None).await.unwrap();
                        let _ = r.bytes().await.unwrap();
                        let ms = t.elapsed().as_secs_f64() * 1000.0;
                        if gz { vg.push(ms) } else { vp.push(ms) }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                }
                all_p.extend(vp.iter().copied());
                all_g.extend(vg.iter().copied());
                println!("REST {name:<15} gzipなし {}\n     {:<15} gzipあり {}", stat(&mut vp), "", stat(&mut vg));
            }
            println!("REST 全体 gzipなし n={} {}", all_p.len(), stat(&mut all_p));
            println!("REST 全体 gzipあり n={} {}", all_g.len(), stat(&mut all_g));
        });
    }

    /// 保存済みトークンで identify し、READY までの時間と受信バイト数を測る（オフライン表示で接続）
    async fn ready_once(url: &str, token: &str) -> (f64, usize, usize, f64) {
        let t0 = std::time::Instant::now();
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        let (mut write, mut read) = ws.split();
        let mut inf = Inflater::new();
        let (mut wire, mut json_bytes, mut parse_ms) = (0usize, 0usize, 0.0);
        let mut identified = false;
        loop {
            let m = read.next().await.unwrap().unwrap();
            wire += m.len();
            let tp = std::time::Instant::now();
            let Some(v) = decode_frame(&m, &mut inf).unwrap() else { continue };
            parse_ms += tp.elapsed().as_secs_f64() * 1000.0;
            json_bytes += match &m { WsMsg::Text(t) => t.len(), _ => inf.out.len() };
            if !identified {
                let identify = json!({ "op": 2, "d": { "token": token, "properties": { "os": "Linux", "browser": "Discord Client", "device": "" },
                    "presence": { "status": "invisible", "since": 0, "activities": [], "afk": false }, "compress": false } });
                write.send(WsMsg::Text(identify.to_string().into())).await.unwrap();
                identified = true;
            }
            if v["t"] == "READY" {
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                let _ = write.send(WsMsg::Close(None)).await;
                return (ms, wire, json_bytes, parse_ms);
            }
        }
    }

    /// cargo test --release live_gateway_ready -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gateway_ready() {
        let token = std::fs::read_to_string(dirs::config_dir().unwrap().join("riscord").join("token")).unwrap().trim().to_string();
        let plain_url = "wss://gateway.discord.gg/?v=10&encoding=json";
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            for i in 0..2 {
                for (name, url) in if i == 0 { [("圧縮なし", plain_url), ("zlib-stream", GATEWAY)] } else { [("zlib-stream", GATEWAY), ("圧縮なし", plain_url)] } {
                    let (ms, wire, json_bytes, parse) = ready_once(url, &token).await;
                    println!("READY {name:<11} 接続〜READY {ms:7.1} ms  受信 {:6} KB (JSON {:6} KB)  展開+解析 {parse:.1} ms", wire / 1024, json_bytes / 1024);
                    tokio::time::sleep(Duration::from_secs(6)).await;
                }
            }
        });
    }

    #[test]
    fn role_cache_expires() {
        let api = Api { base_url: None, http: reqwest::Client::new(), token: String::new(), roles: Default::default(), guild_perms: Default::default() };
        let role = Role { id: "1".into(), name: "mod".into(), color: 0xff0000, position: 3, hoist: false, permissions: String::new() };
        assert!(api.cached_roles("g").is_none());
        api.store_roles("g", std::slice::from_ref(&role));
        assert_eq!(api.cached_roles("g").unwrap()[0].name, "mod");
        // 期限切れは使わない
        api.roles.lock().unwrap().get_mut("g").unwrap().0 -= ROLE_CACHE_TTL + Duration::from_secs(1);
        assert!(api.cached_roles("g").is_none());
    }

    #[test]
    fn role_cache_keeps_guilds_separate_and_replaces_entries() {
        let api = Api { base_url: None, http: reqwest::Client::new(), token: String::new(), roles: Default::default(), guild_perms: Default::default() };
        let role = |name: &str| Role { id: "1".into(), name: name.into(), color: 0, position: 0, hoist: false, permissions: String::new() };
        api.store_roles("a", &[role("old")]);
        api.store_roles("b", &[role("other")]);
        api.store_roles("a", &[role("new")]);
        assert_eq!(api.cached_roles("a").unwrap()[0].name, "new");
        assert_eq!(api.cached_roles("b").unwrap()[0].name, "other");
        assert!(api.cached_roles("missing").is_none());
    }

    #[test]
    fn members_chunk_dispatch() {
        let s = sink();
        let v = members_chunk();
        let (t, d) = split_payload(v);
        dispatch(&t, d, &s.tx, &s.v_tx);
        let evs: Vec<Event> = s.rx.try_iter().collect();
        assert!(matches!(&evs[0], Event::GuildMembers(g, m) if g == "42" && m.len() == 1000 && m[999].user.as_ref().unwrap().id == "10999"));
        assert!(matches!(&evs[1], Event::Presences(p) if p.len() == 1000 && p[0].1.activity.as_deref() == Some("プレイ中: game")));
    }

    #[test]
    fn message_update_dispatch() {
        let s = sink();
        let v = json!({ "op": 0, "t": "MESSAGE_UPDATE", "d": { "channel_id": "1", "id": "2", "content": "x", "embeds": [{ "type": "image", "url": "u" }] } });
        let (t, d) = split_payload(v);
        dispatch(&t, d, &s.tx, &s.v_tx);
        match s.rx.try_recv().unwrap() {
            Event::MessageUpdate { channel_id, id, content, embeds } => {
                assert_eq!((channel_id.as_str(), id.as_str(), content.as_deref()), ("1", "2", Some("x")));
                assert_eq!(embeds.unwrap()[0].url.as_deref(), Some("u"));
            }
            _ => panic!(),
        }
        // 配列など想定外の形でも落ちない
        let (t, d) = split_payload(json!([1, 2]));
        assert!(t.is_empty() && d.is_null());
    }

    #[test]
    fn split_payload_matches_previous_behavior() {
        for value in [
            json!({"t": "READY", "d": {"guilds": [1, 2]}}),
            json!({"t": 123, "d": null}),
            json!({"t": "X"}),
            json!({"d": [1, 2]}),
            json!(null),
            json!([1, 2]),
        ] {
            let expected = (value["t"].as_str().unwrap_or("").to_string(), value["d"].clone());
            assert_eq!(split_payload(value), expected);
        }
    }

    /// cargo test --release bench_dispatch -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_dispatch() {
        let s = sink();
        let text = members_chunk().to_string();
        let n = 200;
        let mut samples = Vec::with_capacity(n);
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let start = std::time::Instant::now();
            let v: Value = serde_json::from_str(&text).unwrap();
            let (t, d) = split_payload(v);
            dispatch(&t, d, &s.tx, &s.v_tx);
            s.rx.try_iter().for_each(drop);
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let elapsed = t0.elapsed().as_secs_f64();
        samples.sort_by(f64::total_cmp);
        println!("GUILD_MEMBERS_CHUNK ({} KB) parse+dispatch: mean {:.3} ms, p95 {:.3} ms, p99 {:.3} ms, {:.1} events/s", text.len() / 1024, elapsed * 1000.0 / n as f64, samples[n * 95 / 100 - 1], samples[n * 99 / 100 - 1], n as f64 / elapsed);
    }

    /// 比べるために、dispatch が出すイベントと音声タスクへの転送を文字列にする
    #[test]
    fn member_list_subscribes_by_range_and_parses_updates() {
        let sub = member_subscription("10", "20", &[[0, 99], [100, 199]]);
        assert_eq!(sub["op"], 37, "全員を取る op 8 ではなく、範囲の購読");
        assert_eq!(sub["d"]["subscriptions"]["10"]["channels"]["20"], json!([[0, 99], [100, 199]]));
        let s = sink();
        let d = json!({
            "guild_id": "10", "id": "everyone", "member_count": 3, "online_count": 2,
            "groups": [{ "id": "online", "count": 2 }, { "id": "offline", "count": 1 }],
            "ops": [
                { "op": "SYNC", "range": [0, 99], "items": [
                    { "group": { "id": "online", "count": 2 } },
                    { "member": { "user": { "id": "1", "username": "a" }, "roles": [], "presence": { "user": { "id": "1" }, "status": "online", "activities": [] } } },
                    { "member": { "user": { "id": "2", "username": "b" }, "roles": [] } },
                    { "group": { "id": "offline", "count": 1 } },
                ] },
                { "op": "DELETE", "index": 2 },
                { "op": "INVALIDATE", "range": [100, 199] },
            ]
        });
        dispatch("GUILD_MEMBER_LIST_UPDATE", d, &s.tx, &s.v_tx);
        let events: Vec<Event> = s.rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, Event::GuildMembers(g, m) if g == "10" && m.len() == 2)));
        assert!(events.iter().any(|e| matches!(e, Event::Presences(p) if p.len() == 1)));
        let Some(Event::MemberList { guild_id, list_id, groups, ops }) = events.into_iter().find(|e| matches!(e, Event::MemberList { .. })) else { panic!("一覧の差分") };
        assert_eq!((guild_id.as_str(), list_id.as_str()), ("10", "everyone"));
        assert_eq!(groups, vec![("online".to_string(), 2), ("offline".to_string(), 1)]);
        assert_eq!(ops[0], ListOp::Sync { range: [0, 99], items: vec![
            ListItem::Group("online".into(), 2), ListItem::Member("1".into()), ListItem::Member("2".into()), ListItem::Group("offline".into(), 1),
        ] });
        assert_eq!(ops[1], ListOp::Delete { index: 2 });
        assert_eq!(ops[2], ListOp::Invalidate { range: [100, 199] });
    }

    #[test]
    fn reconnect_backs_off_and_stops_on_fatal_codes() {
        // 1, 2, 4 … 秒と延び、60 秒で止まる（揺らぎは 1 秒未満）
        for (n, secs) in [(0, 1), (1, 2), (2, 4), (5, 32), (6, 60), (20, 60)] {
            let d = reconnect_delay(n).as_millis() as u64;
            assert!(d >= secs * 1000 && d < secs * 1000 + 1000, "{n} 回目: {d}ms");
        }
        assert!(matches!(close_action(4004), GatewayEnd::Fatal(_)), "トークン無効はやめる");
        assert!(matches!(close_action(4014), GatewayEnd::Fatal(_)));
        assert_eq!(close_action(4009), GatewayEnd::Reidentify, "セッション期限切れは一から");
        assert_eq!(close_action(4007), GatewayEnd::Reidentify);
        assert_eq!(close_action(4000), GatewayEnd::Resume, "その他は再開を試す");
        assert_eq!(close_action(1006), GatewayEnd::Resume);
    }

    #[test]
    fn gateway_rejects_zero_heartbeat_interval() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (sock, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                ws.send(WsMsg::Text(r#"{"op":10,"d":{"heartbeat_interval":0}}"#.into())).await.unwrap();
            });
            let s = sink();
            let (_out_tx, mut out) = unbounded_channel();
            let status = Mutex::new(MyPresence { status: "online".into(), activity: true });
            let mut session = GatewaySession::default();
            let mut ready = false;
            let result = gateway_once_at(&base, "tok", &s.tx, &mut out, &status, &s.v_tx, &mut session, &mut ready).await;
            assert!(result.is_err());
            assert!(!ready);
            server.await.unwrap();
        });
    }

    #[test]
    fn gateway_resumes_session_and_handles_invalid_session() {
        // 本物の代わりに手元で WebSocket サーバーを立て、READY → 切断 → RESUME の流れを確かめる
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("ws://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let mut firsts = Vec::new();
                for step in 0..2 {
                    let (sock, _) = listener.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                    ws.send(WsMsg::Text(r#"{"op":10,"d":{"heartbeat_interval":45000}}"#.into())).await.unwrap();
                    let first: Value = match ws.next().await.unwrap().unwrap() { WsMsg::Text(t) => serde_json::from_str(&t).unwrap(), m => panic!("{m:?}") };
                    firsts.push(first);
                    if step == 0 {
                        let ready = json!({ "op": 0, "s": 5, "t": "READY", "d": { "session_id": "sess", "resume_gateway_url": "ws://unused", "guilds": [] } });
                        ws.send(WsMsg::Text(ready.to_string().into())).await.unwrap();
                        // Discord から再接続を頼む
                        ws.send(WsMsg::Text(r#"{"op":7,"d":null}"#.into())).await.unwrap();
                    } else {
                        ws.send(WsMsg::Text(r#"{"op":0,"s":6,"t":"RESUMED","d":{}}"#.into())).await.unwrap();
                        ws.send(WsMsg::Text(r#"{"op":9,"d":false}"#.into())).await.unwrap();
                    }
                    let _ = ws.next().await;
                }
                firsts
            });
            let s = sink();
            let (_out_tx, mut out) = unbounded_channel();
            let status = Mutex::new(MyPresence { status: "online".into(), activity: true });
            let mut session = GatewaySession::default();
            // 1回目: identify して READY を受け、op 7 で「再開」
            let mut ready = false;
            let end = gateway_once_at(&base, "tok", &s.tx, &mut out, &status, &s.v_tx, &mut session, &mut ready).await.unwrap();
            assert_eq!(end, GatewayEnd::Resume);
            assert!(ready);
            assert_eq!((session.id.as_deref(), session.seq), (Some("sess"), Some(5)));
            // 2回目: RESUME を送り、op 9 (false) で「一から」
            let mut ready = false;
            session.resume_url = Some(base.clone());
            let end = gateway_once_at(&base, "tok", &s.tx, &mut out, &status, &s.v_tx, &mut session, &mut ready).await.unwrap();
            assert_eq!(end, GatewayEnd::Reidentify);
            assert!(ready, "RESUMED でつながった扱い");
            let firsts = server.await.unwrap();
            assert_eq!(firsts[0]["op"], 2, "最初は identify");
            assert_eq!(firsts[0]["d"]["presence"]["activities"][0]["name"], "Riscord");
            assert_eq!(firsts[1]["op"], 6, "再接続は RESUME");
            assert_eq!(firsts[1]["d"]["session_id"], "sess");
            assert_eq!(firsts[1]["d"]["seq"], 5);
        });
    }

    #[test]
    fn presence_shows_riscord_activity_only_when_enabled() {
        let on = presence_payload(&MyPresence { status: "idle".into(), activity: true });
        assert_eq!(on["op"], 3);
        assert_eq!(on["d"]["status"], "idle");
        assert_eq!(on["d"]["activities"][0]["name"], "Riscord");
        assert_eq!(on["d"]["activities"][0]["type"], 0, "プレイ中");
        assert!(on["d"]["activities"][0]["timestamps"]["start"].as_u64().unwrap() > 0);
        let again = presence_payload(&MyPresence { status: "online".into(), activity: true });
        assert_eq!(again["d"]["activities"][0]["timestamps"], on["d"]["activities"][0]["timestamps"], "開始時刻は変わらない");
        let off = presence_payload(&MyPresence { status: "online".into(), activity: false });
        assert_eq!(off["d"]["activities"], json!([]));
    }

    #[test]
    fn profile_errors_are_explained() {
        let err = |status: u16, body: &str| anyhow::Error::new(HttpError { status, text: format!("PATCH /users/@me: {status} {body}") });
        assert!(profile_error_text(&err(400, r#"{"captcha_key": ["captcha-required"]}"#)).contains("CAPTCHA"));
        assert!(profile_error_text(&err(400, r#"{"code": 50035, "errors": {"avatar": {"_errors": [{"code": "AVATAR_RATE_LIMIT"}]}}}"#)).contains("アバターの変更が多すぎます"));
        assert!(profile_error_text(&err(400, r#"{"code": 50035, "errors": {"bio": {}}}"#)).contains("受け付けてもらえません"));
        assert!(profile_error_text(&anyhow::anyhow!("timeout")).contains("通信状態"));
    }

    #[test]
    fn compose_rejections_are_explained() {
        let err = |status: u16, body: &str| anyhow::Error::new(HttpError { status, text: format!("POST /channels/1/messages: {status} {body}") });
        assert_eq!(compose_reject_reason(&err(413, "")).as_deref(), Some("ファイルが大きすぎます"));
        assert_eq!(compose_reject_reason(&err(400, r#"{"code": 50035, "errors": {"content": {"_errors": [{"code": "BASE_TYPE_MAX_LENGTH"}]}}}"#)).as_deref(), Some("本文が長すぎます"));
        assert_eq!(compose_reject_reason(&err(403, r#"{"code": 50013}"#)).as_deref(), Some("このチャンネルで送信する権限がありません"));
        assert!(compose_reject_reason(&err(500, "")).is_none(), "サーバー側の失敗は届いたか分からない");
        assert!(compose_reject_reason(&anyhow::anyhow!("timeout")).is_none());
    }

    fn describe(e: &Event) -> String {
        match e {
            Event::Status(s) => format!("Status({s})"),
            Event::VoiceStates(v) => format!("VoiceStates({v:?})"),
            Event::Presences(p) => format!("Presences({p:?})"),
            Event::GuildMembers(g, m) => format!("GuildMembers({g}, {m:?})"),
            Event::Call { channel_id, ringing } => format!("Call({channel_id}, {ringing:?})"),
            Event::CallEnded(c) => format!("CallEnded({c})"),
            Event::MessageCreate(m) => format!("MessageCreate({m:?})"),
            Event::MessageUpdate { channel_id, id, content, embeds } => format!("MessageUpdate({channel_id}, {id}, {content:?}, {embeds:?})"),
            Event::MessageDelete { channel_id, id } => format!("MessageDelete({channel_id}, {id})"),
            Event::AuthFailed => "AuthFailed".into(),
            _ => "other".into(),
        }
    }

    /// 1件のフレーム文字列を、従来（全体を Value にして dispatch）と dispatch_raw の両方で処理した結果
    #[test]
    fn ready_extracts_my_roles_and_channel_guilds() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (v_tx, _v_rx) = unbounded_channel();
        let tx = Tx { tx, ctx: crate::wake::Waker::default() };
        let ready = json!({
            "user": { "id": "me" },
            "guilds": [
                { "id": "g1", "channels": [{ "id": "c1", "name": "x" }], "threads": [{ "id": "t1" }] },
                { "id": "g2", "channels": [], "members": [{ "user": { "id": "me" }, "roles": ["r2"] }] },
            ],
            "merged_members": [[{ "user_id": "me", "roles": ["r1"] }], []],
            "read_state": { "entries": [{ "id": "c1", "mention_count": 1 }] },
        });
        let raw = serde_json::value::RawValue::from_string(ready.to_string()).unwrap();
        dispatch_raw("READY", Some(&raw), &tx, &v_tx);
        let extra = rx.try_iter().find_map(|e| match e { Event::ReadyExtra(v) => Some(v), _ => None }).unwrap();
        assert_eq!(extra["my_roles"], json!({ "g1": ["r1"], "g2": ["r2"] }));
        assert_eq!(extra["channel_guild"], json!({ "c1": "g1", "t1": "g1" }));
        assert_eq!(extra["read_state"]["entries"][0]["mention_count"], 1);
    }

    fn both_paths(text: &str) -> [(Option<i64>, Option<u64>, Vec<String>, Vec<String>); 2] {
        let run = |raw: bool| {
            let (tx, rx) = std::sync::mpsc::channel();
            let (v_tx, mut v_rx) = unbounded_channel();
            let tx = Tx { tx, ctx: crate::wake::Waker::default() };
            let (seq, op) = if raw {
                let f: GatewayFrame = serde_json::from_str(text).unwrap();
                if f.op.as_u64() == Some(0) { dispatch_raw(f.t.as_str().unwrap_or(""), f.d.as_deref(), &tx, &v_tx); }
                (f.s.as_i64(), f.op.as_u64())
            } else {
                let v: Value = serde_json::from_str(text).unwrap();
                let (seq, op) = (v["s"].as_i64(), v["op"].as_u64());
                if op == Some(0) { let (t, d) = split_payload(v); dispatch(&t, d, &tx, &v_tx); }
                (seq, op)
            };
            let events = rx.try_iter().map(|e| describe(&e)).collect();
            let mut voice = Vec::new();
            while let Ok(m) = v_rx.try_recv() {
                voice.push(match m { VoiceMsg::State(v) => format!("State({v})"), VoiceMsg::Server(v) => format!("Server({v})"), _ => "other".into() });
            }
            (seq, op, events, voice)
        };
        [run(false), run(true)]
    }

    /// dispatch_raw は従来の「全体を Value にしてから dispatch」と同じイベントを出す
    #[test]
    fn dispatch_raw_matches_value_dispatch() {
        let vs = json!({ "user_id": "7", "channel_id": "300", "session_id": "x", "self_mute": true, "deaf": false, "member": { "user": { "id": "7", "username": "seven" } } });
        let presence = json!({ "user": { "id": "7" }, "status": "idle", "activities": [{ "type": 4, "state": "休憩中", "emoji": { "name": "☕" } }, { "type": 0, "name": "game" }] });
        let guild = |id: Value, voice: Option<Value>| {
            let mut g = json!({ "id": id, "name": "g", "channels": [{ "id": "1", "name": "general", "type": 0, "permission_overwrites": [] }], "roles": [{ "id": "1", "name": "@everyone" }], "emojis": [], "member_count": 5 });
            if let Some(v) = voice { g["voice_states"] = v; }
            g
        };
        let frames = [
            json!({ "op": 0, "s": 1, "t": "READY", "d": { "v": 10, "user": { "id": "1" }, "guilds": [guild(json!("10"), Some(json!([vs]))), guild(json!("11"), None), guild(Value::Null, Some(json!([vs])))], "presences": [presence], "private_channels": [], "session_id": "s" } }),
            json!({ "op": 0, "s": 2, "t": "READY", "d": { "guilds": [guild(json!("10"), Some(json!([vs])))] } }),
            json!({ "op": 0, "s": 2, "t": "READY", "d": { "presences": [presence] } }),
            // 想定外の形は全体の解析に戻る
            json!({ "op": 0, "s": 3, "t": "READY", "d": { "guilds": null, "presences": [presence] } }),
            json!({ "op": 0, "s": 3, "t": "READY", "d": { "guilds": [null, guild(json!("12"), Some(json!([vs])))], "presences": "x" } }),
            json!({ "op": 0, "s": 3, "t": "READY", "d": { "guilds": [guild(json!(5), Some(json!({ "a": 1 })))] } }),
            json!({ "op": 0, "s": 3, "t": "READY", "d": [1, 2] }),
            json!({ "op": 0, "s": 4, "t": "READY_SUPPLEMENTAL", "d": { "merged_presences": { "friends": [presence], "guilds": [[presence], []] } } }),
            members_chunk(),
            json!({ "op": 0, "s": 5, "t": "PRESENCE_UPDATE", "d": presence }),
            json!({ "op": 0, "s": 6, "t": "VOICE_STATE_UPDATE", "d": vs }),
            json!({ "op": 0, "s": 7, "t": "VOICE_SERVER_UPDATE", "d": { "token": "t", "guild_id": "10", "endpoint": "e" } }),
            json!({ "op": 0, "s": 8, "t": "CALL_CREATE", "d": { "channel_id": "300", "ringing": ["1", "2"], "voice_states": [vs] } }),
            json!({ "op": 0, "s": 9, "t": "CALL_UPDATE", "d": { "channel_id": "300", "ringing": [] } }),
            json!({ "op": 0, "s": 10, "t": "CALL_DELETE", "d": { "channel_id": "300" } }),
            json!({ "op": 0, "s": 11, "t": "MESSAGE_CREATE", "d": { "id": "5", "channel_id": "300", "author": { "id": "7", "username": "seven" }, "content": "やあ <:pog:1>", "timestamp": "2026-11-05T10:00:00+00:00", "attachments": [], "embeds": [], "mentions": [] } }),
            json!({ "op": 0, "s": 12, "t": "MESSAGE_UPDATE", "d": { "channel_id": "300", "id": "5", "content": "x", "embeds": [{ "type": "image", "url": "u" }] } }),
            json!({ "op": 0, "s": 13, "t": "MESSAGE_DELETE", "d": { "channel_id": "300", "id": "5" } }),
            json!({ "op": 0, "s": 14, "t": "MESSAGE_DELETE", "d": null }),
            json!({ "op": 0, "s": 15, "t": "MESSAGE_CREATE" }),
            json!({ "op": 0, "s": 16, "t": "MESSAGE_CREATE", "d": { "id": 5 } }),
            json!({ "op": 0, "s": 17, "t": "TYPING_START", "d": { "channel_id": "300", "user_id": "7" } }),
            json!({ "op": 0, "s": 18, "t": "MESSAGE_REACTION_ADD", "d": { "emoji": { "name": "👍" } } }),
            json!({ "op": 0, "s": 19, "t": null, "d": {} }),
            json!({ "op": 11 }),
            json!({ "op": 10, "d": { "heartbeat_interval": 41250 } }),
            json!({ "op": "0", "s": "20", "t": "MESSAGE_DELETE", "d": { "channel_id": "300", "id": "5" } }),
        ];
        for f in frames {
            let text = f.to_string();
            let [old, new] = both_paths(&text);
            assert_eq!(old, new, "{}", &text[..text.len().min(200)]);
        }
        // 確認用: READY から実際にイベントが出ている
        let [old, _] = both_paths(&json!({ "op": 0, "s": 1, "t": "READY", "d": { "guilds": [guild(json!("10"), Some(json!([vs])))], "presences": [presence] } }).to_string());
        // Status・VoiceStates・Presences と、チャンネル→サーバーの対応（ReadyExtra）
        assert_eq!(old.2.len(), 4, "{:?}", old.2);
    }

    /// 大きい READY（多数のサーバー・チャンネル・ロール・絵文字）と、使わない種類の処理時間
    /// cargo test --release bench_dispatch_raw -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_dispatch_raw() {
        let guilds: Vec<Value> = (0..100).map(|g| json!({
            "id": format!("{g}"), "name": format!("サーバー {g}"), "member_count": 1000,
            "channels": (0..50).map(|c| json!({ "id": format!("{g}{c}"), "name": format!("チャンネル{c}"), "type": c % 3, "position": c, "topic": "説明文".repeat(5), "permission_overwrites": [{ "id": "1", "type": 0, "allow": "0", "deny": "1024" }] })).collect::<Vec<_>>(),
            "roles": (0..30).map(|r| json!({ "id": format!("{g}{r}"), "name": format!("role{r}"), "color": r, "position": r, "permissions": "123456" })).collect::<Vec<_>>(),
            "emojis": (0..20).map(|e| json!({ "id": format!("{g}{e}"), "name": format!("emoji{e}"), "animated": false })).collect::<Vec<_>>(),
            "voice_states": [{ "user_id": format!("{g}"), "channel_id": format!("{g}1"), "self_mute": false }],
        })).collect();
        let ready = json!({ "op": 0, "s": 1, "t": "READY", "d": { "guilds": guilds, "presences": [], "private_channels": [] } }).to_string();
        let typing = json!({ "op": 0, "s": 2, "t": "TYPING_START", "d": { "channel_id": "300", "guild_id": "1", "user_id": "7", "timestamp": 1, "member": { "user": { "id": "7", "username": "seven", "avatar": "0123456789abcdef" }, "roles": ["1", "2"], "nick": null, "joined_at": "2024-01-01T00:00:00+00:00" } } }).to_string();
        let chunk = members_chunk().to_string();
        let s = sink();
        for (name, text, n) in [("READY", &ready, 20), ("TYPING_START", &typing, 20000), ("GUILD_MEMBERS_CHUNK", &chunk, 100)] {
            for raw in [false, true, false, true] {
                let start = std::time::Instant::now();
                for _ in 0..n {
                    if raw {
                        let f: GatewayFrame = serde_json::from_str(text).unwrap();
                        dispatch_raw(f.t.as_str().unwrap_or(""), f.d.as_deref(), &s.tx, &s.v_tx);
                    } else {
                        let v: Value = serde_json::from_str(text).unwrap();
                        let (t, d) = split_payload(v);
                        dispatch(&t, d, &s.tx, &s.v_tx);
                    }
                    s.rx.try_iter().for_each(drop);
                }
                println!("{name:<20} ({:5} KB) {}: {:.4} ms/件", text.len() / 1024, if raw { "dispatch_raw" } else { "Value全体  " }, start.elapsed().as_secs_f64() * 1000.0 / n as f64);
            }
        }
    }

    /// 保存済みトークンで（オフライン表示で）つなぎ、届くイベントの種類・量と、受信処理の時間を
    /// 従来（全体を Value に）と dispatch_raw で比べる
    /// LIVE_SECS=60 cargo test --release live_gateway_event_mix -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_gateway_event_mix() {
        let token = std::fs::read_to_string(dirs::config_dir().unwrap().join("riscord").join("token")).unwrap().trim().to_string();
        let secs: u64 = std::env::var("LIVE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let texts: Vec<String> = rt.block_on(async {
            let (ws, _) = tokio_tungstenite::connect_async(GATEWAY).await.unwrap();
            let (mut write, mut read) = ws.split();
            let mut inf = Inflater::new();
            let mut texts = Vec::new();
            let end = tokio::time::Instant::now() + Duration::from_secs(secs);
            let mut identified = false;
            let mut hb = tokio::time::interval(Duration::from_secs(40));
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(end) => break,
                    _ = hb.tick(), if identified => { write.send(WsMsg::Text(json!({"op": 1, "d": null}).to_string().into())).await.unwrap(); }
                    m = read.next() => {
                        let m = m.unwrap().unwrap();
                        let text = match &m {
                            WsMsg::Text(t) => t.to_string(),
                            WsMsg::Binary(b) => match inf.push(b).unwrap() { Some(t) => t.to_string(), None => continue },
                            _ => continue,
                        };
                        texts.push(text);
                        if !identified {
                            let identify = json!({ "op": 2, "d": { "token": token, "properties": { "os": "Linux", "browser": "Discord Client", "device": "" },
                                "presence": { "status": "invisible", "since": 0, "activities": [], "afk": false }, "compress": false } });
                            write.send(WsMsg::Text(identify.to_string().into())).await.unwrap();
                            identified = true;
                        }
                    }
                }
            }
            let _ = write.send(WsMsg::Close(None)).await;
            texts
        });
        let mut kinds: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
        for t in &texts {
            let v: Value = serde_json::from_str(t).unwrap();
            let k = v["t"].as_str().map(String::from).unwrap_or_else(|| format!("op{}", v["op"]));
            let e = kinds.entry(k).or_default();
            e.0 += 1;
            e.1 += t.len();
        }
        println!("LIVE {secs} 秒で {} 件・{} KB", texts.len(), texts.iter().map(String::len).sum::<usize>() / 1024);
        for (k, (n, b)) in &kinds {
            println!("  {k:<28} {n:6} 件 {:8} KB 使う={}", b / 1024, handles(k));
        }
        let s = sink();
        for raw in [false, true, false, true] {
            let start = std::time::Instant::now();
            for t in &texts {
                if raw {
                    let f: GatewayFrame = serde_json::from_str(t).unwrap();
                    if f.op.as_u64() == Some(0) { dispatch_raw(f.t.as_str().unwrap_or(""), f.d.as_deref(), &s.tx, &s.v_tx); }
                } else {
                    let v: Value = serde_json::from_str(t).unwrap();
                    if v["op"].as_u64() == Some(0) { let (t, d) = split_payload(v); dispatch(&t, d, &s.tx, &s.v_tx); }
                }
                s.rx.try_iter().for_each(drop);
            }
            println!("LIVE 全件の解析+dispatch {}: {:.2} ms", if raw { "dispatch_raw" } else { "Value全体  " }, start.elapsed().as_secs_f64() * 1000.0);
        }
    }
