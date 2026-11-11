package io.github.kokorodonguri.riscord

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import org.json.JSONArray
import org.json.JSONObject

fun JSONArray.objects(): List<JSONObject> = (0 until length()).mapNotNull { optJSONObject(it) }
fun JSONObject.label(): String = optString("global_name").takeIf { it.isNotBlank() && it != "null" }
    ?: optString("name").takeIf { it.isNotBlank() && it != "null" }
    ?: optString("username", "不明")
fun JSONObject.channelLabel(): String = optString("name").takeIf { it.isNotBlank() && it != "null" }
    ?: optJSONArray("recipients")?.objects()?.joinToString(", ") { it.label() }?.takeIf { it.isNotBlank() } ?: "DM"

/** 無料アカウントで送れる添付の上限（Rust 側の UPLOAD_LIMIT と同じ） */
const val UPLOAD_LIMIT=10L*1024*1024
private const val INCOMING_CALL_NOTIFICATION=30

data class PendingAttachment(val uri:android.net.Uri,val name:String,val size:Long)
data class ChatState(
    val lastChannelId:String?=null,
    val previews:Map<String,JSONObject> = emptyMap(),
    val unreadChannels:Set<String> = emptySet(),
    val attachments:Map<String,PendingAttachment> = emptyMap(),
    val editing:JSONObject?=null,val editDraft:String="",
    val profile: JSONObject? = null,
    val me: JSONObject? = null, val guilds: List<JSONObject> = emptyList(),
    val channels: List<JSONObject> = emptyList(), val dms: List<JSONObject> = emptyList(),
    val guild: JSONObject? = null, val channel: JSONObject? = null,
    val messages: List<JSONObject> = emptyList(), val loading: Boolean = false,
    val sending: Boolean = false, val error: String? = null, val connection: String = "未接続",
    val ticket: String? = null, val qr: JSONObject? = null, val loginBusy: Boolean = false,
    val drafts: Map<String, String> = emptyMap(), val reply: JSONObject? = null,
    val incomingCall:JSONObject? = null,
    val voiceStatus:String = "未接続", val voiceChannel:String? = null, val mute:Boolean=false, val deaf:Boolean=false, val speaker:Boolean=false,
    val persistent: Boolean = false,
    val hasMore: Boolean = true, val typing: String? = null,
    /** 保存済みログインで再接続を試みている間 true（ログイン画面ではなく待機画面を出す） */
    val restoring: Boolean = false,
    val status: String = "online",
    /** サーバーID（DM は ""）ごとの user_guild_settings */
    val guildSettings: Map<String,JSONObject> = emptyMap(),
)
class ChatModel(app: Application, restoreSession:Boolean=true,
    private val apiOverride:(suspend (String,String,JSONObject?)->Any?)?=null
) : AndroidViewModel(app) {
    val state = MutableStateFlow(ChatState())
    private val store = TokenStore(app)
    private val settings = app.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE)
    @Volatile private var handle = NativeBridge.create()
    private var historyJob: Job? = null
    private var restoreJob: Job? = null
    private var pauseJob: Job? = null
    private val messageCache=mutableMapOf<String,List<JSONObject>>()
    private var connecting = false
    private var pendingChannel: String? = null
    private var foreground = false
    private var gatewayPaused = false
    private var lastHomeRefresh = 0L
    init {
        state.update { it.copy(status=settings.getString("status","online") ?: "online") }
        if(restoreSession) restoreJob = viewModelScope.launch { restore() }
        viewModelScope.launch {
            while (isActive) {
                try {
                    // イベントが届くまで Rust 側で待つ（届けばすぐ返る）
                    val events = NativeBridge.request(handle, JSONObject().put("op", "poll").put("wait_ms", 25_000)) as? JSONArray
                    val list = events?.objects().orEmpty()
                    list.forEach { event(it) }
                    if (list.isNotEmpty()) delay(50)
                } catch (e: Exception) { if (e is CancellationException) throw e; delay(1000) }
            }
        }
        viewModelScope.launch {
            ProcessLifecycleOwner.get().lifecycle.currentStateFlow.collect { lifecycle ->
                foreground = lifecycle.isAtLeast(Lifecycle.State.STARTED)
                if (foreground) cancelIncomingCallNotification()
                updateGateway()
            }
        }
    }
    private fun task(block: suspend () -> Unit) { viewModelScope.launch {
        try { block() } catch (e: Exception) {
            if (e is CancellationException) throw e
            if (e is AuthExpiredException) { expire(e.message); return@launch }
            state.update { it.copy(error = e.message ?: "通信に失敗しました", loading = false, sending = false, loginBusy = false) }
        }
    } }
    suspend fun rest(path: String, method: String = "GET", body: JSONObject? = null): Any? {
        apiOverride?.let {return it(path,method,body)}
        val input = JSONObject().put("op", "rest").put("path", path).put("method", method)
        if (body != null) input.put("body", body)
        return NativeBridge.request(handle, input)
    }
    fun dismissError() { state.update { it.copy(error = null) } }

    /** 保存済みトークンで接続する。通信エラーなら再試行し、認証切れのときだけ保存を消す */
    private suspend fun restore() {
        val token = try { store.load() } catch (e: Exception) {
            store.clear()
            state.update { it.copy(error = "保存済みログインを読み込めませんでした。ログインし直してください。") }
            return
        } ?: return
        state.update { it.copy(restoring = true, connection = "接続中") }
        var wait = 2_000L
        while (currentCoroutineContext().isActive) {
            try {
                connect(token, false)
                if (state.value.me != null) { state.update { it.copy(restoring = false, error = null) }; return }
            } catch (e: AuthExpiredException) {
                store.clear()
                state.update { it.copy(restoring = false, error = e.message) }
                return
            } catch (e: Exception) {
                if (e is CancellationException) throw e
                state.update { it.copy(connection = "接続できません。${wait / 1000}秒後に再試行します", error = e.message) }
            }
            delay(wait)
            wait = (wait * 2).coerceAtMost(60_000)
        }
    }
    /** 再接続の待機をやめてログイン画面へ（保存したログインは消す） */
    fun cancelRestore() {
        restoreJob?.cancel(); restoreJob = null
        store.clear()
        state.update { it.copy(restoring = false, error = null, connection = "未接続") }
    }
    private fun stopRestore() { restoreJob?.cancel(); restoreJob = null; state.update { it.copy(restoring = false) } }
    fun login(email: String, password: String, code: String) = task {
        stopRestore()
        state.update { it.copy(loginBusy = true, error = null) }
        val ticket = state.value.ticket
        NativeBridge.request(handle, JSONObject().put("op", "login").put("mode", if (ticket == null) "password" else "mfa")
            .put("email", email).put("password", password).put("ticket", ticket).put("code", code))
    }
    fun qrLogin() = task {
        stopRestore()
        state.update { it.copy(loginBusy = true, qr = null, ticket = null, error = null) }
        NativeBridge.request(handle, JSONObject().put("op", "login").put("mode", "qr"))
    }
    fun tokenLogin(token: String) = task { stopRestore(); connect(token.trim(), true) }
    private suspend fun connect(token: String, save: Boolean) {
        if (connecting) return
        connecting = true
        state.update { it.copy(loading = true, loginBusy = true, error = null) }
        try {
            val me = NativeBridge.request(handle, JSONObject().put("op", "connect").put("token", token).put("status", state.value.status)) as JSONObject
            if (save) store.save(token)
            gatewayPaused = false
            state.update { it.copy(me = me, loginBusy = false, ticket = null, qr = null) }
            refreshHome()
            pendingChannel?.let { openFromNotification(it) }
            updateGateway()
        } finally { connecting = false; state.update { it.copy(loading = false, loginBusy = false) } }
    }
    /** トークンが無効になったとき。保存を消してログイン画面へ戻す */
    private fun expire(message: String?) {
        logout()
        state.update { it.copy(error = message ?: "ログインの有効期限が切れました。もう一度ログインしてください。") }
    }
    private suspend fun refreshHome() = coroutineScope {
        val guilds = async { (rest("/users/@me/guilds") as JSONArray).objects() }
        val dms = async { (rest("/users/@me/channels") as JSONArray).objects() }
        val gs = guilds.await(); val ds = dms.await()
        lastHomeRefresh = System.currentTimeMillis()
        state.update { it.copy(guilds = gs, dms = ds) }
    }

    /** 画面を閉じていて常駐も通話もしていないときは、少し待ってから Gateway を切る */
    private fun updateGateway() {
        val s = state.value
        if (s.me == null || apiOverride != null) return
        val active = foreground || s.persistent || s.voiceChannel != null || s.voiceStatus != "未接続"
        if (active) {
            pauseJob?.cancel(); pauseJob = null
            if (gatewayPaused) {
                gatewayPaused = false
                task { NativeBridge.request(handle, JSONObject().put("op", "resume")) }
            }
        } else if (!gatewayPaused && pauseJob == null) {
            pauseJob = viewModelScope.launch {
                delay(60_000)
                pauseJob = null
                runCatching { NativeBridge.request(handle, JSONObject().put("op", "pause")) }
                gatewayPaused = true
                state.update { it.copy(connection = "一時停止中") }
            }
        }
    }

    private fun event(v: JSONObject) {
        val d = v.optJSONObject("d") ?: JSONObject()
        when (v.optString("t")) {
            "CALL_CREATE", "CALL_UPDATE" -> {
                val ringing=d.optJSONArray("ringing")
                val me=state.value.me?.optString("id")
                if(ringing!=null && (0 until ringing.length()).any {ringing.optString(it)==me}) {
                    val channel=state.value.dms.firstOrNull {it.optString("id")==d.optString("channel_id")}
                        ?: JSONObject().put("id",d.optString("channel_id")).put("type",1).put("name","DM通話")
                    state.update {it.copy(incomingCall=channel)}
                    if(!foreground) notifyIncomingCall(channel)
                } else { state.update {it.copy(incomingCall=null)}; cancelIncomingCallNotification() }
            }
            "CALL_DELETE" -> { state.update {it.copy(incomingCall=null)}; cancelIncomingCallNotification() }
            "VOICE" -> {
                val status=d.optString("status")
                state.update { it.copy(voiceStatus=status,voiceChannel=d.optString("channel_id").takeIf {id->id.isNotEmpty()},
                    speaker=if(status=="未接続") false else it.speaker,
                    error=d.optString("error").takeIf {error->error.isNotEmpty()} ?: it.error) }
                if(status=="未接続") getApplication<Application>().stopService(android.content.Intent(getApplication(),CallService::class.java))
                updateGateway()
            }
            "LOGIN_TOKEN" -> task { connect(d.getString("token"), true) }
            "LOGIN_MFA" -> state.update { it.copy(ticket = d.getString("ticket"), loginBusy = false) }
            "LOGIN_QR" -> state.update { it.copy(qr = d.optJSONObject("modules"), loginBusy = false) }
            "LOGIN_SCANNED" -> state.update { it.copy(connection = "${d.optString("name")} の承認待ち") }
            "ERROR" -> state.update { it.copy(error = d.optString("message"), loginBusy = false) }
            "AUTH_EXPIRED" -> if (state.value.me != null) expire(null)
            "CONNECTION" -> state.update { it.copy(connection = d.optString("status")) }
            "READY" -> {
                val entries=d.optJSONArray("guild_settings")?.objects().orEmpty()
                state.update { it.copy(guildSettings=entries.associateBy { e -> e.nonNull("guild_id") ?: "" }) }
                // 接続直後は connect() で取得したばかりなので取り直さない
                if (System.currentTimeMillis() - lastHomeRefresh > 10_000) resync()
            }
            "RESUMED", "RESYNC" -> resync()
            "USER_GUILD_SETTINGS_UPDATE" -> state.update { it.copy(guildSettings=it.guildSettings+((d.nonNull("guild_id") ?: "") to d)) }
            "CHANNEL_CREATE", "CHANNEL_UPDATE", "CHANNEL_DELETE" -> {
                val guildId=d.nonNull("guild_id")
                if (guildId == null) task { refreshHome() }
                else state.value.guild?.takeIf { it.optString("id")==guildId }?.let { guild -> task { loadChannels(guild) } }
            }
            "MESSAGE_CREATE" -> {
                val channelId=d.optString("channel_id")
                state.update { old -> old.copy(
                    previews=old.previews+(channelId to d),
                    dms=old.dms.map {if(it.optString("id")==channelId) JSONObject(it.toString()).put("last_message_id",d.optString("id")) else it},
                    unreadChannels=if(old.channel?.optString("id")!=channelId && d.optJSONObject("author")?.optString("id")!=old.me?.optString("id")) old.unreadChannels+channelId else old.unreadChannels
                ) }
                notifyMessage(d)
                if (channelId == state.value.channel?.optString("id")) {
                    state.update { old -> old.copy(messages = (old.messages.filterNot { it.optString("id") == d.optString("id") } + d).sortedBy { it.optString("id").padStart(20, '0') }, typing = null) }
                }
            }
            "MESSAGE_UPDATE" -> state.update { old -> old.copy(messages = old.messages.map { msg ->
                if (msg.optString("id") != d.optString("id")) msg else JSONObject(msg.toString()).apply { d.keys().forEach { key -> put(key, d.get(key)) } }
            }) }
            "MESSAGE_DELETE" -> state.update { old -> old.copy(messages = old.messages.filterNot { it.optString("id") == d.optString("id") && it.optString("channel_id") == d.optString("channel_id") }) }
            "MESSAGE_REACTION_ADD", "MESSAGE_REACTION_REMOVE" -> {
                val added=v.optString("t")=="MESSAGE_REACTION_ADD"
                val mine=d.optString("user_id")==state.value.me?.optString("id")
                state.update { old -> old.copy(messages = old.messages.map { msg ->
                    if (msg.optString("id") != d.optString("message_id")) msg else applyReaction(msg, d.optJSONObject("emoji") ?: JSONObject(), added, mine)
                }) }
            }
            "TYPING_START" -> if (d.optString("channel_id") == state.value.channel?.optString("id") && d.optString("user_id") != state.value.me?.optString("id")) {
                state.update { it.copy(typing = "入力中…") }; viewModelScope.launch { delay(8000); state.update { it.copy(typing = null) } }
            }
        }
    }
    private fun resync() = task {
        if (state.value.me != null) {
            refreshHome()
            state.value.channel?.let { refreshMessages(it.getString("id")) }
        }
    }
    fun selectGuild(guild: JSONObject?) {
        cacheMessages()
        historyJob?.cancel()
        state.update { it.copy(guild = guild, channel = null, channels = emptyList(), messages = emptyList(), reply = null) }
        if (guild != null) task {
            state.update { it.copy(loading = true) }
            loadChannels(guild)
        }
    }
    private suspend fun loadChannels(guild: JSONObject) {
        val channels = (rest("/guilds/${guild.getString("id")}/channels") as JSONArray).objects().sortedBy { it.optInt("position") }
        if (state.value.guild?.optString("id") == guild.optString("id")) state.update { it.copy(channels = channels, loading = false) }
    }
    private fun cacheMessages() {state.value.channel?.optString("id")?.let {messageCache[it]=state.value.messages}}
    fun openChannel(channel: JSONObject) {
        cacheMessages()
        historyJob?.cancel()
        val id=channel.getString("id")
        state.update { it.copy(channel = channel, lastChannelId=id, messages = messageCache[id].orEmpty(), loading = true, hasMore = true, reply = null, editing=null,editDraft="",typing = null, unreadChannels=it.unreadChannels-id) }
        historyJob = viewModelScope.launch { try { refreshMessages(channel.getString("id")) } catch(e: Exception) {
            if(e is CancellationException) throw e
            if(e is AuthExpiredException) { expire(e.message); return@launch }
            state.update { it.copy(error = e.message, loading = false) }
        } }
    }
    private suspend fun refreshMessages(id: String, before: String? = null) {
        val list = (rest("/channels/$id/messages?limit=50" + (before?.let { "&before=$it" } ?: "")) as JSONArray).objects()
        if (state.value.channel?.optString("id") == id) state.update { old ->
            val fetchedMax = list.maxOfOrNull { it.optString("id").padStart(20, '0') }
            val fetchedMin = list.minOfOrNull { it.optString("id").padStart(20, '0') }
            val retained = if(before != null) old.messages else old.messages.filter {
                val key=it.optString("id").padStart(20,'0')
                fetchedMin!=null && fetchedMax!=null && (key<fetchedMin || key>fetchedMax)
            }
            val merged = (list + retained).distinctBy { it.optString("id") }.sortedBy { it.optString("id").padStart(20, '0') }
            old.copy(messages = merged, loading = false, hasMore = list.size == 50)
        }
    }
    fun older() = task {
        val s=state.value; val id=s.channel?.optString("id") ?: return@task
        if (s.loading || !s.hasMore) return@task
        state.update { it.copy(loading = true) }; refreshMessages(id,s.messages.firstOrNull()?.optString("id"))
    }
    fun back() { cacheMessages(); historyJob?.cancel(); state.update { it.copy(channel = null, reply = null, editing=null,editDraft="",loading = false) } }
    fun draft(value: String) { val id=state.value.channel?.optString("id") ?: return; state.update { it.copy(drafts=it.drafts+(id to value)) } }
    fun reply(message: JSONObject?) { state.update { it.copy(reply = message) } }
    fun send() {
        val s=state.value
        if(s.sending) return
        if(s.editing!=null) {submitEdit();return}
        s.channel?.optString("id")?.let {id->s.attachments[id]?.let {upload(it.uri);return}}
        sendText()
    }
    private fun sendText() = task {
        val s=state.value; val id=s.channel?.optString("id") ?: return@task
        val text=s.drafts[id].orEmpty(); if(text.isBlank() || s.sending) return@task
        state.update { it.copy(sending = true) }
        val body=JSONObject().put("content",text).put("nonce",System.currentTimeMillis().toString()).put("enforce_nonce",true)
        s.reply?.let { body.put("message_reference", JSONObject().put("message_id",it.getString("id")).put("channel_id",id)) }
        val message=rest("/channels/$id/messages","POST",body) as JSONObject
        state.update { old -> old.copy(sending=false, drafts=if(old.drafts[id]==text) old.drafts+(id to "") else old.drafts,
            reply=if(old.channel?.optString("id")==id) null else old.reply,
            messages=if(old.channel?.optString("id")==id) (old.messages.filterNot { it.optString("id")==message.optString("id") }+message).sortedBy { it.optString("id").padStart(20,'0') } else old.messages) }
    }
    fun stageAttachment(uri:android.net.Uri) = task {
        val id=state.value.channel?.optString("id") ?: return@task
        val attachment=withContext(Dispatchers.IO) {
            var name="添付ファイル"; var size=-1L
            getApplication<Application>().contentResolver.query(uri,arrayOf(android.provider.OpenableColumns.DISPLAY_NAME,android.provider.OpenableColumns.SIZE),null,null,null)?.use {cursor->
                if(cursor.moveToFirst()) {name=cursor.getString(0) ?: name; if(!cursor.isNull(1)) size=cursor.getLong(1)}
            }
            require(size<=UPLOAD_LIMIT) {"添付は10MBまでです"}
            PendingAttachment(uri,name,size)
        }
        state.update {it.copy(attachments=it.attachments+(id to attachment))}
    }
    fun removeAttachment() {val id=state.value.channel?.optString("id") ?: return;state.update {it.copy(attachments=it.attachments-id)}}
    fun beginEdit(message:JSONObject) {state.update {it.copy(editing=message,editDraft=message.optString("content"),reply=null)}}
    fun cancelEdit() {state.update {it.copy(editing=null,editDraft="")}}
    fun editDraft(text:String) {state.update {it.copy(editDraft=text)}}
    private fun submitEdit() = task {
        val s=state.value;val message=s.editing ?: return@task
        if(s.editDraft.isBlank()||s.sending) return@task
        state.update {it.copy(sending=true)}
        val updated=rest("/channels/${message.getString("channel_id")}/messages/${message.getString("id")}","PATCH",JSONObject().put("content",s.editDraft)) as JSONObject
        state.update {old->old.copy(sending=false,editing=null,editDraft="",messages=old.messages.map {if(it.optString("id")==updated.optString("id")) updated else it})}
    }
    /** ファイルの中身は Kotlin 側で読まず、fd の所有権ごと Rust へ渡す */
    fun upload(uri: android.net.Uri) = task {
        val snapshot=state.value; val channel=snapshot.channel?.optString("id") ?: return@task
        if(snapshot.sending) return@task
        state.update { it.copy(sending=true) }
        val content=snapshot.drafts[channel].orEmpty()
        val resolver=getApplication<Application>().contentResolver
        val (filename,fd)=withContext(Dispatchers.IO) {
            val filename=snapshot.attachments[channel]?.name ?: resolver.query(uri,arrayOf(android.provider.OpenableColumns.DISPLAY_NAME),null,null,null)?.use { cursor ->
                if(cursor.moveToFirst()) cursor.getString(0) else null
            } ?: "attachment"
            val descriptor=resolver.openFileDescriptor(uri,"r") ?: error("ファイルを開けませんでした")
            filename to descriptor.detachFd()
        }
        val message=NativeBridge.request(handle,JSONObject().put("op","upload").put("channel",channel).put("filename",filename).put("fd",fd).put("content",content)) as JSONObject
        state.update { old -> old.copy(sending=false,attachments=old.attachments-channel,drafts=if(old.drafts[channel]==content) old.drafts+(channel to "") else old.drafts,
            messages=if(old.channel?.optString("id")==channel) (old.messages.filterNot { it.optString("id")==message.optString("id") }+message).sortedBy {it.optString("id").padStart(20,'0')} else old.messages) }
    }
    fun edit(message: JSONObject, text: String) = task {
        val id=message.getString("channel_id")
        val updated=rest("/channels/$id/messages/${message.getString("id")}","PATCH",JSONObject().put("content",text)) as JSONObject
        state.update { s -> s.copy(messages=s.messages.map { if(it.optString("id")==updated.optString("id")) updated else it }) }
    }
    fun delete(message: JSONObject) = task {
        rest("/channels/${message.getString("channel_id")}/messages/${message.getString("id")}","DELETE")
        state.update { s -> s.copy(messages=s.messages.filterNot { it.optString("id")==message.optString("id") }) }
    }
    fun joinVoice(channel:JSONObject,guild:String?,ring:Boolean=true) = task {
        val app=getApplication<Application>()
        require(app.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO)==android.content.pm.PackageManager.PERMISSION_GRANTED) { "マイクの許可が必要です" }
        val shouldRing=ring && state.value.incomingCall?.optString("id")!=channel.optString("id")
        state.update {it.copy(incomingCall=null)}
        cancelIncomingCallNotification()
        app.startForegroundService(android.content.Intent(app,CallService::class.java))
        try {
            NativeBridge.request(handle,JSONObject().put("op","voice_join").put("channel",channel.getString("id")).put("guild",guild).put("ring",guild==null&&shouldRing))
        } catch(e:Exception) {app.stopService(android.content.Intent(app,CallService::class.java));throw e}
    }
    fun dismissCall() = task {
        val channel=state.value.incomingCall
        state.update {it.copy(incomingCall=null)}
        cancelIncomingCallNotification()
        channel?.let {rest("/channels/${it.getString("id")}/call/stop-ringing","POST",JSONObject())}
    }
    fun leaveVoice() = task {
        try { NativeBridge.request(handle,JSONObject().put("op","voice_leave")) }
        finally {
            getApplication<Application>().stopService(android.content.Intent(getApplication(),CallService::class.java))
            state.update {it.copy(voiceStatus="未接続",voiceChannel=null,speaker=false)}
            updateGateway()
        }
    }
    @Suppress("DEPRECATION")
    fun speaker(enabled:Boolean) {
        val audio=getApplication<Application>().getSystemService(android.media.AudioManager::class.java)
        if(android.os.Build.VERSION.SDK_INT>=31) {
            if(enabled) {
                audio.availableCommunicationDevices.firstOrNull {it.type==android.media.AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}?.let {audio.setCommunicationDevice(it)}
            } else audio.clearCommunicationDevice()
        } else { audio.isSpeakerphoneOn=enabled }
        state.update {it.copy(speaker=enabled)}
    }
    fun voiceFlags(mute:Boolean,deaf:Boolean) = task {
        NativeBridge.request(handle,JSONObject().put("op","voice_flags").put("mute",mute).put("deaf",deaf))
        state.update {it.copy(mute=mute,deaf=deaf)}
    }
    fun loadMyProfile() = task {
        state.update { it.copy(profile=null) }
        val id=state.value.me?.optString("id") ?: return@task
        val profile=rest("/users/$id/profile") as JSONObject
        state.update { it.copy(profile=profile.optJSONObject("user_profile") ?: JSONObject()) }
    }
    /** 変わった項目だけ送る */
    fun saveProfile(name: String, bio: String, status: String) = task {
        val s=state.value
        if(name!=s.me?.label()) {
            val me=rest("/users/@me","PATCH",JSONObject().put("global_name",name)) as JSONObject
            state.update { it.copy(me=me) }
        }
        if(bio!=s.profile?.optString("bio").orEmpty()) rest("/users/@me/profile","PATCH",JSONObject().put("bio",bio))
        if(status!=s.status) {
            NativeBridge.request(handle,JSONObject().put("op","presence").put("status",status))
            settings.edit().putString("status",status).apply()
            state.update { it.copy(status=status) }
        }
    }
    fun openFromNotification(id: String) {
        if(id.isEmpty() || id.length>24 || !id.all { it.isDigit() }) return
        pendingChannel=id
        if(state.value.me==null) return
        task {
            val channel=rest("/channels/$id") as JSONObject
            val guild=state.value.guilds.firstOrNull { it.optString("id")==channel.optString("guild_id") }
            selectGuild(guild); openChannel(channel); pendingChannel=null
        }
    }
    fun setPersistent(enabled: Boolean) {
        val app=getApplication<Application>()
        try {
            val intent=android.content.Intent(app,ConnectionService::class.java)
            if(enabled) app.startForegroundService(intent) else app.stopService(intent)
            state.update { it.copy(persistent=enabled) }
            updateGateway()
        } catch(e:Exception) { state.update { it.copy(error="常駐を開始できませんでした: ${e.message}") } }
    }
    private fun canNotify(app:Application)=android.os.Build.VERSION.SDK_INT<33 || app.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)==android.content.pm.PackageManager.PERMISSION_GRANTED
    private fun notifyMessage(message: JSONObject) {
        val s=state.value
        if(!s.persistent || message.optJSONObject("author")?.optString("id")==s.me?.optString("id")) return
        val channelId=message.optString("channel_id")
        if(foreground && s.channel?.optString("id")==channelId) return
        if(!shouldNotify(message,s)) return
        val app=getApplication<Application>()
        if(!canNotify(app)) return
        val intent=android.content.Intent(app,MainActivity::class.java).putExtra("channel_id",channelId).addFlags(android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val open=android.app.PendingIntent.getActivity(app,channelId.hashCode(),intent,android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE)
        val notification=android.app.Notification.Builder(app,"messages").setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(message.optJSONObject("author")?.label() ?: "メッセージ")
            .setContentText(message.optString("content").ifBlank { "添付ファイル" }).setContentIntent(open).setAutoCancel(true).build()
        app.getSystemService(android.app.NotificationManager::class.java).notify(channelId.hashCode(),notification)
    }
    private fun notifyIncomingCall(channel:JSONObject) {
        val app=getApplication<Application>()
        if(!canNotify(app)) return
        val open=android.app.PendingIntent.getActivity(app,INCOMING_CALL_NOTIFICATION,android.content.Intent(app,MainActivity::class.java).addFlags(android.content.Intent.FLAG_ACTIVITY_SINGLE_TOP),android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE)
        val notification=android.app.Notification.Builder(app,"incoming_calls").setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("着信").setContentText(channel.channelLabel()).setCategory(android.app.Notification.CATEGORY_CALL)
            .setContentIntent(open).setAutoCancel(true).setTimeoutAfter(60_000).build()
        app.getSystemService(android.app.NotificationManager::class.java).notify(INCOMING_CALL_NOTIFICATION,notification)
    }
    private fun cancelIncomingCallNotification() {
        getApplication<Application>().getSystemService(android.app.NotificationManager::class.java).cancel(INCOMING_CALL_NOTIFICATION)
    }
    fun createDm(userId: String) = task {
        require(userId.isNotEmpty() && userId.all { it.isDigit() }) { "ユーザーIDを入力してください" }
        val channel=rest("/users/@me/channels","POST",JSONObject().put("recipient_id",userId)) as JSONObject
        refreshHome(); selectGuild(null); openChannel(channel)
    }
    /** 反映は Gateway の MESSAGE_REACTION_ADD / REMOVE で行う */
    fun react(message: JSONObject, emoji: String) = task {
        rest(reactionPath(message,emoji),"PUT",JSONObject())
    }
    /** 付いているリアクションを押したとき。自分が付けていれば外し、なければ付ける */
    fun toggleReaction(message: JSONObject, reaction: JSONObject) = task {
        val emoji=reaction.optJSONObject("emoji") ?: return@task
        val key=emoji.nonNull("id")?.let { "${emoji.optString("name")}:$it" } ?: emoji.optString("name")
        rest(reactionPath(message,key),if(reaction.optBoolean("me")) "DELETE" else "PUT",if(reaction.optBoolean("me")) null else JSONObject())
    }
    private fun reactionPath(message:JSONObject,emoji:String):String {
        val encoded=java.net.URLEncoder.encode(emoji,"UTF-8").replace("+","%20")
        return "/channels/${message.getString("channel_id")}/messages/${message.getString("id")}/reactions/$encoded/@me"
    }
    fun logout() {
        restoreJob?.cancel(); restoreJob=null; pauseJob?.cancel(); pauseJob=null; gatewayPaused=false
        val app=getApplication<Application>()
        app.stopService(android.content.Intent(app,CallService::class.java)); setPersistent(false); historyJob?.cancel(); messageCache.clear(); store.clear()
        app.getSystemService(android.app.NotificationManager::class.java).cancelAll()
        val old=handle; handle=NativeBridge.create(); NativeBridge.destroy(old)
        state.value=ChatState(status=state.value.status)
    }
    override fun onCleared() { NativeBridge.destroy(handle) }
}

