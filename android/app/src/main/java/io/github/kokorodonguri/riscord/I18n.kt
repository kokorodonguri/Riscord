package io.github.kokorodonguri.riscord

import java.util.Locale

/** 端末（またはアプリごとの言語設定）が日本語なら日本語、それ以外は英語で表示する */
val isEnglish: Boolean get() = Locale.getDefault().language != "ja"

fun tr(ja: String, en: String): String = if (isEnglish) en else ja

/** コアから届く接続・通話の状態（内部では日本語の決まった値）を表示用にする */
fun statusLabel(status: String): String = if (!isEnglish) status else when (status) {
    "未接続" -> "Disconnected"
    "接続中" -> "Connecting"
    "接続済み" -> "Connected"
    "再接続待ち" -> "Waiting to reconnect"
    "通話中" -> "In call"
    "一時停止中" -> "Paused"
    else -> status
}
