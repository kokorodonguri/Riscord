package io.github.kokorodonguri.riscord

import android.app.Application

class RiscordApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        NativeBridge.initialize(this)
        getSystemService(android.app.NotificationManager::class.java).createNotificationChannels(listOf(
            android.app.NotificationChannel("messages","メッセージ",android.app.NotificationManager.IMPORTANCE_DEFAULT),
            android.app.NotificationChannel("incoming_calls","着信",android.app.NotificationManager.IMPORTANCE_HIGH),
        ))
    }
    // One session owner for the activity and the optional foreground service.
    val chatModel by lazy { ChatModel(this) }
}
