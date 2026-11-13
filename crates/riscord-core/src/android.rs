//! Small JSON JNI boundary. No Java references cross Rust worker threads.
use crate::mobile::Client;
use jni::{
    objects::{JObject, JString},
    sys::{jlong, jstring},
    JNIEnv,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicI64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};
static CLIENTS: OnceLock<Mutex<HashMap<i64, Arc<Client>>>> = OnceLock::new();
static NEXT: AtomicI64 = AtomicI64::new(1);
fn clients() -> &'static Mutex<HashMap<i64, Arc<Client>>> {
    CLIENTS.get_or_init(Default::default)
}
fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    // 通信の待ちが中心なので、コア数ぶん（8 コア機なら 8 本）は要らない。スレッドとメモリを抑える
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(std::thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)))
            .thread_name("riscord-net")
            .enable_all()
            .build()
            .expect("runtime")
    })
}
#[no_mangle]
pub extern "system" fn Java_io_github_kokorodonguri_riscord_NativeBridge_create(
    _env: JNIEnv,
    _obj: JObject,
) -> jlong {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    clients().lock().unwrap().insert(id, Client::new());
    id
}
#[no_mangle]
pub extern "system" fn Java_io_github_kokorodonguri_riscord_NativeBridge_destroy(
    _env: JNIEnv,
    _obj: JObject,
    id: jlong,
) {
    if let Some(c) = clients().lock().unwrap().remove(&id) {
        c.stop();
    }
}
#[no_mangle]
pub extern "system" fn Java_io_github_kokorodonguri_riscord_NativeBridge_call(
    mut env: JNIEnv,
    _obj: JObject,
    id: jlong,
    input: JString,
) -> jstring {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> anyhow::Result<Value> {
            let input: String = env.get_string(&input)?.into();
            let v: Value = serde_json::from_str(&input)?;
            // Kotlin が所有する fd を複製し、送信中のキャンセルや例外でも双方を閉じる。
            #[cfg(unix)]
            let upload_file = match (v["op"].as_str(), v["fd"].as_i64()) {
                (Some("upload"), Some(fd)) if (0..=i32::MAX as i64).contains(&fd) => {
                    Some(duplicate_file(fd as i32)?)
                }
                _ => None,
            };
            let client = clients()
                .lock()
                .unwrap()
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("セッションは終了しました"))?;
            runtime().block_on(async {
            match v["op"].as_str().unwrap_or_default() {
                #[cfg(target_os = "android")]
                "audio_devices" => Ok(json!({"input":crate::voice::input_devices(),"output":crate::voice::output_devices()})),
                #[cfg(target_os = "android")]
                "voice_join" => client.voice(crate::mobile_voice::Command::Join {guild:v["guild"].as_str().map(str::to_owned),channel:v["channel"].as_str().unwrap_or_default().into(),ring:v["ring"].as_bool().unwrap_or(false)}),
                #[cfg(target_os = "android")]
                "voice_leave" => client.voice(crate::mobile_voice::Command::Leave),
                #[cfg(target_os = "android")]
                "voice_flags" => {
                    client.voice_ctl.mute.store(v["mute"].as_bool().unwrap_or(false),Ordering::Relaxed);
                    client.voice_ctl.deaf.store(v["deaf"].as_bool().unwrap_or(false),Ordering::Relaxed);
                    client.voice(crate::mobile_voice::Command::Flags)
                },
                "subscribe_typing" => client.subscribe_typing(v["guild"].as_str().unwrap_or_default(),v["channel"].as_str().unwrap_or_default()),
                "presence" => client.presence(v["status"].as_str().unwrap_or("online")),
                "poll" => Ok(client.wait_events(std::time::Duration::from_millis(v["wait_ms"].as_u64().unwrap_or(0).min(30_000))).await),
                "pause" => { client.pause(); Ok(Value::Null) },
                "resume" => client.resume(),
                "connect" => client.connect(v["token"].as_str().unwrap_or_default().into(),v["status"].as_str()).await,
                "login" => { client.login(v["mode"].as_str().unwrap_or("password"),&v); Ok(Value::Null) },
                #[cfg(unix)]
                "upload" => {
                    let file=upload_file.ok_or_else(||anyhow::anyhow!("ファイルを開けませんでした"))?;
                    client.upload(v["channel"].as_str().unwrap_or_default(),v["filename"].as_str().unwrap_or("attachment"),file,v["content"].as_str().unwrap_or_default(),v["reply_to"].as_str()).await
                },
                "rest" => client.request(v["method"].as_str().unwrap_or("GET").parse()?,v["path"].as_str().unwrap_or_default(),v.get("body").cloned()).await,
                _ => anyhow::bail!("不明な操作"),
            }
        })
        },
    ));
    let output = match result {
        Ok(Ok(v)) => json!({"ok":true,"data":v}),
        Ok(Err(e)) => {
            json!({"ok":false,"error":e.to_string(),"auth":e.is::<crate::mobile::AuthExpired>()})
        }
        Err(_) => json!({"ok":false,"error":"内部エラーが発生しました"}),
    };
    env.new_string(output.to_string())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(unix)]
fn duplicate_file(fd: i32) -> anyhow::Result<std::fs::File> {
    use std::os::fd::FromRawFd;
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { std::fs::File::from_raw_fd(duplicate) })
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::fd::AsRawFd;
    #[test]
    fn duplicate_does_not_take_ownership_of_the_original() {
        let original = std::fs::File::open("/dev/null").unwrap();
        let duplicate = super::duplicate_file(original.as_raw_fd()).unwrap();
        drop(duplicate);
        assert!(original.metadata().is_ok());
        assert!(super::duplicate_file(-1).is_err());
    }
}

/// 端末の言語に合わせる（コアから画面に出す文言の言語）。"ja" / "en" / "zh" 以外は英語
#[no_mangle]
pub extern "system" fn Java_io_github_kokorodonguri_riscord_NativeBridge_setLanguage(
    mut env: JNIEnv,
    _obj: JObject,
    code: JString,
) {
    let code: String = env.get_string(&code).map(Into::into).unwrap_or_default();
    crate::i18n::set_setting(crate::i18n::parse_setting(&code).unwrap_or(crate::i18n::EN));
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_github_kokorodonguri_riscord_NativeBridge_initialize(
    env: JNIEnv,
    _obj: JObject,
    context: JObject,
) {
    static CONTEXT: OnceLock<jni::objects::GlobalRef> = OnceLock::new();
    if CONTEXT.get().is_none() {
        if let (Ok(vm), Ok(global)) = (env.get_java_vm(), env.new_global_ref(context)) {
            if let Ok(()) = CONTEXT.set(global) {
                unsafe {
                    ndk_context::initialize_android_context(
                        vm.get_java_vm_pointer().cast(),
                        CONTEXT.get().unwrap().as_obj().as_raw().cast(),
                    );
                }
            }
        }
    }
}