/** ミュート中か（期限つきのミュートは期限を過ぎていれば解けている） */
private fun JSONObject.mutedNow():Boolean {
    if(!optBoolean("muted")) return false
    val end=optJSONObject("mute_config")?.nonNull("end_time") ?: return true
    return runCatching { java.time.OffsetDateTime.parse(end).toInstant().isAfter(java.time.Instant.now()) }.getOrDefault(true)
}
/** user_guild_settings に従って通知するか。通知は DM と自分宛てのメンションに限る */
internal fun shouldNotify(message:JSONObject,s:ChatState):Boolean {
    val me=s.me?.optString("id") ?: return false
    val channelId=message.optString("channel_id")
    val guildId=message.nonNull("guild_id")
    val dm=guildId==null && s.dms.any { it.optString("id")==channelId }
    val guild=s.guildSettings[guildId ?: ""]
    val override=guild?.optJSONArray("channel_overrides")?.objects()?.firstOrNull { it.optString("channel_id")==channelId }
    if(override?.mutedNow()==true) return false
    if(dm) return true
    if(guild?.mutedNow()==true) return false
    // 0=すべて 1=メンションのみ 2=なし 3=上位の設定に従う
    val level=override?.optInt("message_notifications",3)?.takeIf { it!=3 } ?: guild?.optInt("message_notifications",3) ?: 3
    if(level==2) return false
    if(message.optJSONArray("mentions")?.objects()?.any { it.optString("id")==me }==true) return true
    return message.optBoolean("mention_everyone") && guild!=null && !guild.optBoolean("suppress_everyone")
}
/** リアクションの増減をメッセージへ反映した新しい JSON を返す */
internal fun applyReaction(message:JSONObject,emoji:JSONObject,added:Boolean,mine:Boolean):JSONObject {
    val copy=JSONObject(message.toString())
    val list=copy.optJSONArray("reactions")?.objects().orEmpty().toMutableList()
    fun same(r:JSONObject):Boolean { val e=r.optJSONObject("emoji") ?: return false; val id=emoji.nonNull("id"); return if(id!=null) e.nonNull("id")==id else e.optString("name")==emoji.optString("name") }
    val index=list.indexOfFirst(::same)
    if(added) {
        if(index<0) list+=JSONObject().put("emoji",emoji).put("count",1).put("me",mine)
        else list[index]=JSONObject(list[index].toString()).put("count",list[index].optInt("count")+1).put("me",list[index].optBoolean("me")||mine)
    } else if(index>=0) {
        val count=list[index].optInt("count")-1
        if(count<=0) list.removeAt(index) else list[index]=JSONObject(list[index].toString()).put("count",count).put("me",list[index].optBoolean("me")&&!mine)
    }
    copy.put("reactions",JSONArray(list))
    return copy
}
