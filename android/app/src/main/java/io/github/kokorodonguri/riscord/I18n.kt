package io.github.kokorodonguri.riscord

import java.util.Locale

/** 表示言語。端末（またはアプリごとの言語設定）が日本語・中国語ならその言語、それ以外は英語 */
enum class Lang(val code: String) { JA("ja"), EN("en"), ZH("zh") }

val currentLang: Lang get() = when (Locale.getDefault().language) {
    "ja" -> Lang.JA
    "zh" -> Lang.ZH
    else -> Lang.EN
}

val isEnglish: Boolean get() = currentLang == Lang.EN

/** 日本語・英語・中国語（簡体字）の 3 つを並べて書き、今の言語のものを返す */
fun tr(ja: String, en: String, zh: String): String = when (currentLang) {
    Lang.JA -> ja
    Lang.EN -> en
    Lang.ZH -> zh
}

/** コアから届く接続・通話の状態（内部では日本語の決まった値）を表示用にする */
fun statusLabel(status: String): String = when (status) {
    "未接続" -> tr(status, "Disconnected", "未连接")
    "接続中" -> tr(status, "Connecting", "正在连接")
    "接続済み" -> tr(status, "Connected", "已连接")
    "再接続待ち" -> tr(status, "Waiting to reconnect", "等待重新连接")
    "通話中" -> tr(status, "In call", "通话中")
    "一時停止中" -> tr(status, "Paused", "已暂停")
    else -> status
}
