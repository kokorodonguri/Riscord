package io.github.kokorodonguri.riscord

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.*
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.PathParser
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.*
import coil.compose.AsyncImage
import org.json.JSONObject
import java.time.*
import java.time.format.DateTimeFormatter

internal val MobileBackground=Color(0xff0c0d10)
internal val MobilePanel=Color(0xff111216)
internal val MobileRaised=Color(0xff1d1e24)
internal val MobileMuted=Color(0xff999ca8)
internal val MobileAccent=Color(0xff7581ff)

internal enum class Glyph(val path:String) {
    Back("M15 5L8 12L15 19M8 12H21"),
    Search("M10.5 3a7.5 7.5 0 1 0 0 15a7.5 7.5 0 1 0 0 -15M16 16L22 22"),
    Plus("M12 4V20M4 12H20"),
    Close("M6 6L18 18M18 6L6 18"),
    Down("M6 9L12 15L18 9"),
    Right("M9 6L15 12L9 18"),
    Chat("M5 4H19Q21 4 21 6V15Q21 17 19 17H9L3 21V6Q3 4 5 4Z"),
    Hash("M9 3L6 21M18 3L15 21M4 9H22M2 15H20"),
    Speaker("M3 9H7L12 5V19L7 15H3ZM16 8Q20 12 16 16M19 5Q26 12 19 19"),
    Phone("M5 3L9 3L11 8L8 10Q10 14 14 16L16 13L21 15V19Q21 22 18 21Q5 18 3 6Q2 3 5 3Z"),
    Send("M3 3L22 12L3 21L7 12ZM7 12H22"),
    More("M12 4V5M12 11V12M12 18V19"),
    Bell("M5 17H19L17 14V9Q17 4 12 4Q7 4 7 9V14ZM10 21H14M12 2V4"),
    Settings("M9 3H15L16 6L19 7L22 11L20 14L20 18L16 21L12 20L8 21L4 18V14L2 11L5 7L8 6ZM12 8a4 4 0 1 0 0 8a4 4 0 1 0 0 -8"),
    Reply("M10 5L3 11L10 17V13Q18 12 21 20Q22 8 10 9Z"),
    Copy("M8 8H21V21H8ZM16 8V3H3V16H8"),
    Edit("M4 17L16 5L20 9L8 21H4ZM14 7L18 11M16 5L18 3L22 7L20 9"),
    Trash("M3 6H21M9 6V3H15V6M5 6L6 21H18L19 6M10 10V17M14 10V17"),
    Smile("M12 2a10 10 0 1 0 0 20a10 10 0 1 0 0 -20M8 9H8.1M16 9H16.1M7 14Q12 20 17 14"),
    File("M5 2H14L20 8V22H5ZM14 2V8H20M8 13H17M8 17H15"),
    Check("M4 12L9 17L20 6"),
    Mic("M9 4Q12 0 15 4V11Q12 15 9 11ZM5 10Q5 18 12 18Q19 18 19 10M12 18V22M8 22H16"),
    Mute("M9 4Q12 0 15 4V11M5 10Q5 18 12 18Q16 18 18 15M12 18V22M8 22H16M2 2L22 22"),
    Logout("M10 3H3V21H10M8 12H22M17 7L22 12L17 17"),
    Download("M12 2V15M7 10L12 15L17 10M4 16V22H20V16")
}
private val vectors=mutableMapOf<Glyph,ImageVector>()
@Composable internal fun MobileIcon(glyph:Glyph, description:String?=null, modifier:Modifier=Modifier.size(22.dp), tint:Color=LocalContentColor.current) {
    val vector=vectors.getOrPut(glyph) {
        ImageVector.Builder(glyph.name,24.dp,24.dp,24f,24f).addPath(
            pathData=PathParser().parsePathString(glyph.path).toNodes(),
            fill=null,stroke=SolidColor(Color.Black),strokeLineWidth=1.8f,
            strokeLineCap=StrokeCap.Round,strokeLineJoin=StrokeJoin.Round
        ).build()
    }
    Icon(vector,description,modifier,tint)
}
@Composable internal fun MobileIconButton(glyph:Glyph,description:String,onClick:()->Unit,modifier:Modifier=Modifier,enabled:Boolean=true,tint:Color=LocalContentColor.current) {
    IconButton(onClick,modifier.sizeIn(minWidth=48.dp,minHeight=48.dp),enabled=enabled) {MobileIcon(glyph,description,tint=tint)}
}
/** URL を外部アプリで開く。開けるアプリがなければ知らせるだけにする */
internal fun openUrl(context:android.content.Context,url:String) {
    try { context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW,android.net.Uri.parse(url))) }
    catch(_:android.content.ActivityNotFoundException) { android.widget.Toast.makeText(context,tr("開けるアプリがありません","No app can open this"),android.widget.Toast.LENGTH_SHORT).show() }
}
internal fun JSONObject.nonNull(key:String):String? = optString(key).takeIf {it.isNotBlank()&&it!="null"}
internal fun channelUser(channel:JSONObject):JSONObject = channel.optJSONArray("recipients")?.optJSONObject(0) ?: channel
/**
 * 添付画像の表示用 URL。Discord のメディアプロキシに縮小版を頼み、原寸（数 MB）を落とさない。
 * 画面の幅（maxPx）に収まる大きさで取るので見た目は変わらない。GIF はアニメを残すため、
 * 大きさが分からないものは縮めずにそのまま返す（デスクトップ版の app::resized と同じ方針）
 */
