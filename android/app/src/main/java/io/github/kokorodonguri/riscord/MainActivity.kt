package io.github.kokorodonguri.riscord

import android.os.Bundle
import android.content.Intent
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveableStateHolder
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.*
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import org.json.JSONObject

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState:Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(statusBarStyle=SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),navigationBarStyle=SystemBarStyle.dark(android.graphics.Color.TRANSPARENT))
        intent.getStringExtra("channel_id")?.let {(application as RiscordApplication).chatModel.openFromNotification(it)}
        setContent {RiscordTheme {Riscord((application as RiscordApplication).chatModel)}}
    }
    override fun onNewIntent(intent:Intent) {
        super.onNewIntent(intent);setIntent(intent)
        intent.getStringExtra("channel_id")?.let {(application as RiscordApplication).chatModel.openFromNotification(it)}
    }
}
@Composable internal fun RiscordTheme(content:@Composable ()->Unit) {
    MaterialTheme(colorScheme=darkColorScheme(primary=MobileAccent,onPrimary=Color.White,primaryContainer=Color(0xff282c50),background=MobileBackground,surface=MobilePanel,surfaceVariant=MobileRaised,onSurface=Color(0xfff1f1f4),onSurfaceVariant=MobileMuted),content=content)
}
@OptIn(ExperimentalMaterial3Api::class)
@Composable fun Riscord(model:ChatModel) {
    val s by model.state.collectAsStateWithLifecycle()
    val context=LocalContext.current
    val screens=rememberSaveableStateHolder()
    var attachmentChannel by rememberSaveable {mutableStateOf<String?>(null)}
    var attachmentOwner by rememberSaveable {mutableStateOf<String?>(null)}
    val attachmentPicker=rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) {uri->
        val channel=attachmentChannel;val owner=attachmentOwner
        attachmentChannel=null;attachmentOwner=null
        if(uri!=null && channel!=null && owner!=null) model.stageAttachment(uri,channel,owner)
    }
    var pendingVoice by remember {mutableStateOf<Pair<JSONObject,String?>?>(null)}
    val micPermission=rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {allowed->
        if(allowed) pendingVoice?.let {(channel,guild)->model.joinVoice(channel,guild)}
        pendingVoice=null
    }
    val onVoice:(JSONObject,String?)->Unit={channel,guild->
        if(context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO)==android.content.pm.PackageManager.PERMISSION_GRANTED) model.joinVoice(channel,guild)
        else {pendingVoice=channel to guild;micPermission.launch(android.Manifest.permission.RECORD_AUDIO)}
    }
    val notificationPermission=rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {allowed->if(allowed) model.setPersistent(true)}
    val notifications:()->Unit={
        if(!s.persistent&&android.os.Build.VERSION.SDK_INT>=33&&context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS)!=android.content.pm.PackageManager.PERMISSION_GRANTED) notificationPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        else model.setPersistent(!s.persistent)
    }
    var account by remember {mutableStateOf(false)}
    var profile by remember {mutableStateOf(false)}
    var logout by remember {mutableStateOf(false)}
    var callControls by remember {mutableStateOf(false)}
    BackHandler(s.channel!=null) {model.back()}
    Surface(Modifier.fillMaxSize(),color=MobileBackground) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars).imePadding()) {
            s.error?.let {error->Surface(color=Color(0xff40212b)) {
                Row(Modifier.fillMaxWidth().padding(start=14.dp),verticalAlignment=Alignment.CenterVertically) {
                    Text(error,Modifier.weight(1f).padding(vertical=10.dp),color=Color(0xffffc6cb),fontSize=12.sp)
                    MobileIconButton(Glyph.Close,"エラーを閉じる",model::dismissError)
                }
            }}
            if(s.me==null&&s.restoring) {
                Column(Modifier.fillMaxSize().padding(24.dp),verticalArrangement=Arrangement.spacedBy(16.dp,Alignment.CenterVertically),horizontalAlignment=Alignment.CenterHorizontally) {
                    CircularProgressIndicator(color=MobileAccent)
                    Text(s.connection,color=MobileMuted,fontSize=14.sp)
                    TextButton(onClick=model::cancelRestore) {Text("別のアカウントでログイン")}
                }
            } else if(s.me==null) {
                Row(Modifier.fillMaxWidth().padding(horizontal=20.dp,vertical=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(10.dp)) {
                    MobileIcon(Glyph.Chat,tint=MobileAccent);Text("Riscord",fontSize=20.sp,fontWeight=FontWeight.Bold)
                }
                if(s.loading||s.loginBusy) LinearProgressIndicator(Modifier.fillMaxWidth())
                LoginScreen(s,model)
            } else {
                if(s.voiceChannel!=null) Surface(color=Color(0xff15372b),onClick={callControls=true}) {
                    Row(Modifier.fillMaxWidth().heightIn(min=48.dp).padding(start=14.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)) {
                        MobileIcon(Glyph.Phone,tint=Color(0xff7ce0aa),modifier=Modifier.size(18.dp))
                        Text(s.voiceStatus,Modifier.weight(1f),fontSize=13.sp,fontWeight=FontWeight.SemiBold,color=Color(0xffa1efbf))
                        MobileIconButton(if(s.mute) Glyph.Mute else Glyph.Mic,if(s.mute) "ミュートを解除" else "マイクをミュート",{model.voiceFlags(!s.mute,s.deaf)},tint=Color(0xffa1efbf))
                        MobileIconButton(Glyph.Close,"通話を切断",model::leaveVoice,tint=Color(0xffffa5ad))
                    }
                }
                Box(Modifier.weight(1f)) {
                    if(s.channel==null) screens.SaveableStateProvider("home") {MobileHome(s,model,{account=true},notifications,onVoice)}
                    else screens.SaveableStateProvider("chat-${s.channel!!.optString("id")}") {MobileChat(s,model,onVoice) {channel->attachmentChannel=channel;attachmentOwner=s.me?.optString("id");attachmentPicker.launch(arrayOf("*/*"))}}
                }
            }
        }
    }
    if(account) ModalBottomSheet(onDismissRequest={account=false},containerColor=MobileRaised) {
        Column(Modifier.fillMaxWidth().padding(horizontal=16.dp).padding(bottom=28.dp)) {
            Row(Modifier.padding(12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(14.dp)) {
                MobileAvatar(s.me ?: JSONObject(),52.dp)
                Column {Text(s.me?.label().orEmpty(),fontSize=21.sp,fontWeight=FontWeight.Bold);Text(s.connection,fontSize=12.sp,color=MobileMuted)}
            }
            SheetAction(Glyph.Edit,"プロフィールを編集",{account=false;profile=true;model.loadMyProfile()})
            SheetAction(Glyph.Bell,if(s.persistent) "バックグラウンド通知をオフ" else "バックグラウンド通知をオン",notifications)
            Text("オンにすると、アプリを開いていない間も接続を維持します。",Modifier.padding(horizontal=12.dp,vertical=8.dp),fontSize=12.sp,color=MobileMuted)
            HorizontalDivider(Modifier.padding(vertical=8.dp),color=Color.White.copy(alpha=.06f))
            SheetAction(Glyph.Logout,"ログアウト",{account=false;logout=true},danger=true)
        }
    }
    if(profile) ProfileEditor(s,model,{profile=false})
    if(logout) AlertDialog(onDismissRequest={logout=false},title={Text("ログアウト")},text={Text("保存したログイン情報をこの端末から削除します。")},confirmButton={TextButton(onClick={logout=false;model.logout()}) {Text("ログアウト",color=MaterialTheme.colorScheme.error)}},dismissButton={TextButton(onClick={logout=false}) {Text("キャンセル")}})
    s.incomingCall?.let {channel->AlertDialog(onDismissRequest=model::dismissCall,title={Text("着信")},text={Text(channel.channelLabel())},confirmButton={TextButton(onClick={onVoice(channel,null)}) {Text("応答")}},dismissButton={TextButton(onClick=model::dismissCall) {Text("拒否")}})}
    if(callControls&&s.voiceChannel!=null) ModalBottomSheet(onDismissRequest={callControls=false},containerColor=MobileRaised) {
        Column(Modifier.fillMaxWidth().padding(horizontal=20.dp).padding(bottom=24.dp),verticalArrangement=Arrangement.spacedBy(12.dp)) {
            Text(s.voiceStatus,fontSize=24.sp,fontWeight=FontWeight.Bold)
            SheetAction(if(s.mute) Glyph.Mute else Glyph.Mic,if(s.mute) "マイクのミュートを解除" else "マイクをミュート",{model.voiceFlags(!s.mute,s.deaf)})
            SheetAction(Glyph.Speaker,if(s.deaf) "通話の音声を戻す" else "通話の音声を消音",{model.voiceFlags(s.mute,!s.deaf)})
            SheetAction(Glyph.Phone,if(s.speaker) "受話口に切り替え" else "スピーカーに切り替え",{model.speaker(!s.speaker)})
            Button(onClick={model.leaveVoice();callControls=false},colors=ButtonDefaults.buttonColors(containerColor=Color(0xffd53f52)),modifier=Modifier.fillMaxWidth().height(52.dp)) {Text("通話を切断")}
        }
    }
}
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun ProfileEditor(s:ChatState,model:ChatModel,onDismiss:()->Unit) {
    var name by remember {mutableStateOf(s.me?.label().orEmpty())}
    var bio by remember {mutableStateOf("")}
    var status by remember {mutableStateOf(s.status)}
    LaunchedEffect(s.profile) {s.profile?.let {bio=it.nonNull("bio").orEmpty()}}
    val saving by rememberUpdatedState(s.profileSaving)
    val sheetState=rememberModalBottomSheetState(confirmValueChange={it!=SheetValue.Hidden || !saving})
    ModalBottomSheet(onDismissRequest={if(!s.profileSaving) onDismiss()},sheetState=sheetState,containerColor=MobileRaised,modifier=Modifier.imePadding()) {
        Column(Modifier.fillMaxWidth().fillMaxHeight(.85f).padding(horizontal=20.dp)) {
            Row(Modifier.fillMaxWidth(),verticalAlignment=Alignment.CenterVertically) {
                Text("プロフィール",Modifier.weight(1f),fontSize=23.sp,fontWeight=FontWeight.Bold)
                TextButton(onClick={model.saveProfile(name,bio,status,onDismiss)},enabled=s.profile!=null&&name.isNotBlank()&&!s.profileSaving) {Text(if(s.profileSaving) "保存中…" else "保存")}
            }
            s.profileError?.let {Text(it,Modifier.fillMaxWidth().padding(vertical=8.dp),color=MaterialTheme.colorScheme.error,fontSize=13.sp)}
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()),verticalArrangement=Arrangement.spacedBy(16.dp)) {
                OutlinedTextField(name,{name=it},label={Text("表示名")},singleLine=true,enabled=!s.profileSaving,modifier=Modifier.fillMaxWidth())
                if(s.profileLoading) CircularProgressIndicator()
                else if(s.profile==null) TextButton(onClick=model::loadMyProfile) {Text("再読み込み")}
                else OutlinedTextField(bio,{bio=it},label={Text("自己紹介")},minLines=3,maxLines=6,enabled=!s.profileSaving,modifier=Modifier.fillMaxWidth())
                Text("ステータス",fontSize=13.sp,color=MobileMuted)
                listOf("online" to "オンライン","idle" to "退席中","dnd" to "取り込み中","invisible" to "オンライン状態を隠す").forEach {(value,label)->Row(Modifier.fillMaxWidth().heightIn(min=48.dp).clickable(enabled=!s.profileSaving) {status=value},verticalAlignment=Alignment.CenterVertically) {RadioButton(status==value,{status=value},enabled=!s.profileSaving);Text(label,fontSize=14.sp)}}
                Spacer(Modifier.height(24.dp))
            }
        }
    }
}
@Composable private fun LoginScreen(s:ChatState, model:ChatModel) {
    var email by remember { mutableStateOf("") }; var password by remember { mutableStateOf("") }
    var code by remember { mutableStateOf("") }; var token by remember { mutableStateOf("") }
    var tokenMode by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(24.dp),verticalArrangement=Arrangement.spacedBy(16.dp)) {
        Text("おかえりなさい",style=MaterialTheme.typography.headlineMedium)
        Text("Riscord は非公式のDiscordクライアントです。",style=MaterialTheme.typography.bodySmall)
        if(s.ticket!=null) {
            OutlinedTextField(code,{code=it},label={Text("認証コード・バックアップコード")},modifier=Modifier.fillMaxWidth(),singleLine=true)
        } else {
            OutlinedTextField(email,{email=it},label={Text("メールアドレス")},modifier=Modifier.fillMaxWidth(),singleLine=true)
            OutlinedTextField(password,{password=it},label={Text("パスワード")},visualTransformation=PasswordVisualTransformation(),modifier=Modifier.fillMaxWidth(),singleLine=true)
        }
        Button(onClick={model.login(email,password,code);password=""}, enabled=!s.loginBusy && (s.ticket!=null && code.isNotBlank() || email.isNotBlank() && password.isNotBlank()),modifier=Modifier.fillMaxWidth()) { Text(if(s.loginBusy) "接続中…" else "ログイン") }
        HorizontalDivider()
        OutlinedButton(onClick=model::qrLogin,enabled=!s.loginBusy,modifier=Modifier.fillMaxWidth()) {Text("QRコードでログイン")}
        Text("別端末のDiscordアプリで読み取って承認してください。",style=MaterialTheme.typography.bodySmall)
        s.qr?.let { qr ->
            val bits=qr.getJSONArray("bits"); val size=qr.getInt("size")
            Canvas(Modifier.size(256.dp).align(Alignment.CenterHorizontally).background(Color.White)) {
                val cell=this.size.width/(size+8)
                for(y in 0 until size) for(x in 0 until size) if(bits.getBoolean(y*size+x)) drawRect(Color.Black,topLeft=androidx.compose.ui.geometry.Offset((x+4)*cell,(y+4)*cell),size=androidx.compose.ui.geometry.Size(cell+0.2f,cell+0.2f))
            }
            Text(s.connection)
        }
        TextButton(onClick={tokenMode=!tokenMode}) {Text("詳細なログイン方法")}
        if(tokenMode) {
            Text("既に持っている自分のアカウントのトークンでログインできます。トークンは他人と共有しないでください。",style=MaterialTheme.typography.bodySmall)
            OutlinedTextField(token,{token=it},label={Text("アカウントトークン")},visualTransformation=PasswordVisualTransformation(),singleLine=true,modifier=Modifier.fillMaxWidth())
            Button(onClick={model.tokenLogin(token);token=""},enabled=token.isNotBlank()&&!s.loginBusy) {Text("接続")}
        }
    }
}
