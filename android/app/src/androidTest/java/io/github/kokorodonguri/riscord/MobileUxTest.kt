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
    private var restHandler:(suspend (String,String,JSONObject?)->Any?)?=null
    private val typingRequests=mutableListOf<String>()
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
            model=ChatModel(app,restoreSession=false,apiOverride={path,method,body->
                restHandler?.let {return@ChatModel it(path,method,body)}
                if(method=="POST"&&path.endsWith("/typing")) {typingRequests+=path;return@ChatModel JSONObject.NULL}
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
    @Test fun olderHomeRefreshCannotOverwriteANewerResult() {
        val oldGuilds=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val oldDms=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        var guildCalls=0;var dmCalls=0
        restHandler={path,_,_->when(path) {
            "/users/@me/guilds" -> if(++guildCalls==1) oldGuilds.await() else JSONArray().put(JSONObject().put("id","20").put("name","新しいサーバー"))
            "/users/@me/channels" -> if(++dmCalls==1) oldDms.await() else JSONArray().put(JSONObject().put("id","200").put("name","新しいDM"))
            else -> error("unexpected request: $path")
        }}
        render()
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","RESYNC"))}
        compose.waitUntil(5000) {guildCalls==1 && dmCalls==1}
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","RESYNC"))}
        compose.waitUntil(5000) {model.state.value.guilds.singleOrNull()?.optString("id")=="20"}
        oldGuilds.complete(JSONArray().put(JSONObject().put("id","10").put("name","古いサーバー")))
        oldDms.complete(JSONArray().put(JSONObject().put("id","100").put("name","古いDM")))
        compose.waitForIdle()
        Assert.assertEquals("20",model.state.value.guilds.single().optString("id"))
        Assert.assertEquals("200",model.state.value.dms.single().optString("id"))
        compose.onNodeWithContentDescription("新しいサーバー").assertIsDisplayed()
        compose.onNodeWithText("新しいDM").assertIsDisplayed()
        screenshot("home-refresh")
    }
    @Test fun homeRefreshFromAnotherAccountIsDiscarded() {
        val guilds=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val dms=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        var requests=0
        restHandler={path,_,_->requests++;when(path) {
            "/users/@me/guilds" -> guilds.await()
            "/users/@me/channels" -> dms.await()
            else -> error("unexpected request: $path")
        }}
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","RESYNC"))}
        compose.waitUntil(5000) {requests==2}
        compose.runOnUiThread {model.state.value=ChatState(me=JSONObject().put("id","2"))}
        guilds.complete(JSONArray().put(JSONObject().put("id","10")))
        dms.complete(JSONArray().put(JSONObject().put("id","100")))
        compose.waitForIdle()
        Assert.assertTrue(model.state.value.guilds.isEmpty())
        Assert.assertTrue(model.state.value.dms.isEmpty())
    }
    @Test fun anOlderFailedHomeRefreshDoesNotShowAnErrorAfterSuccess() {
        val old=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        var calls=0
        restHandler={path,_,_->
            if(path=="/users/@me/guilds") {
                if(++calls==1) old.await() else JSONArray().put(JSONObject().put("id","20"))
            } else JSONArray()
        }
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","RESYNC"))}
        compose.waitUntil(5000) {calls==1}
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","RESYNC"))}
        compose.waitUntil(5000) {model.state.value.guilds.singleOrNull()?.optString("id")=="20"}
        old.completeExceptionally(java.io.IOException("old request failed"))
        compose.waitForIdle()
        Assert.assertNull(model.state.value.error)
    }
    @Test fun currentHomeRefreshFailureIsReported() {
        restHandler={_,_,_->throw java.io.IOException("offline")}
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","RESYNC"))}
        compose.waitUntil(5000) {model.state.value.error!=null}
        Assert.assertTrue(model.state.value.error!!.contains("再試行"))
    }
    @Test fun olderChannelUpdateCannotOverwriteANewerResult() {
        val old=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val guild=JSONObject().put("id","9").put("name","サーバー")
        var calls=0
        restHandler={path,_,_->
            check(path=="/guilds/9/channels")
            when(++calls) {
                1 -> JSONArray().put(JSONObject().put("id","1").put("name","初期"))
                2 -> old.await()
                else -> JSONArray().put(JSONObject().put("id","3").put("name","新しい"))
            }
        }
        compose.runOnUiThread {model.selectGuild(guild)}
        compose.waitUntil(5000) {!model.state.value.loading}
        val event=JSONObject().put("t","CHANNEL_UPDATE").put("d",JSONObject().put("guild_id","9"))
        compose.runOnUiThread {model.receiveEvent(event)}
        compose.waitUntil(5000) {calls==2}
        compose.runOnUiThread {model.receiveEvent(event)}
        compose.waitUntil(5000) {model.state.value.channels.singleOrNull()?.optString("id")=="3"}
        old.complete(JSONArray().put(JSONObject().put("id","2").put("name","古い")))
        compose.waitForIdle()
        Assert.assertEquals("3",model.state.value.channels.single().optString("id"))
    }
    @Test fun failedChannelUpdateReleasesTheInitialLoadingState() {
        val initial=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val guild=JSONObject().put("id","9").put("name","サーバー")
        var calls=0
        restHandler={_,_,_->if(++calls==1) initial.await() else throw java.io.IOException("offline")}
        compose.runOnUiThread {model.selectGuild(guild)}
        compose.waitUntil(5000) {calls==1 && model.state.value.loading}
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","CHANNEL_UPDATE").put("d",JSONObject().put("guild_id","9")))}
        compose.waitUntil(5000) {calls==2 && !model.state.value.loading}
        Assert.assertTrue(model.state.value.channelLoadFailed)
        initial.complete(JSONArray().put(JSONObject().put("id","1")))
        compose.waitForIdle()
        Assert.assertTrue(model.state.value.channels.isEmpty())
    }
    @Test fun channelLoadFailureShowsRetryInsteadOfAnEmptyServer() {
        val retry=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val guild=JSONObject().put("id","9").put("name","サーバー")
        var calls=0
        restHandler={path,_,_->
            check(path=="/guilds/9/channels")
            if(++calls==1) throw java.io.IOException("offline") else retry.await()
        }
        render()
        compose.runOnUiThread {model.selectGuild(guild)}
        compose.waitUntil(5000) {calls==1 && !model.state.value.loading}
        screenshot("channel-load-failure-before")
        compose.onNodeWithText("チャンネルがありません").assertDoesNotExist()
        compose.onNodeWithText("チャンネルを読み込めませんでした").assertIsDisplayed()
        screenshot("channel-load-failure-after")
        compose.onNodeWithText("再読み込み").performClick()
        compose.waitUntil(5000) {calls==2 && model.state.value.loading}
        compose.onNodeWithText("再読み込み").assertDoesNotExist()
        retry.complete(JSONArray().put(JSONObject().put("id","11").put("name","雑談").put("type",0)))
        compose.waitUntil(5000) {!model.state.value.loading}
        compose.onNodeWithText("雑談").assertIsDisplayed()
    }
    @Test fun anEmptyChannelListIsNotShownAsAnError() {
        restHandler={_,_,_->JSONArray()}
        render()
        compose.runOnUiThread {model.selectGuild(JSONObject().put("id","9").put("name","サーバー"))}
        compose.waitUntil(5000) {!model.state.value.loading}
        compose.onNodeWithText("チャンネルがありません").assertIsDisplayed()
        compose.onNodeWithText("再読み込み").assertDoesNotExist()
    }
    @Test fun failedChannelRefreshKeepsTheCachedListAndCanRetry() {
        val retry=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        var calls=0
        restHandler={_,_,_->when(++calls) {
            1 -> JSONArray().put(JSONObject().put("id","11").put("name","元のチャンネル").put("type",0))
            2 -> throw java.io.IOException("offline")
            else -> retry.await()
        }}
        render()
        compose.runOnUiThread {model.selectGuild(JSONObject().put("id","9").put("name","サーバー"))}
        compose.waitUntil(5000) {model.state.value.channels.isNotEmpty() && !model.state.value.loading}
        compose.runOnUiThread {model.receiveEvent(JSONObject().put("t","CHANNEL_UPDATE").put("d",JSONObject().put("guild_id","9")))}
        compose.waitUntil(5000) {model.state.value.channelLoadFailed}
        compose.onNodeWithText("元のチャンネル").assertIsDisplayed()
        compose.onNodeWithText("チャンネルを再読み込み").performClick()
        compose.waitUntil(5000) {calls==3 && model.state.value.loading}
        compose.onNodeWithText("チャンネルを再読み込み").assertDoesNotExist()
        retry.complete(JSONArray().put(JSONObject().put("id","12").put("name","更新後").put("type",0)))
        compose.waitUntil(5000) {!model.state.value.loading}
        compose.onNodeWithText("更新後").assertIsDisplayed()
        compose.onNodeWithText("元のチャンネル").assertDoesNotExist()
        Assert.assertNull(model.state.value.error)
    }
    @Test fun switchingToDmCancelsGuildLoadingAndIgnoresTheOldResult() {
        val gate=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val guild=JSONObject("""{"id":"9","name":"遅いサーバー"}""")
        restHandler={_,_,_->gate.await()}
        compose.runOnUiThread {model.selectGuild(guild)}
        compose.waitUntil(5000) {model.state.value.loading}
        compose.runOnUiThread {model.selectGuild(null)}
        Assert.assertFalse(model.state.value.loading)
        gate.complete(JSONArray().put(JSONObject().put("id","11").put("name","古いチャンネル")))
        compose.waitForIdle()
        Assert.assertNull(model.state.value.guild)
        Assert.assertTrue(model.state.value.channels.isEmpty())
        Assert.assertNull(model.state.value.error)
    }
    @Test fun returningFromAChannelWaitsForTheOriginalGuildList() {
        val gate=kotlinx.coroutines.CompletableDeferred<JSONArray>()
        val guild=JSONObject("""{"id":"9","name":"サーバー"}""")
        val channel=JSONObject("""{"id":"11","name":"一般","type":0}""")
        restHandler={path,_,_->if(path=="/guilds/9/channels") gate.await() else JSONArray()}
        compose.runOnUiThread {model.selectGuild(guild);model.openChannel(channel);model.back()}
        Assert.assertTrue(model.state.value.loading)
        gate.complete(JSONArray().put(channel))
        compose.waitUntil(5000) {!model.state.value.loading}
        Assert.assertEquals("11",model.state.value.channels.single().optString("id"))
        Assert.assertNull(model.state.value.error)
    }
    @Test fun attachmentIsKeptWithTheChannelThatOpenedThePicker() {
        val file=File(app.cacheDir,"picker-origin.txt").apply {writeText("fixture")}
        try {
            compose.runOnUiThread {
                model.state.value=model.state.value.copy(channel=dm)
                model.state.value=model.state.value.copy(channel=JSONObject().put("id","200"))
                model.stageAttachment(Uri.fromFile(file),"100")
            }
            compose.waitUntil(5000) {model.state.value.attachments.containsKey("100")}
            Assert.assertFalse(model.state.value.attachments.containsKey("200"))
        } finally {file.delete()}
    }
    @Test fun pickerResultFromAnotherAccountIsDiscarded() {
        val file=File(app.cacheDir,"picker-other-account.txt").apply {writeText("fixture")}
        try {
            compose.runOnUiThread {
                model.state.value=model.state.value.copy(me=JSONObject().put("id","2"),channel=dm)
                model.stageAttachment(Uri.fromFile(file),"100","1")
            }
            compose.waitForIdle()
            Assert.assertTrue(model.state.value.attachments.isEmpty())
        } finally {file.delete()}
    }
    @Test fun profileSaveRetainsPartialSuccessAndCanRetryWithoutDoubleSubmit() {
        val gate=kotlinx.coroutines.CompletableDeferred<JSONObject>()
        var names=0;var bios=0;var saved=0;var failBio=true
        compose.runOnUiThread {
            model.state.value=model.state.value.copy(profile=JSONObject().put("bio","元の紹介"))
            restHandler={path,method,_->
                check(method=="PATCH")
                when(path) {
                    "/users/@me"->{names++;gate.await()}
                    "/users/@me/profile"->{bios++;if(failBio) throw java.io.IOException("private technical error");JSONObject()}
                    else->error("unexpected request")
                }
            }
            model.saveProfile("新しい名前","新しい紹介","online") {saved++}
            model.saveProfile("新しい名前","新しい紹介","online") {saved++}
        }
        Assert.assertTrue(model.state.value.profileSaving)
        Assert.assertEquals(1,names)
        gate.complete(JSONObject().put("id","1").put("global_name","新しい名前"))
        compose.waitUntil(5000) {!model.state.value.profileSaving}
        Assert.assertEquals("新しい名前",model.state.value.me?.label())
        Assert.assertEquals("元の紹介",model.state.value.profile?.optString("bio"))
        Assert.assertTrue(model.state.value.profileError!!.startsWith("一部の変更"))
        Assert.assertTrue(model.state.value.profileError!!.contains("再試行"))
        Assert.assertFalse(model.state.value.profileError!!.contains("private"))
        Assert.assertEquals(0,saved)
        failBio=false
        compose.runOnUiThread {model.saveProfile("新しい名前","新しい紹介","online") {saved++}}
        compose.waitUntil(5000) {saved==1}
        Assert.assertEquals(1,names)
        Assert.assertEquals(2,bios)
        Assert.assertEquals("新しい紹介",model.state.value.profile?.optString("bio"))
        Assert.assertNull(model.state.value.profileError)
    }
    @Test fun incompleteProfileResponseShowsRetryInsteadOfAnEmptyForm() {
        restHandler={_,_,_->JSONObject()}
        compose.runOnUiThread {model.loadMyProfile()}
        compose.waitUntil(5000) {!model.state.value.profileLoading}
        Assert.assertNull(model.state.value.profile)
        Assert.assertTrue(model.state.value.profileError!!.contains("再試行"))
    }
    @Test fun profileSheetShowsSavingAndKeepsInputWhenTheRequestFails() {
        val gate=kotlinx.coroutines.CompletableDeferred<JSONObject>()
        var attempts=0
        restHandler={path,method,_->
            when {
                method=="GET" && path=="/users/1/profile" -> JSONObject().put("user_profile",JSONObject().put("bio","元の紹介"))
                method=="PATCH" && path=="/users/@me" -> {attempts++;if(attempts==1) gate.await() else JSONObject().put("id","1").put("global_name","新しい名前")}
                else -> error("unexpected request: $method $path")
            }
        }
        render()
        compose.onNodeWithText("テストユーザー").performClick()
        compose.onNodeWithText("プロフィールを編集").performClick()
        compose.waitUntil(5000) {model.state.value.profile!=null}
        compose.onNode(hasSetTextAction() and hasText("テストユーザー")).performTextReplacement("新しい名前")
        compose.onNodeWithText("保存").performClick()
        compose.onNodeWithText("保存中…").assertIsDisplayed().assertIsNotEnabled()
        gate.completeExceptionally(java.io.IOException("private diagnostic"))
        compose.waitUntil(5000) {!model.state.value.profileSaving}
        compose.onNodeWithText("プロフィールを保存できませんでした。内容を確認して再試行してください。").assertIsDisplayed()
        compose.onNode(hasSetTextAction() and hasText("新しい名前")).assertIsDisplayed()
        screenshot("profile-save-error")
        compose.onNodeWithText("保存").performClick()
        compose.waitUntil(5000) {model.state.value.me?.label()=="新しい名前"}
        Assert.assertEquals(2,attempts)
        compose.onNodeWithText("プロフィール").assertDoesNotExist()
    }
    @Test fun sendingDoesNotClearAReplySelectedWhileRequestIsPending() {
        val gate=kotlinx.coroutines.CompletableDeferred<JSONObject>()
        val first=message(1);val second=message(2)
        compose.runOnUiThread {
            model.state.value=model.state.value.copy(channel=dm,reply=first,drafts=mapOf("100" to "送る"))
            restHandler={_,method,_->check(method=="POST");gate.await()}
            model.send()
            model.reply(second)
        }
        Assert.assertTrue(model.state.value.sending)
        gate.complete(message(3,"送る","1"))
        compose.waitUntil(5000) {!model.state.value.sending}
        Assert.assertEquals(second.optString("id"),model.state.value.reply?.optString("id"))
    }
    @Test fun pendingSendDoesNotUpdateANewAccount() {
        val gate=kotlinx.coroutines.CompletableDeferred<JSONObject>()
        compose.runOnUiThread {
            model.state.value=model.state.value.copy(channel=dm,drafts=mapOf("100" to "送る"))
            restHandler={_,_,_->gate.await()}
            model.send()
            model.state.value=ChatState(me=JSONObject().put("id","2"),channel=dm,drafts=mapOf("100" to "新しい下書き"))
        }
        gate.complete(message(3,"送る","1"))
        compose.waitForIdle()
        Assert.assertEquals("新しい下書き",model.state.value.drafts["100"])
        Assert.assertTrue(model.state.value.messages.isEmpty())
    }
    @Test fun typingIsSentOnActualEditsAndThrottledWithinABurst() {
        render();open()
        Assert.assertTrue(typingRequests.isEmpty())
        compose.onNodeWithContentDescription("メッセージ入力").performTextInput("a")
        compose.onNodeWithContentDescription("メッセージ入力").performTextInput("b")
        compose.onNodeWithContentDescription("メッセージ入力").performTextClearance()
        compose.waitForIdle()
        Assert.assertEquals(listOf("/channels/100/typing"),typingRequests)
    }
    @Test fun typingNamesAreScopedToTheConversationAndExcludeMe() {
        render();open()
        compose.runOnIdle {model.receiveEvent(JSONObject("""{"t":"TYPING_START","d":{"channel_id":"100","user_id":"2"}}"""))}
        compose.onNodeWithText("Mika が入力中…").assertIsDisplayed()
        compose.runOnIdle {model.receiveEvent(JSONObject("""{"t":"TYPING_START","d":{"channel_id":"100","user_id":"1"}}"""))}
        compose.onNodeWithText("Mika が入力中…").assertIsDisplayed()
        compose.runOnIdle {model.receiveEvent(JSONObject("""{"t":"TYPING_START","d":{"channel_id":"100","user_id":"3","member":{"nick":"別の人"}}}"""))}
        compose.onNodeWithText("Mika、別の人 が入力中…").assertIsDisplayed()
        compose.runOnIdle {model.receiveEvent(JSONObject().put("t","MESSAGE_CREATE").put("d",message(3,"送信されたメッセージ")))}
        compose.onNodeWithText("別の人 が入力中…").assertIsDisplayed()
        compose.onNodeWithContentDescription("チャンネル一覧に戻る").performClick();open()
        Assert.assertNull(model.state.value.typing)
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
        compose.waitForIdle()
        Assert.assertEquals("下書きを残す",model.state.value.drafts["100"])
        compose.onNodeWithContentDescription("メッセージ入力").assertTextContains("下書きを残す")
        screenshot("reply-edit-draft")
        compose.onNodeWithContentDescription("チャンネル一覧に戻る").performClick()
        open()
        compose.onNodeWithContentDescription("メッセージ入力").assertTextContains("下書きを残す")
    }
    @Test fun newMessagesAndOlderHistoryDoNotMoveTheReader() {
        fun history(range:IntRange)=range.map {message(it,author=if(it%2==0) "1" else "2")}
        fixtureMessages=history(51..100)
        render();open()
        compose.onNodeWithText("過去 100").assertIsDisplayed()
        compose.onNodeWithContentDescription("メッセージ履歴").performScrollToNode(hasText("過去 60"))
        val before=compose.onNodeWithText("過去 60").assertIsDisplayed().fetchSemanticsNode().boundsInRoot.top
        compose.runOnIdle {fixtureMessages=history(1..50);model.older()}
        compose.waitUntil(5000) {model.state.value.messages.size==100&&!model.state.value.loading}
        val after=compose.onNodeWithText("過去 60").assertIsDisplayed().fetchSemanticsNode().boundsInRoot.top
        Assert.assertEquals(before,after,2f)
        compose.onNodeWithContentDescription("チャンネル一覧に戻る").performClick()
        compose.runOnIdle {fixtureMessages=history(51..100)}
        open()
        compose.onNodeWithText("過去 60").assertIsDisplayed()
        compose.runOnIdle {model.state.value=model.state.value.copy(messages=model.state.value.messages+message(101,"新しいメッセージ"))}
        compose.waitForIdle()
        compose.onNodeWithText("過去 60").assertIsDisplayed()
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