internal fun previewUrl(proxy:String?,url:String,width:Int,height:Int,maxPx:Int):String {
    val base=proxy?.takeIf {it.contains("media.discordapp.net")} ?: return url
    val path=base.substringBefore('?').substringBefore('#').lowercase()
    if(path.endsWith(".gif")||width<=0||height<=0||maxPx<=0) return base
    val scale=minOf(1.0,maxPx.toDouble()/maxOf(width,height))
    val (w,h)=maxOf(1,(width*scale).toInt()) to maxOf(1,(height*scale).toInt())
    // WebP は animated=true がないとプロキシが 1 枚目だけにする
    val anim=if(path.endsWith(".webp")) "&animated=true" else ""
    return "$base${if('?' in base) '&' else '?'}width=$w&height=$h$anim"
}
internal fun avatarUrl(user:JSONObject):String? = user.nonNull("avatar")?.let { "https://cdn.discordapp.com/avatars/${user.optString("id")}/$it.png?size=128" }
@Composable internal fun MobileAvatar(user:JSONObject,size:Dp=40.dp,group:Boolean=false) {
    val url=if(group) user.nonNull("icon")?.let {"https://cdn.discordapp.com/channel-icons/${user.optString("id")}/$it.png?size=128"} else avatarUrl(user)
    val initials=user.label().take(2)
    Box(Modifier.size(size).clip(CircleShape).background(Color(0xff303448)),contentAlignment=Alignment.Center) {
        Text(initials,color=Color.White,fontSize=(size.value*.34).sp,fontWeight=FontWeight.SemiBold,maxLines=1)
        if(url!=null) AsyncImage(url,null,Modifier.fillMaxSize(),contentScale=ContentScale.Crop)
    }
}
internal fun messageInstant(message:JSONObject):Instant?=runCatching {Instant.parse(message.optString("timestamp"))}.getOrNull()
internal fun messageDay(message:JSONObject):LocalDate?=messageInstant(message)?.atZone(ZoneId.systemDefault())?.toLocalDate()
internal fun messageTime(message:JSONObject):String=messageInstant(message)?.atZone(ZoneId.systemDefault())?.format(DateTimeFormatter.ofPattern("HH:mm")) ?: ""
internal fun messageDate(message:JSONObject):String {
    val date=messageDay(message) ?: return ""
    return when(date) {LocalDate.now()->tr("今日","Today");LocalDate.now().minusDays(1)->tr("昨日","Yesterday");else->date.format(if(isEnglish) DateTimeFormatter.ofPattern("MMM d, yyyy",java.util.Locale.ENGLISH) else DateTimeFormatter.ofPattern("yyyy年M月d日"))}
}
internal fun dmTime(channel:JSONObject):String {
    val snowflake=channel.optString("last_message_id").toLongOrNull() ?: return ""
    val date=Instant.ofEpochMilli((snowflake shr 22)+1420070400000L).atZone(ZoneId.systemDefault())
    return if(date.toLocalDate()==LocalDate.now()) date.format(DateTimeFormatter.ofPattern("HH:mm")) else date.format(DateTimeFormatter.ofPattern("M/d"))
}
internal fun displayContent(message:JSONObject):String {
    val users=message.optJSONArray("mentions")?.objects()?.associateBy {it.optString("id")}.orEmpty()
    return message.optString("content")
        .replace(Regex("<@!?(\\d+)>")) {"@"+(users[it.groupValues[1]]?.label() ?: it.groupValues[1])}
        .replace(Regex("<a?:([A-Za-z0-9_]+):\\d+>")) {":${it.groupValues[1]}:"}
}
