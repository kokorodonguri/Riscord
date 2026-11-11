package io.github.kokorodonguri.riscord

import android.graphics.Bitmap
import android.net.Uri
import androidx.compose.foundation.layout.*
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.createAndroidComposeRule
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.SystemBarStyle
import android.view.WindowManager
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.*
import org.junit.runner.RunWith
import java.io.File

/** UI tests use an isolated model and a fake REST transport. They never send to Discord. */
@RunWith(AndroidJUnit4::class)
class MobileUxTest {
    @get:Rule val compose=createAndroidComposeRule<ComponentActivity>()
    private val app=InstrumentationRegistry.getInstrumentation().targetContext.applicationContext as RiscordApplication
    private val store=ViewModelStore()
    private lateinit var model:ChatModel
    private var fixtureMessages=listOf<JSONObject>()
    private val dm=JSONObject("""{"id":"100","type":1,"recipients":[{"id":"2","username":"mika","global_name":"Mika"}]}""")
    private val me=JSONObject("""{"id":"1","username":"riscord","global_name":"テストユーザー"}""")
    private fun message(index:Int,text:String="過去 $index",author:String="2"):JSONObject=JSONObject()
        .put("id",(1000+index).toString()).put("channel_id","100").put("content",text)
        .put("timestamp","2026-11-11T09:${(index/60).toString().padStart(2,'0')}:${(index%60).toString().padStart(2,'0')}Z")
        .put("author",JSONObject().put("id",author).put("username",if(author=="1") "自分" else "Mika"))
    @Before fun setup() {
        fixtureMessages=listOf(message(1,"テストのメッセージ"),message(2,"自分のメッセージ","1"))
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            compose.activity.enableEdgeToEdge(statusBarStyle=SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),navigationBarStyle=SystemBarStyle.dark(android.graphics.Color.TRANSPARENT))
            compose.activity.window.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
            model=ChatModel(app,restoreSession=false,apiOverride={path,method,_->
                check(method=="GET") {"UI test attempted a network write: $method $path"}
                when {
                    path.contains("/messages")->JSONArray().apply {fixtureMessages.asReversed().forEach {put(it)}}
                    path.contains("/channels")->JSONArray()
                    else->JSONObject()
                }
            })
            store.put("isolated-ui",model)
            model.state.value=ChatState(me=me,dms=listOf(dm),connection="接続済み",hasMore=false)
        }
    }
    @After fun teardown() {InstrumentationRegistry.getInstrumentation().runOnMainSync {store.clear()}}
    private fun render(compact:Boolean=false) {
        compose.setContent {RiscordTheme {
            if(compact) {
                val density=LocalDensity.current
                CompositionLocalProvider(LocalDensity provides Density(density.density,1.4f)) {
                    Box(Modifier.width(320.dp).fillMaxHeight()) {Riscord(model)}
                }
            } else Riscord(model)
        }}
    }
    private fun open() {compose.onNodeWithText("Mika").performClick();compose.waitForIdle()}
    private fun screenshot(name:String) {
        val directory=File(app.getExternalFilesDir(null),"ui-validation").apply {mkdirs()}
        compose.waitForIdle()
        val bitmap=requireNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
        File(directory,"$name.png").outputStream().use {bitmap.compress(Bitmap.CompressFormat.PNG,100,it)}
        bitmap.recycle()
    }
    @Test fun draftReplyEditAndBackRemainInTheSameConversation() {
        render();open()
        compose.onNodeWithContentDescription("メッセージ入力").performTextInput("下書きを残す")
        compose.onNodeWithText("テストのメッセージ").performTouchInput {longClick()}
        compose.onNodeWithText("返信",useUnmergedTree=true).performClick()
        compose.onNodeWithText("Mika に返信").assertIsDisplayed()
        compose.onNodeWithContentDescription("返信を解除").performClick()
        compose.onNodeWithText("自分のメッセージ").performTouchInput {longClick()}
        compose.onNodeWithText("編集",useUnmergedTree=true).performClick()
        compose.onNodeWithContentDescription("メッセージ入力").assertTextContains("自分のメッセージ")
        compose.onNodeWithContentDescription("編集をキャンセル").performClick()
        compose.onNodeWithContentDescription("メッセージ入力").assertTextContains("下書きを残す")
        screenshot("reply-edit-draft")
        compose.onNodeWithContentDescription("チャンネル一覧に戻る").performClick()
        open()
        compose.onNodeWithContentDescription("メッセージ入力").assertTextContains("下書きを残す")
    }
    @Test fun newMessagesAndOlderHistoryDoNotMoveTheReader() {
        fixtureMessages=(51..100).map {message(it)}
        render();open()
        compose.onNodeWithText("過去 100").assertIsDisplayed()
        compose.onNodeWithContentDescription("メッセージ履歴").performScrollToNode(hasText("過去 70"))
        val before=compose.onNodeWithText("過去 70").assertIsDisplayed().fetchSemanticsNode().boundsInRoot.top
        compose.runOnIdle {fixtureMessages=(1..50).map {message(it)};model.older()}
        compose.waitUntil(5000) {model.state.value.messages.size==100&&!model.state.value.loading}
        val after=compose.onNodeWithText("過去 70").assertIsDisplayed().fetchSemanticsNode().boundsInRoot.top
        Assert.assertEquals(before,after,2f)
        compose.onNodeWithContentDescription("チャンネル一覧に戻る").performClick()
        compose.runOnIdle {fixtureMessages=(51..100).map {message(it)}}
        open()
        compose.onNodeWithText("過去 70").assertIsDisplayed()
        compose.runOnIdle {model.state.value=model.state.value.copy(messages=model.state.value.messages+message(101,"新しいメッセージ"))}
        compose.waitForIdle()
        compose.onNodeWithText("過去 70").assertIsDisplayed()
        screenshot("reading-history")
        compose.onNodeWithContentDescription("最新のメッセージへ").performClick()
        compose.waitForIdle()
        compose.onNodeWithText("新しいメッセージ").assertIsDisplayed()
        compose.runOnIdle {model.state.value=model.state.value.copy(messages=model.state.value.messages+message(102,"さらに新着"))}
        compose.waitForIdle()
        compose.onNodeWithText("さらに新着").assertIsDisplayed()
    }
    @Test fun attachmentIsStagedAndCanBeCancelledWithoutSending() {
        render();open()
        val file=File(app.cacheDir,"mobile-ux-attachment.txt").apply {writeText("attachment fixture")}
        try {
            compose.runOnIdle {model.stageAttachment(Uri.fromFile(file))}
            compose.waitUntil(5000) {model.state.value.attachments.isNotEmpty()}
            compose.onNodeWithText("送信待ち",substring=true).assertIsDisplayed()
            compose.onNodeWithContentDescription("メッセージを送信").assertIsEnabled()
            screenshot("staged-attachment")
            compose.onNodeWithContentDescription("添付を取り消す").performClick()
            compose.onNodeWithContentDescription("メッセージを送信").assertDoesNotExist()
            Assert.assertTrue(model.state.value.attachments.isEmpty())
        } finally {file.delete()}
    }
    @Test fun serverCategoriesSearchAndVoiceConfirmationWork() {
        val guild=JSONObject("""{"id":"9","name":"テストサーバー"}""")
        val channels=JSONArray("""[
            {"id":"10","name":"テキスト","type":4,"position":0},
            {"id":"11","name":"雑談","type":0,"parent_id":"10","position":1},
            {"id":"12","name":"お知らせ","type":0,"parent_id":"10","position":0},
            {"id":"13","name":"作業部屋","type":2,"position":0}
        ]""").objects()
        compose.runOnUiThread {model.state.value=model.state.value.copy(guild=guild,guilds=listOf(guild),channels=channels)}
        render()
        compose.onNodeWithText("テキスト").performClick()
        compose.onNodeWithText("雑談").assertDoesNotExist()
        compose.onNodeWithContentDescription("チャンネルを検索").performTextInput("雑談")
        compose.onNode(hasText("雑談") and hasSetTextAction().not()).assertIsDisplayed()
        compose.onNodeWithText("作業部屋").assertDoesNotExist()
        compose.onNodeWithContentDescription("検索をクリア").performClick()
        compose.onNodeWithText("作業部屋").performClick()
        compose.onNodeWithText("通話に参加する").assertIsDisplayed()
        Assert.assertNull(model.state.value.voiceChannel)
        screenshot("voice-confirmation")
    }
    @Test fun multipleImagesShareARowInsteadOfFillingTheWholeScreen() {
        val files=listOf("first.png","second.png").mapIndexed {index,name->File(app.cacheDir,name).apply {
            val bitmap=Bitmap.createBitmap(32,24,Bitmap.Config.ARGB_8888)
            bitmap.eraseColor(if(index==0) android.graphics.Color.BLUE else android.graphics.Color.CYAN)
            outputStream().use {bitmap.compress(Bitmap.CompressFormat.PNG,100,it)}
            bitmap.recycle()
        }}
        try {
            val images=JSONArray().apply {files.forEach {put(JSONObject().put("filename",it.name).put("content_type","image/png").put("url",Uri.fromFile(it).toString()).put("width",32).put("height",24))}}
            fixtureMessages=listOf(message(1,"写真をまとめて表示").put("attachments",images))
            render();open()
            val first=compose.onNodeWithContentDescription("first.png").assertIsDisplayed().fetchSemanticsNode().boundsInRoot
            val second=compose.onNodeWithContentDescription("second.png").assertIsDisplayed().fetchSemanticsNode().boundsInRoot
            Assert.assertEquals(first.top,second.top,2f)
            Assert.assertTrue(first.right<=second.left)
            screenshot("image-grid")
        } finally {files.forEach {it.delete()}}
    }
    @Test fun narrowScreenAndLargeTextKeepComposerActionsReachable() {
        render(compact=true);open()
        compose.onNodeWithContentDescription("メッセージ入力").performTextInput("長い文章でも送信ボタンを隠さないようにします。\n二行目\n三行目\n四行目\n五行目\n六行目")
        compose.onNodeWithContentDescription("チャンネル一覧に戻る").assertIsDisplayed()
        compose.onNodeWithContentDescription("メッセージを送信").assertIsDisplayed()
        compose.onNodeWithContentDescription("ファイルを添付").assertIsDisplayed()
        screenshot("narrow-large-text")
    }
}
