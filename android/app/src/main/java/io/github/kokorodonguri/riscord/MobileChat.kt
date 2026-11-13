package io.github.kokorodonguri.riscord

import androidx.compose.foundation.*
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.shape.*
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.*
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.*
import androidx.compose.ui.text.font.*
import androidx.compose.ui.text.input.*
import androidx.compose.ui.text.style.*
import androidx.compose.ui.unit.*
import coil.compose.AsyncImage
import kotlinx.coroutines.launch
import org.json.JSONObject
import java.time.Duration

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun MobileChat(s:ChatState,model:ChatModel,onVoice:(JSONObject,String?)->Unit,onPickAttachment:(String)->Unit) {
    val channel=s.channel ?: return
    val id=channel.getString("id")
    val dm=channel.optInt("type") in listOf(1,3)
    val list=rememberSaveable(id,saver=LazyListState.Saver) {LazyListState()}
    val scope=rememberCoroutineScope()
    var followLatest by rememberSaveable(id) {mutableStateOf(true)}
    val nearBottom by remember {derivedStateOf {list.firstVisibleItemIndex==0 && list.firstVisibleItemScrollOffset<80}}
    var selected by remember(id) {mutableStateOf<JSONObject?>(null)}
    var deleting by remember {mutableStateOf<JSONObject?>(null)}
    var info by remember {mutableStateOf(false)}
    val focus=remember {FocusRequester()}
    val focusManager=LocalFocusManager.current
    val keyboard=LocalSoftwareKeyboardController.current
    val density=LocalDensity.current
    var swiping by remember {mutableStateOf(false)}
    var drag by remember {mutableFloatStateOf(0f)}
    val latest=s.messages.lastOrNull()?.optString("id")
    LaunchedEffect(list) {snapshotFlow {list.isScrollInProgress to nearBottom}.collect {(scrolling,bottom)->if(scrolling) followLatest=bottom}}
    LaunchedEffect(latest) {if(followLatest) list.scrollToItem(0)}
    LaunchedEffect(s.reply?.optString("id"),s.editing?.optString("id")) {if(s.reply!=null||s.editing!=null) focus.requestFocus()}
    Column(Modifier.fillMaxSize().background(MobilePanel).pointerInput(id) {
        detectHorizontalDragGestures(onDragStart={swiping=it.x<with(density) {40.dp.toPx()};drag=0f},onHorizontalDrag={change,amount->if(swiping) {drag+=amount;change.consume()}},onDragEnd={if(swiping&&drag>with(density) {80.dp.toPx()}) model.back();swiping=false})
    }) {
        Row(Modifier.fillMaxWidth().heightIn(min=64.dp).padding(end=4.dp),verticalAlignment=Alignment.CenterVertically) {
            MobileIconButton(Glyph.Back,tr("チャンネル一覧に戻る","Back to channels","返回频道列表"),model::back)
            if(dm) MobileAvatar(if(channel.optInt("type")==3) channel else channelUser(channel),32.dp,channel.optInt("type")==3)
            else MobileIcon(Glyph.Hash,tint=MobileMuted,modifier=Modifier.size(24.dp))
            Column(Modifier.weight(1f).clickable {info=true}.padding(start=10.dp,end=4.dp,top=10.dp,bottom=10.dp)) {
                Text(channel.channelLabel(),fontWeight=FontWeight.Bold,fontSize=17.sp,maxLines=1,overflow=TextOverflow.Ellipsis)
                Text(if(dm) if(channel.optInt("type")==3) tr("グループDM","Group DM","群组私信") else tr("ダイレクトメッセージ","Direct Message","私信") else s.guild?.label().orEmpty(),color=MobileMuted,fontSize=11.sp,maxLines=1,overflow=TextOverflow.Ellipsis)
            }
            if(dm) MobileIconButton(Glyph.Phone,tr("音声通話を開始","Start voice call","发起语音通话"),{onVoice(channel,null)},enabled=s.voiceChannel==null,tint=MobileMuted)
            MobileIconButton(Glyph.More,tr("チャンネル情報","Channel info","频道信息"),{info=true},tint=MobileMuted)
        }
        HorizontalDivider(color=Color.White.copy(alpha=.05f))
        if(s.messageLoadFailed && s.messages.isNotEmpty() && !s.loading) {
            TextButton(onClick=model::retryMessages,modifier=Modifier.align(Alignment.End).padding(horizontal=12.dp)) {Text(tr("メッセージを再読み込み","Reload messages","重新加载消息"))}
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            LazyColumn(state=list,reverseLayout=true,contentPadding=PaddingValues(top=12.dp,bottom=12.dp),modifier=Modifier.fillMaxSize().semantics {contentDescription=tr("メッセージ履歴","Message history","消息历史")}) {
                itemsIndexed(s.messages.asReversed(),key={_,message->message.getString("id")}) {reverseIndex,message->
                    val index=s.messages.lastIndex-reverseIndex
                    val older=s.messages.getOrNull(index-1)
                    val dateChanged=older==null || messageDay(older)!=messageDay(message)
                    val grouped=!dateChanged && message.optJSONObject("referenced_message")==null && older?.optJSONObject("author")?.optString("id")==message.optJSONObject("author")?.optString("id") && runCatching {Duration.between(messageInstant(older!!),messageInstant(message)).seconds in 0..300}.getOrDefault(false)
                    Column {
                        if(dateChanged) DateDivider(messageDate(message))
                        MobileMessage(message,grouped,{focusManager.clearFocus();keyboard?.hide();selected=message},{model.toggleReaction(message,it)},s.pendingPollVotes.contains("${message.optString("channel_id")}:${message.optString("id")}"),{model.togglePollVote(message,it)})
                    }
                }
                item("older") {
                    Box(Modifier.fillMaxWidth().padding(vertical=12.dp),contentAlignment=Alignment.Center) {
                        if(s.loading) CircularProgressIndicator(Modifier.size(24.dp),strokeWidth=2.dp)
                        else if(s.messageLoadFailed && s.messages.isEmpty()) Column(Modifier.padding(24.dp),horizontalAlignment=Alignment.CenterHorizontally,verticalArrangement=Arrangement.spacedBy(12.dp)) {
                            Text(tr("メッセージを読み込めませんでした","Couldn't load messages","无法加载消息"),fontWeight=FontWeight.SemiBold,fontSize=16.sp)
                            Text(tr("通信状態を確認して再試行してください。","Check your connection and try again.","请检查网络连接后重试。"),color=MobileMuted,fontSize=13.sp)
                            TextButton(onClick=model::retryMessages) {Text(tr("再読み込み","Reload","重新加载"))}
                        }
                        else if(!s.messageLoadFailed&&s.hasMore&&s.messages.isNotEmpty()) TextButton(onClick=model::older) {Text(tr("以前のメッセージを読み込む","Load older messages","加载更早的消息"),fontSize=12.sp)}
                        else if(s.messages.isEmpty()) Column(Modifier.padding(24.dp),verticalArrangement=Arrangement.spacedBy(12.dp)) {
                            MobileIcon(if(dm) Glyph.Chat else Glyph.Hash,tint=MobileMuted,modifier=Modifier.size(44.dp))
                            Text(if(dm) channel.channelLabel() else tr("#${channel.channelLabel()} へようこそ","Welcome to #${channel.channelLabel()}","欢迎来到 #${channel.channelLabel()}"),fontSize=24.sp,fontWeight=FontWeight.Bold)
                            Text(if(dm) tr("ここから会話をはじめましょう。","Start the conversation here.","从这里开始对话吧。") else tr("このチャンネルの最初のメッセージを送信しましょう。","Send the first message in this channel.","来发送此频道的第一条消息吧。"),color=MobileMuted,fontSize=14.sp)
                        }
                    }
                }
            }
            if(!nearBottom&&s.messages.isNotEmpty()) SmallFloatingActionButton(onClick={scope.launch {list.animateScrollToItem(0);followLatest=true}},containerColor=MobileRaised,contentColor=Color.White,modifier=Modifier.align(Alignment.BottomEnd).padding(12.dp)) {MobileIcon(Glyph.Down,tr("最新のメッセージへ","Jump to latest","跳到最新消息"))}
        }
        MessageComposer(s,model,focus,onPickAttachment)
    }
    selected?.let {message->MessageActions(message,s.me?.optString("id").orEmpty(),onDismiss={selected=null},onReply={model.reply(message);selected=null},onEdit={model.beginEdit(message);selected=null},onDelete={deleting=message;selected=null},onReact={model.react(message,it);selected=null})}
    deleting?.let {message->AlertDialog(onDismissRequest={deleting=null},title={Text(tr("メッセージを削除","Delete message","删除消息"))},text={Text(tr("このメッセージを削除します。元に戻せません。","This message will be deleted. This can't be undone.","将删除这条消息。此操作无法撤销。"))},confirmButton={TextButton(onClick={model.delete(message);deleting=null}) {Text(tr("削除","Delete","删除"),color=MaterialTheme.colorScheme.error)}},dismissButton={TextButton(onClick={deleting=null}) {Text(tr("キャンセル","Cancel","取消"))}})}
    if(info) ModalBottomSheet(onDismissRequest={info=false},containerColor=MobileRaised) {
        Column(Modifier.fillMaxWidth().padding(horizontal=24.dp).padding(bottom=32.dp),verticalArrangement=Arrangement.spacedBy(16.dp)) {
            if(dm) MobileAvatar(channelUser(channel),56.dp) else MobileIcon(Glyph.Hash,modifier=Modifier.size(36.dp),tint=MobileMuted)
            Text(channel.channelLabel(),fontSize=24.sp,fontWeight=FontWeight.Bold)
            channel.nonNull("topic")?.let {Text(it,color=MobileMuted)}
            if(!dm) Text(s.guild?.label().orEmpty(),color=MobileMuted)
            if(dm) OutlinedButton(onClick={onVoice(channel,null);info=false},enabled=s.voiceChannel==null,modifier=Modifier.fillMaxWidth()) {MobileIcon(Glyph.Phone);Spacer(Modifier.width(10.dp));Text(tr("音声通話を開始","Start voice call","发起语音通话"))}
        }
    }
}
@Composable private fun DateDivider(label:String) {
    Row(Modifier.fillMaxWidth().padding(horizontal=16.dp,vertical=18.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(12.dp)) {
        HorizontalDivider(Modifier.weight(1f),color=Color.White.copy(alpha=.09f));Text(label,fontSize=11.sp,fontWeight=FontWeight.SemiBold,color=MobileMuted);HorizontalDivider(Modifier.weight(1f),color=Color.White.copy(alpha=.09f))
    }
}
@OptIn(ExperimentalFoundationApi::class)
@Composable private fun MobileMessage(message:JSONObject,grouped:Boolean,onLongPress:()->Unit,onReaction:(JSONObject)->Unit,pollPending:Boolean,onPollVote:(Int)->Unit) {
    val author=message.optJSONObject("author") ?: JSONObject()
    val context=LocalContext.current
    Row(Modifier.fillMaxWidth().combinedClickable(onClick={},onLongClick=onLongPress,onLongClickLabel=tr("メッセージ操作","Message actions","消息操作")).padding(start=12.dp,end=16.dp,top=if(grouped) 2.dp else 12.dp,bottom=2.dp),horizontalArrangement=Arrangement.spacedBy(12.dp)) {
        if(grouped) Spacer(Modifier.width(38.dp)) else MobileAvatar(author,38.dp)
        Column(Modifier.weight(1f)) {
            message.optJSONObject("referenced_message")?.let {referenced->
                Row(Modifier.padding(bottom=5.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(6.dp)) {
                    MobileIcon(Glyph.Reply,modifier=Modifier.size(13.dp),tint=MobileMuted)
                    Text(referenced.optJSONObject("author")?.label().orEmpty(),fontSize=11.sp,fontWeight=FontWeight.SemiBold,color=MobileMuted,maxLines=1)
                    Text(displayContent(referenced),fontSize=11.sp,color=MobileMuted,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f))
                }
            }
            if(!grouped) Row(Modifier.padding(bottom=3.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)) {
                Text(author.label(),fontWeight=FontWeight.SemiBold,fontSize=15.sp,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f,fill=false))
                if(author.optBoolean("bot")) Text("APP",fontSize=9.sp,color=MobileAccent,fontWeight=FontWeight.Bold)
                Text(messageTime(message),fontSize=10.sp,color=MobileMuted)
            }
            val content=displayContent(message)
            if(content.isNotBlank()) {
                Text(richMessage(content),fontSize=15.sp,lineHeight=22.sp)
                if(message.nonNull("edited_timestamp")!=null) Text(tr("編集済み","edited","已编辑"),fontSize=10.sp,color=MobileMuted)
            }
            message.optJSONObject("poll")?.let { poll ->
                Surface(shape=RoundedCornerShape(12.dp),color=MobileRaised,modifier=Modifier.fillMaxWidth().padding(top=6.dp)) {
                    Column(Modifier.padding(12.dp),verticalArrangement=Arrangement.spacedBy(6.dp)) {
                        Text(poll.optJSONObject("question")?.optString("text").orEmpty(),fontWeight=FontWeight.SemiBold)
                        val counts=poll.optJSONObject("results")?.optJSONArray("answer_counts")?.objects().orEmpty()
                        poll.optJSONArray("answers")?.objects().orEmpty().forEach { answer ->
                            val media=answer.optJSONObject("poll_media")
                            val count=counts.firstOrNull {it.optInt("id")==answer.optInt("answer_id")}
                            val emoji=media?.optJSONObject("emoji")?.optString("name").orEmpty()
                            val label=media?.optString("text").orEmpty()
                            val expired=poll.nonNull("expiry")?.let { runCatching { !java.time.OffsetDateTime.parse(it).toInstant().isAfter(java.time.Instant.now()) }.getOrDefault(false) } ?: false
                            val enabled=poll.optJSONObject("results")?.optBoolean("is_finalized") == false && !expired && !pollPending
                            Row(Modifier.fillMaxWidth().clickable(enabled=enabled) {onPollVote(answer.optInt("answer_id"))}.padding(vertical=4.dp),horizontalArrangement=Arrangement.SpaceBetween) {
                                Text("$emoji $label",modifier=Modifier.weight(1f),fontSize=13.sp)
                                if(count!=null) Text(tr("${count.optInt("count")} 票","${count.optInt("count")} votes","${count.optInt("count")} 票")+if(count.optBoolean("me_voted")) tr(" · 回答済み"," · Voted"," · 已投票") else "",fontSize=12.sp,color=MobileMuted)
                            }
                        }
                        if(poll.optJSONObject("results")==null) Text(tr("投票結果を取得できませんでした","Couldn't load poll results","无法获取投票结果"),fontSize=11.sp,color=MobileMuted)
                    }
                }
            }
            val reactions=message.optJSONArray("reactions")?.objects().orEmpty()
            if(reactions.isNotEmpty()) Row(Modifier.horizontalScroll(rememberScrollState()).padding(top=5.dp),horizontalArrangement=Arrangement.spacedBy(6.dp)) {
                reactions.forEach {reaction->Surface(onClick={onReaction(reaction)},shape=RoundedCornerShape(8.dp),color=if(reaction.optBoolean("me")) MobileAccent.copy(alpha=.2f) else MobileRaised) {
                    Text("${reaction.optJSONObject("emoji")?.optString("name").orEmpty()}  ${reaction.optInt("count")}",Modifier.padding(horizontal=9.dp,vertical=5.dp),fontSize=12.sp)
                }}
            }
            val attachments=message.optJSONArray("attachments")?.objects().orEmpty()
            val images=attachments.filter {it.optString("content_type").startsWith("image/")}
            // 画面幅ぶんの画素で取れば十分（押したときは原寸の url を開く）
            val maxPx=with(LocalDensity.current) {LocalConfiguration.current.screenWidthDp.dp.roundToPx()}
            val preview={image:JSONObject->previewUrl(image.nonNull("proxy_url"),image.optString("url"),image.optInt("width"),image.optInt("height"),maxPx)}
            if(images.size>1) Column(Modifier.padding(top=6.dp),verticalArrangement=Arrangement.spacedBy(4.dp)) {
                images.chunked(2).forEach {row->Row(horizontalArrangement=Arrangement.spacedBy(4.dp)) {
                    row.forEach {image->AsyncImage(preview(image),image.optString("filename"),Modifier.weight(1f).aspectRatio(1.1f).clip(RoundedCornerShape(12.dp)).background(MobileRaised).clickable {openUrl(context,image.optString("url"))},contentScale=androidx.compose.ui.layout.ContentScale.Crop)}
                    if(row.size==1) Spacer(Modifier.weight(1f))
                }}
            } else images.firstOrNull()?.let {image->
                val ratio=if(image.optInt("height")>0) (image.optDouble("width",1.0)/image.optDouble("height",1.0)).toFloat().coerceIn(.7f,2.4f) else 1.4f
                AsyncImage(preview(image),image.optString("filename"),Modifier.padding(top=6.dp).fillMaxWidth().heightIn(max=300.dp).aspectRatio(ratio).clip(RoundedCornerShape(14.dp)).background(MobileRaised).clickable {openUrl(context,image.optString("url"))},contentScale=androidx.compose.ui.layout.ContentScale.Fit)
            }
            attachments.filterNot {it in images}.forEach {attachment->Surface(onClick={openUrl(context,attachment.optString("url"))},shape=RoundedCornerShape(12.dp),color=MobileRaised,modifier=Modifier.fillMaxWidth().padding(top=6.dp)) {
                Row(Modifier.padding(12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(10.dp)) {MobileIcon(Glyph.File,tint=MobileMuted);Text(attachment.optString("filename",tr("添付ファイル","Attachment","附件")),Modifier.weight(1f),fontSize=13.sp,maxLines=2,overflow=TextOverflow.Ellipsis);MobileIcon(Glyph.Download,tint=MobileMuted)}
            }}
            message.optJSONArray("embeds")?.objects()?.forEach {embed->
                Surface(shape=RoundedCornerShape(12.dp),color=MobileRaised,modifier=Modifier.fillMaxWidth().padding(top=6.dp)) {
                    Column(Modifier.padding(12.dp),verticalArrangement=Arrangement.spacedBy(6.dp)) {
                        val url=embed.optString("url")
                        embed.nonNull("title")?.let {Text(it,fontSize=14.sp,fontWeight=FontWeight.SemiBold,color=MobileAccent,modifier=Modifier.clickable(enabled=url.startsWith("https://")) {openUrl(context,url)})}
                        embed.nonNull("description")?.let {Text(it,fontSize=13.sp,lineHeight=19.sp)}
                        (embed.optJSONObject("image") ?: embed.optJSONObject("thumbnail"))?.let {AsyncImage(it.optString("proxy_url",it.optString("url")),null,Modifier.fillMaxWidth().heightIn(max=220.dp).clip(RoundedCornerShape(8.dp)))}
                    }
                }
            }
        }
    }
}
private fun richMessage(text:String):AnnotatedString=buildAnnotatedString {
    val regex=Regex("```[\\s\\S]*?```|`[^`\\n]+`|\\*\\*[^*]+\\*\\*|https?://[^\\s<>]+")
    var offset=0
    for(match in regex.findAll(text)) {
        append(text.substring(offset,match.range.first));val token=match.value
        when {
            token.startsWith("http")->withLink(LinkAnnotation.Url(token)) {withStyle(SpanStyle(color=MobileAccent,textDecoration=TextDecoration.Underline)) {append(token)}}
            token.startsWith("```")->withStyle(SpanStyle(fontFamily=FontFamily.Monospace,background=MobileRaised)) {append(token.removePrefix("```").removeSuffix("```"))}
            token.startsWith("`")->withStyle(SpanStyle(fontFamily=FontFamily.Monospace,background=MobileRaised)) {append(token.removeSurrounding("`"))}
            else->withStyle(SpanStyle(fontWeight=FontWeight.Bold)) {append(token.removeSurrounding("**"))}
        }
        offset=match.range.last+1
    }
    append(text.substring(offset))
}
@Composable private fun MessageComposer(s:ChatState,model:ChatModel,focus:FocusRequester,onPickAttachment:(String)->Unit) {
    val id=s.channel?.optString("id") ?: return
    var emoji by remember {mutableStateOf(false)}
    val editing=s.editing!=null
    val text=if(editing) s.editDraft else s.drafts[id].orEmpty()
    val attachment=s.attachments[id]
    val change:(String)->Unit=if(editing) model::editDraft else model::draft
    Column(Modifier.fillMaxWidth().background(MobilePanel).padding(horizontal=10.dp).padding(top=4.dp,bottom=8.dp)) {
        if(s.reply!=null||editing) Surface(color=MobileRaised,shape=RoundedCornerShape(topStart=18.dp,topEnd=18.dp)) {
            Row(Modifier.fillMaxWidth().padding(start=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)) {
                MobileIcon(if(editing) Glyph.Edit else Glyph.Reply,modifier=Modifier.size(18.dp),tint=MobileAccent)
                Column(Modifier.weight(1f).padding(vertical=8.dp)) {
                    Text(if(editing) tr("メッセージを編集","Editing message","正在编辑消息") else s.reply?.optJSONObject("author")?.label().orEmpty().let {tr("$it に返信","Replying to $it","回复 $it")},fontSize=12.sp,fontWeight=FontWeight.SemiBold)
                    if(!editing) Text(s.reply?.optString("content").orEmpty(),fontSize=11.sp,color=MobileMuted,maxLines=1,overflow=TextOverflow.Ellipsis)
                }
                MobileIconButton(Glyph.Close,if(editing) tr("編集をキャンセル","Cancel edit","取消编辑") else tr("返信を解除","Cancel reply","取消回复"),{if(editing) model.cancelEdit() else model.reply(null)})
            }
        }
        attachment?.let {item->Surface(shape=RoundedCornerShape(14.dp),color=MobileRaised,modifier=Modifier.padding(bottom=6.dp)) {
            Row(Modifier.fillMaxWidth().padding(start=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)) {
                MobileIcon(Glyph.File,tint=MobileAccent)
                Column(Modifier.weight(1f)) {Text(item.name,fontSize=13.sp,maxLines=1,overflow=TextOverflow.Ellipsis);Text(tr("送信待ち","Pending","等待发送")+(if(item.size>=0) " • ${item.size/1024} KB" else ""),fontSize=11.sp,color=MobileMuted)}
                MobileIconButton(Glyph.Close,tr("添付を取り消す","Remove attachment","移除附件"),model::removeAttachment,enabled=!s.sending)
            }
        }}
        Surface(color=MobileRaised,shape=RoundedCornerShape(if(s.reply!=null||editing) 12.dp else 26.dp),modifier=Modifier.fillMaxWidth()) {
            Row(Modifier.fillMaxWidth().heightIn(min=54.dp).padding(3.dp),verticalAlignment=Alignment.Bottom) {
                if(!editing) MobileIconButton(Glyph.Plus,tr("ファイルを添付","Attach file","附加文件"),{onPickAttachment(id)},enabled=!s.sending,tint=MobileMuted)
                key(editing) {
                    BasicTextField(text,change,textStyle=MaterialTheme.typography.bodyMedium.copy(color=Color(0xfff1f1f4),fontSize=15.sp,lineHeight=21.sp),cursorBrush=SolidColor(MobileAccent),maxLines=5,keyboardOptions=KeyboardOptions(capitalization=KeyboardCapitalization.Sentences,imeAction=ImeAction.Default),modifier=Modifier.weight(1f).padding(horizontal=6.dp,vertical=14.dp).focusRequester(focus).semantics {contentDescription=tr("メッセージ入力","Message input","消息输入")},decorationBox={inner->Box {if(text.isEmpty()) Text(if(editing) tr("メッセージを編集","Editing message","正在编辑消息") else "${if(s.channel.optInt("type") in listOf(1,3)) "@" else "#"}${s.channel.channelLabel()} へメッセージ",color=MobileMuted,fontSize=14.sp,maxLines=1,overflow=TextOverflow.Ellipsis);inner()}})
                }
                if(text.isNotBlank()||attachment!=null||editing) {
                    FilledIconButton(onClick=model::send,enabled=!s.sending&&(text.isNotBlank()||!editing&&attachment!=null),shape=CircleShape,modifier=Modifier.size(48.dp),colors=IconButtonDefaults.filledIconButtonColors(containerColor=MobileAccent)) {
                        if(s.sending) CircularProgressIndicator(Modifier.size(20.dp),strokeWidth=2.dp,color=Color.White)
                        else MobileIcon(if(editing) Glyph.Check else Glyph.Send,if(editing) tr("編集を保存","Save edit","保存编辑") else tr("メッセージを送信","Send message","发送消息"),modifier=Modifier.size(21.dp),tint=Color.White)
                    }
                } else MobileIconButton(Glyph.Smile,tr("絵文字を選ぶ","Choose emoji","选择表情"),{emoji=!emoji},tint=MobileMuted)
            }
        }
        s.typing?.let {Text(it,Modifier.padding(start=14.dp,top=4.dp),fontSize=10.sp,color=MobileMuted)}
        if(emoji) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),horizontalArrangement=Arrangement.SpaceEvenly) {
            listOf("😀","😂","🥰","👍","❤️","🎉","🙏","👀").forEach {value->TextButton(onClick={change(text+value);emoji=false;focus.requestFocus()},modifier=Modifier.size(48.dp),contentPadding=PaddingValues(0.dp)) {Text(value,fontSize=24.sp)}}
        }
    }
}
@OptIn(ExperimentalMaterial3Api::class)
@Composable private fun MessageActions(message:JSONObject,myId:String,onDismiss:()->Unit,onReply:()->Unit,onEdit:()->Unit,onDelete:()->Unit,onReact:(String)->Unit) {
    val clipboard=LocalContext.current.getSystemService(android.content.ClipboardManager::class.java)
    ModalBottomSheet(onDismissRequest=onDismiss,containerColor=MobileRaised) {
        Column(Modifier.fillMaxWidth().padding(horizontal=16.dp).padding(bottom=24.dp)) {
            Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceEvenly) {
                listOf("👍","❤️","😂","🎉","👀").forEach {emoji->Surface(onClick={onReact(emoji)},shape=CircleShape,color=MobilePanel,modifier=Modifier.size(48.dp)) {Box(contentAlignment=Alignment.Center) {Text(emoji,fontSize=25.sp)}}}
            }
            Text(displayContent(message).ifBlank {tr("添付メッセージ","Attachment message","附件消息")},Modifier.padding(vertical=16.dp,horizontal=8.dp),fontSize=13.sp,color=MobileMuted,maxLines=2,overflow=TextOverflow.Ellipsis)
            SheetAction(Glyph.Reply,tr("返信","Reply","回复"),onReply)
            SheetAction(Glyph.Copy,tr("テキストをコピー","Copy text","复制文字"),{clipboard.setPrimaryClip(android.content.ClipData.newPlainText(tr("メッセージ","Messages","消息"),message.optString("content")));onDismiss()})
            if(message.optJSONObject("author")?.optString("id")==myId) {
                SheetAction(Glyph.Edit,tr("編集","Edit","编辑"),onEdit)
                SheetAction(Glyph.Trash,tr("削除","Delete","删除"),onDelete,danger=true)
            }
        }
    }
}
@Composable internal fun SheetAction(glyph:Glyph,label:String,onClick:()->Unit,danger:Boolean=false) {
    Surface(onClick=onClick,color=Color.Transparent,shape=RoundedCornerShape(12.dp),modifier=Modifier.fillMaxWidth()) {
        Row(Modifier.heightIn(min=54.dp).padding(horizontal=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(16.dp)) {
            val color=if(danger) Color(0xffff7c86) else Color(0xffededf3)
            MobileIcon(glyph,tint=color);Text(label,color=color,fontSize=15.sp)
        }
    }
}
