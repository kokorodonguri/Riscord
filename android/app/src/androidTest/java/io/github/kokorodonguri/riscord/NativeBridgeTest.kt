package io.github.kokorodonguri.riscord

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.json.JSONObject

@RunWith(AndroidJUnit4::class)
class NativeBridgeTest {
    // コアのエラー文は日本語で確かめる
    @org.junit.Before fun japanese() = NativeBridge.setLanguage("ja")
    @Test fun nativeSessionLifecycleAndUnicodeErrors() {
        val id=NativeBridge.create()
        assertTrue(id>0)
        val poll=JSONObject(NativeBridge.call(id,"{\"op\":\"poll\"}"))
        assertTrue(poll.getBoolean("ok"))
        assertEquals(0,poll.getJSONArray("data").length())
        val bad=JSONObject(NativeBridge.call(id,"{\"op\":\"不明な操作😀\"}"))
        assertFalse(bad.getBoolean("ok"))
        assertEquals("不明な操作",bad.getString("error"))
        NativeBridge.destroy(id)
        assertFalse(JSONObject(NativeBridge.call(id,"{\"op\":\"poll\"}")).getBoolean("ok"))
    }
    @Test fun androidAudioBackendFindsOutputDevices() {
        val id=NativeBridge.create()
        try {
            val result=JSONObject(NativeBridge.call(id,"{\"op\":\"audio_devices\"}"))
            assertTrue(result.toString(),result.getBoolean("ok"))
            assertTrue(result.getJSONObject("data").getJSONArray("output").length()>0)
        } finally {NativeBridge.destroy(id)}
    }
    @Test fun uploadFailureLeavesTheCallersDescriptorOpen() {
        android.os.ParcelFileDescriptor.open(java.io.File("/dev/null"),android.os.ParcelFileDescriptor.MODE_READ_ONLY).use { descriptor ->
            val request=JSONObject().put("op","upload").put("fd",descriptor.fd).put("channel","1").put("filename","x").put("content","")
            val result=JSONObject(NativeBridge.call(-1,request.toString()))
            assertFalse(result.getBoolean("ok"))
            android.system.Os.fstat(descriptor.fileDescriptor)
        }
    }
    @Test fun qrLoginCreatesScannableModules() {
        val id=NativeBridge.create()
        try {
            assertTrue(JSONObject(NativeBridge.call(id,"{\"op\":\"login\",\"mode\":\"qr\"}")).getBoolean("ok"))
            val deadline=System.currentTimeMillis()+30000
            while(System.currentTimeMillis()<deadline) {
                val events=JSONObject(NativeBridge.call(id,"{\"op\":\"poll\"}")).getJSONArray("data")
                for(i in 0 until events.length()) {
                    val event=events.getJSONObject(i)
                    assertNotEquals(event.toString(),"ERROR",event.getString("t"))
                    if(event.getString("t")=="LOGIN_QR") {
                        val modules=event.getJSONObject("d").getJSONObject("modules")
                        val size=modules.getInt("size")
                        assertTrue(size>20)
                        assertEquals(size*size,modules.getJSONArray("bits").length())
                        return
                    }
                }
                Thread.sleep(100)
            }
            fail("QRログインがタイムアウトしました")
        } finally { NativeBridge.destroy(id) }
    }
    @Test fun rustTlsCanReachDiscordAndRejectInvalidToken() {
        val id=NativeBridge.create()
        try {
            val result=JSONObject(NativeBridge.call(id,"{\"op\":\"connect\",\"token\":\"riscord-invalid-test-token\"}"))
            assertFalse(result.getBoolean("ok"))
            assertTrue(result.getString("error"),result.getString("error").contains("ログイン"))
        } finally { NativeBridge.destroy(id) }
    }
}
