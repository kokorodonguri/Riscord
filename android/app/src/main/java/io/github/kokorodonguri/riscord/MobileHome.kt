package io.github.kokorodonguri.riscord

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.*
import androidx.compose.foundation.shape.*
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.*
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.*
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import org.json.JSONObject

internal data class ChannelSection(val category:JSONObject?,val channels:List<JSONObject>)
internal fun channelSections(channels:List<JSONObject>,query:String):List<ChannelSection> {
    val categories=channels.filter {it.optInt("type")==4}.sortedBy {it.optInt("position")}
    val ids=categories.map {it.optString("id")}.toSet()
    val visible=channels.filter {it.optInt("type") in listOf(0,2,5,10,11,12,13,15,16)}
    fun matches(channel:JSONObject)=query.isBlank() || channel.channelLabel().contains(query,true)
    val result=mutableListOf<ChannelSection>()
    val root=visible.filter {(it.nonNull("parent_id") ?: "") !in ids && matches(it)}.sortedBy {it.optInt("position")}
    if(root.isNotEmpty()) result+=ChannelSection(null,root)
    for(category in categories) {
        val items=visible.filter {it.optString("parent_id")==category.optString("id") && (category.channelLabel().contains(query,true)||matches(it))}.sortedBy {it.optInt("position")}
        if(items.isNotEmpty()) result+=ChannelSection(category,items)
    }
    return result
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable internal fun MobileHome(s:ChatState,model:ChatModel,onAccount:()->Unit,onNotifications:()->Unit,onVoice:(JSONObject,String?)->Unit) {
    var query by rememberSaveable(s.guild?.optString("id")) {mutableStateOf("")}
    var collapsed by rememberSaveable(s.guild?.optString("id")) {mutableStateOf(arrayListOf<String>())}
    var newDm by remember {mutableStateOf(false)}
    var recipient by rememberSaveable {mutableStateOf("")}
    var voice by remember {mutableStateOf<JSONObject?>(null)}
    val focus=LocalFocusManager.current
    BackHandler(query.isNotEmpty()) {query="";focus.clearFocus()}
    Column(Modifier.fillMaxSize().background(MobileBackground)) {
        Row(Modifier.weight(1f)) {
            ServerRail(s,model::selectGuild)
            Surface(shape=RoundedCornerShape(topStart=28.dp),color=MobilePanel,border=BorderStroke(1.dp,Color.White.copy(alpha=.06f)),modifier=Modifier.weight(1f).fillMaxHeight()) {
                Column {
                    Row(Modifier.fillMaxWidth().padding(start=16.dp,end=16.dp,top=12.dp,bottom=8.dp),verticalAlignment=Alignment.CenterVertically) {
                        Text(s.guild?.label() ?: "メッセージ",Modifier.weight(1f),fontSize=23.sp,fontWeight=FontWeight.Bold,maxLines=1,overflow=TextOverflow.Ellipsis)
                    }
                    Row(Modifier.padding(horizontal=12.dp).padding(top=4.dp,bottom=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(8.dp)) {
                        Surface(shape=RoundedCornerShape(14.dp),color=MobileRaised,modifier=Modifier.weight(1f)) {
                            Row(Modifier.heightIn(min=48.dp).padding(start=12.dp),verticalAlignment=Alignment.CenterVertically) {
                                MobileIcon(Glyph.Search,tint=MobileMuted,modifier=Modifier.size(20.dp))
                                BasicTextField(query,{query=it},singleLine=true,textStyle=MaterialTheme.typography.bodyMedium.copy(color=MaterialTheme.colorScheme.onSurface),cursorBrush=SolidColor(MobileAccent),keyboardOptions=KeyboardOptions(imeAction=ImeAction.Search),keyboardActions=KeyboardActions(onSearch={focus.clearFocus()}),modifier=Modifier.weight(1f).padding(10.dp).semantics {contentDescription=if(s.guild==null) "DMを検索" else "チャンネルを検索"},decorationBox={inner->
                                    Box {if(query.isEmpty()) Text(if(s.guild==null) "メッセージを検索" else "チャンネルを検索",color=MobileMuted,fontSize=13.sp,maxLines=1);inner()}
                                })
                                if(query.isNotEmpty()) MobileIconButton(Glyph.Close,"検索をクリア",{query=""},modifier=Modifier.width(48.dp))
                            }
                        }
                        if(s.guild==null) FilledIconButton(onClick={newDm=true},shape=RoundedCornerShape(14.dp),modifier=Modifier.size(48.dp),colors=IconButtonDefaults.filledIconButtonColors(containerColor=MobileAccent)) {MobileIcon(Glyph.Plus,"新しいDM",tint=Color.White)}
                    }
                    if(s.loading) LinearProgressIndicator(Modifier.fillMaxWidth(),color=MobileAccent)
                    val dms=s.dms.sortedByDescending {it.optString("last_message_id").padStart(20,'0')}.filter {it.channelLabel().contains(query,true)}
                    val sections=channelSections(s.channels,query)
                    LazyColumn(Modifier.fillMaxSize(),contentPadding=PaddingValues(start=8.dp,end=8.dp,bottom=12.dp),state=rememberSaveable(s.guild?.optString("id"),saver=LazyListState.Saver) {LazyListState()}) {
                        if(s.guild==null) {
                            if(query.isEmpty()&&dms.size>2) item("recent") {
                                LazyRow(contentPadding=PaddingValues(horizontal=4.dp,vertical=4.dp),horizontalArrangement=Arrangement.spacedBy(8.dp),modifier=Modifier.padding(bottom=12.dp)) {
                                    items(dms.take(6),key={it.optString("id")}) {dm->
                                        Surface(onClick={model.openChannel(dm)},shape=RoundedCornerShape(18.dp),color=MobileRaised,modifier=Modifier.width(76.dp)) {
                                            Column(Modifier.padding(vertical=12.dp,horizontal=6.dp),horizontalAlignment=Alignment.CenterHorizontally,verticalArrangement=Arrangement.spacedBy(8.dp)) {
                                                MobileAvatar(if(dm.optInt("type")==3) dm else channelUser(dm),44.dp,dm.optInt("type")==3)
                                                Text(dm.channelLabel(),fontSize=11.sp,maxLines=1,overflow=TextOverflow.Ellipsis)
                                            }
                                        }
                                    }
                                }
                            }
                            items(dms,key={"dm-${it.optString("id")}"}) {dm->DmRow(dm,s,{model.openChannel(dm)})}
                            if(dms.isEmpty()&&!s.loading) item {HomeEmpty(if(query.isNotEmpty()) "メッセージが見つかりません" else "会話をはじめよう",if(query.isNotEmpty()) "別の名前で検索してください" else "右上の＋からDMを開けます")}
                        } else {
                            sections.forEach {section->
                                section.category?.let {category->
                                    item("category-${category.optString("id")}") {
                                        Row(Modifier.fillMaxWidth().heightIn(min=48.dp).clickable {
                                            val id=category.optString("id");collapsed=ArrayList(collapsed).apply {if(contains(id)) remove(id) else add(id)}
                                        }.padding(horizontal=8.dp).semantics {stateDescription=if(category.optString("id") in collapsed) "折りたたみ" else "展開中"},verticalAlignment=Alignment.CenterVertically) {
                                            Text(category.channelLabel(),Modifier.weight(1f),color=MobileMuted,fontSize=12.sp,fontWeight=FontWeight.SemiBold,maxLines=1,overflow=TextOverflow.Ellipsis)
                                            MobileIcon(if(category.optString("id") in collapsed) Glyph.Right else Glyph.Down,modifier=Modifier.size(16.dp),tint=MobileMuted)
                                        }
                                    }
                                }
                                if(query.isNotEmpty() || section.category?.optString("id") !in collapsed) {
                                    items(section.channels,key={"channel-${it.optString("id")}"}) {channel->
                                        val isVoice=channel.optInt("type") in listOf(2,13)
                                        val selected=s.lastChannelId==channel.optString("id")
                                        Surface(onClick={if(isVoice) voice=channel else model.openChannel(channel)},color=if(selected) MobileRaised else Color.Transparent,shape=RoundedCornerShape(10.dp),modifier=Modifier.fillMaxWidth().padding(vertical=1.dp)) {
                                            Row(Modifier.heightIn(min=48.dp).padding(horizontal=10.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(10.dp)) {
                                                MobileIcon(if(isVoice) Glyph.Speaker else Glyph.Hash,tint=if(selected) Color.White else MobileMuted)
                                                Text(channel.channelLabel(),fontSize=16.sp,color=if(selected) Color.White else MobileMuted,maxLines=1,overflow=TextOverflow.Ellipsis,modifier=Modifier.weight(1f))
                                                if(channel.optString("id") in s.unreadChannels) Box(Modifier.size(7.dp).clip(CircleShape).background(Color.White))
                                            }
                                        }
                                    }
                                }
                            }
                            if(sections.isEmpty()&&!s.loading) item {HomeEmpty(if(query.isBlank()) "チャンネルがありません" else "チャンネルが見つかりません",if(query.isBlank()) "サーバーを切り替えてください" else "別の名前で検索してください")}
                        }
                    }
                }
            }
        }
        Surface(shape=RoundedCornerShape(22.dp),color=MobileRaised,border=BorderStroke(1.dp,Color.White.copy(alpha=.05f)),modifier=Modifier.padding(8.dp)) {
            Row(Modifier.fillMaxWidth().heightIn(min=60.dp),verticalAlignment=Alignment.CenterVertically) {
                Row(Modifier.weight(1f).clickable(onClick=onAccount).padding(10.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(10.dp)) {
                    MobileAvatar(s.me ?: JSONObject(),38.dp)
                    Column(Modifier.weight(1f)) {
                        Text(s.me?.label().orEmpty(),fontWeight=FontWeight.SemiBold,fontSize=15.sp,maxLines=1,overflow=TextOverflow.Ellipsis)
                        Text(if(s.connection=="接続済み") "接続済み" else s.connection,color=MobileMuted,fontSize=11.sp,maxLines=1)
                    }
                    MobileIcon(Glyph.Down,modifier=Modifier.size(16.dp),tint=MobileMuted)
                }
                MobileIconButton(Glyph.Bell,if(s.persistent) "常駐通知を停止" else "常駐通知を有効にする",onNotifications,tint=if(s.persistent) MobileAccent else MobileMuted)
            }
        }
    }
    if(newDm) AlertDialog(onDismissRequest={newDm=false},title={Text("新しいメッセージ")},text={Column(verticalArrangement=Arrangement.spacedBy(12.dp)) {
        Text("相手のユーザーIDを入力してください。",color=MobileMuted,fontSize=14.sp)
        OutlinedTextField(recipient,{recipient=it.filter(Char::isDigit)},label={Text("ユーザーID")},singleLine=true,keyboardOptions=KeyboardOptions(keyboardType=KeyboardType.Number),modifier=Modifier.fillMaxWidth())
    }},confirmButton={TextButton(onClick={model.createDm(recipient);newDm=false},enabled=recipient.isNotBlank()) {Text("DMを開く")}},dismissButton={TextButton(onClick={newDm=false}) {Text("キャンセル")}})
    voice?.let {channel->ModalBottomSheet(onDismissRequest={voice=null},containerColor=MobileRaised) {
        Column(Modifier.fillMaxWidth().padding(horizontal=24.dp).padding(bottom=24.dp),verticalArrangement=Arrangement.spacedBy(16.dp)) {
            MobileIcon(Glyph.Speaker,modifier=Modifier.size(32.dp),tint=MobileAccent)
            Text(channel.channelLabel(),style=MaterialTheme.typography.headlineSmall,fontWeight=FontWeight.Bold)
            Text(s.guild?.label().orEmpty(),color=MobileMuted)
            Button(onClick={onVoice(channel,s.guild?.optString("id"));voice=null},modifier=Modifier.fillMaxWidth().height(52.dp)) {Text("通話に参加する")}
        }
    }}
}
@Composable private fun ServerRail(s:ChatState,onSelect:(JSONObject?)->Unit) {
    LazyColumn(Modifier.width(72.dp).fillMaxHeight(),contentPadding=PaddingValues(top=8.dp,bottom=12.dp),verticalArrangement=Arrangement.spacedBy(10.dp),horizontalAlignment=Alignment.CenterHorizontally) {
        item("home") {RailItem("メッセージ",s.guild==null,{onSelect(null)}) {MobileIcon(Glyph.Chat,modifier=Modifier.size(27.dp),tint=if(s.guild==null) Color.White else MobileMuted)}}
        item("divider") {HorizontalDivider(Modifier.width(40.dp),color=Color.White.copy(alpha=.1f))}
        items(s.guilds,key={it.optString("id")}) {guild->
            RailItem(guild.label(),s.guild?.optString("id")==guild.optString("id"),{onSelect(guild)}) {
                val icon=guild.nonNull("icon")
                if(icon!=null) AsyncImage("https://cdn.discordapp.com/icons/${guild.optString("id")}/$icon.png?size=128",null,Modifier.fillMaxSize(),contentScale=ContentScale.Crop)
                else Text(guild.label().take(3),Modifier.padding(4.dp),fontSize=12.sp,fontWeight=FontWeight.SemiBold,maxLines=1)
            }
        }
    }
}
@Composable private fun RailItem(label:String,selected:Boolean,onClick:()->Unit,content:@Composable ()->Unit) {
    Box(Modifier.fillMaxWidth(),contentAlignment=Alignment.Center) {
        if(selected) Box(Modifier.align(Alignment.CenterStart).width(4.dp).height(32.dp).clip(RoundedCornerShape(topEnd=4.dp,bottomEnd=4.dp)).background(Color.White))
        Surface(onClick=onClick,shape=RoundedCornerShape(if(selected) 16.dp else 23.dp),color=if(selected) MobileAccent else MobileRaised,modifier=Modifier.size(48.dp).semantics {contentDescription=label;this.selected=selected}) {Box(contentAlignment=Alignment.Center) {content()}}
    }
}
@Composable private fun DmRow(dm:JSONObject,s:ChatState,onClick:()->Unit) {
    val preview=s.previews[dm.optString("id")]
    val unread=dm.optString("id") in s.unreadChannels
    val subtitle=preview?.let {displayContent(it).ifBlank {if(it.optJSONArray("attachments")?.length()?.let {n->n>0}==true) "添付ファイル" else "メッセージ"}} ?: if(dm.optInt("type")==3) "グループDM" else channelUser(dm).nonNull("username")?.let {"@$it"}.orEmpty()
    Surface(onClick=onClick,shape=RoundedCornerShape(14.dp),color=if(s.lastChannelId==dm.optString("id")) MobileRaised else Color.Transparent,modifier=Modifier.fillMaxWidth()) {
        Row(Modifier.padding(horizontal=8.dp,vertical=12.dp),verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(12.dp)) {
            MobileAvatar(if(dm.optInt("type")==3) dm else channelUser(dm),40.dp,dm.optInt("type")==3)
            Column(Modifier.weight(1f),verticalArrangement=Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment=Alignment.CenterVertically,horizontalArrangement=Arrangement.spacedBy(6.dp)) {
                    Text(dm.channelLabel(),Modifier.weight(1f),fontSize=15.sp,fontWeight=if(unread) FontWeight.Bold else FontWeight.Medium,maxLines=1,overflow=TextOverflow.Ellipsis)
                    Text(dmTime(dm),fontSize=10.sp,color=MobileMuted)
                    if(unread) Box(Modifier.size(6.dp).clip(CircleShape).background(Color.White))
                }
                Text(subtitle,fontSize=12.sp,color=MobileMuted,maxLines=1,overflow=TextOverflow.Ellipsis)
            }
        }
    }
}
@Composable private fun HomeEmpty(title:String,subtitle:String) {
    Column(Modifier.fillMaxWidth().padding(horizontal=12.dp,vertical=36.dp),horizontalAlignment=Alignment.CenterHorizontally,verticalArrangement=Arrangement.spacedBy(12.dp)) {
        MobileIcon(Glyph.Chat,modifier=Modifier.size(36.dp),tint=MobileMuted)
        Text(title,fontWeight=FontWeight.SemiBold,fontSize=16.sp)
        Text(subtitle,color=MobileMuted,fontSize=13.sp)
    }
}
