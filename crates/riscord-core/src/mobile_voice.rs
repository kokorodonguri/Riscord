use std::{sync::{Arc, Weak, atomic::Ordering::Relaxed},time::Duration};
use serde_json::{json,Value};
use tokio::sync::mpsc::UnboundedReceiver;
use crate::{mobile::Client,voice::{VoiceCtl,VoiceSession}};

pub enum Command { Join { guild:Option<String>,channel:String,ring:bool }, Leave, Flags, State(Value), Server(Value) }
struct Session(VoiceSession);
impl Drop for Session { fn drop(&mut self) { self.0.leave(); } }
struct Target { guild:Option<String>,channel:String,ring:bool }
fn payload(target:&Target,channel:Option<&str>,ctl:&VoiceCtl)->Value {
    json!({"op":4,"d":{"guild_id":target.guild,"channel_id":channel,"self_mute":ctl.mute.load(Relaxed),"self_deaf":ctl.deaf.load(Relaxed),"self_video":false}})
}
pub async fn run(client:Weak<Client>,me:String,ctl:Arc<VoiceCtl>,mut rx:UnboundedReceiver<Command>) {
    let mut target:Option<Target>=None;
    let mut session:Option<Session>=None;
    let mut sid:Option<String>=None;
    let mut server:Option<Value>=None;
    let mut tick=tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let command=tokio::select! { command=rx.recv()=>match command {Some(c)=>c,None=>break}, _=tick.tick(), if session.is_some()=>{ if let Some(s)=&mut session {s.0.tick();} continue; } };
        let Some(c)=client.upgrade() else {break};
        match command {
            Command::Join{guild,channel,ring}=>{
                session.take();sid=None;server=None;
                let next=Target{guild,channel,ring};
                let _=c.send_gateway(payload(&next,Some(&next.channel),&ctl));
                c.push(json!({"t":"VOICE","d":{"status":"接続中","channel_id":next.channel}}));
                target=Some(next);
            },
            Command::Leave=>{
                if let Some(t)=target.take() {let _=c.send_gateway(payload(&t,None,&ctl));}
                session.take();sid=None;server=None;
                c.push(json!({"t":"VOICE","d":{"status":"未接続"}}));
            },
            Command::Flags=>if let Some(t)=&target {let _=c.send_gateway(payload(t,Some(&t.channel),&ctl));},
            Command::State(d)=>{
                if d["user_id"].as_str()!=Some(&me) {continue;}
                if d["channel_id"].is_null() {
                    session.take();target=None;sid=None;server=None;
                    c.push(json!({"t":"VOICE","d":{"status":"未接続"}}));
                } else if let Some(t)=&mut target {
                    if let Some(channel)=d["channel_id"].as_str() {t.channel=channel.to_owned();}
                    sid=d["session_id"].as_str().map(str::to_owned);
                }
            },
            Command::Server(d)=>{
                let Some(t)=&target else {continue};
                if t.guild.as_deref()!=d["guild_id"].as_str() {continue;}
                server=Some(d);
            },
        }
        if let (Some(t),Some(sid_value),Some(server_value))=(&mut target,&sid,&server) {
            let (Some(endpoint),Some(token))=(server_value["endpoint"].as_str(),server_value["token"].as_str()) else {continue};
            let mut active=Session(VoiceSession::new(ctl.clone()));
            match active.0.connect(t.guild.as_deref(),&t.channel,endpoint,sid_value,token,&me).await {
                Ok(())=>{
                    c.push(json!({"t":"VOICE","d":{"status":"通話中","channel_id":t.channel}}));
                    if std::mem::take(&mut t.ring) {let _=c.request(reqwest::Method::POST,&format!("/channels/{}/call/ring",t.channel),Some(json!({"recipients":null}))).await;}
                    session=Some(active);
                },
                Err(e)=>{
                    let _=c.send_gateway(payload(t,None,&ctl));
                    c.push(json!({"t":"VOICE","d":{"status":"未接続","error":format!("通話に接続できませんでした: {e}")}}));
                    target=None;session.take();
                }
            }
            server=None;
        }
    }
}
