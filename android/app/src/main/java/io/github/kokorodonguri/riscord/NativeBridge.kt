package io.github.kokorodonguri.riscord

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.json.JSONObject

object NativeBridge {
    init { System.loadLibrary("riscord_core") }
    external fun initialize(context: android.content.Context)
    external fun setEnglish(english: Boolean)
    external fun create(): Long
    external fun destroy(id: Long)
    external fun call(id: Long, input: String): String
    suspend fun request(id: Long, input: JSONObject): Any? = withContext(Dispatchers.IO) {
        val result = JSONObject(call(id, input.toString()))
        if (!result.getBoolean("ok")) {
            val message = result.optString("error", tr("通信に失敗しました","Network request failed"))
            throw if (result.optBoolean("auth")) AuthExpiredException(message) else IllegalStateException(message)
        }
        result.opt("data")
    }
}

/** トークンが無効になった（保存したログインを消してログインし直す） */
class AuthExpiredException(message: String) : IllegalStateException(message)
