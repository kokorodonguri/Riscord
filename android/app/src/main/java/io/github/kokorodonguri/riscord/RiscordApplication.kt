package io.github.kokorodonguri.riscord

import android.app.Application

class RiscordApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        NativeBridge.initialize(this)
        NativeBridge.setEnglish(isEnglish)
        getSystemService(android.app.NotificationManager::class.java).createNotificationChannels(listOf(
            android.app.NotificationChannel("messages",tr("メッセージ","Messages"),android.app.NotificationManager.IMPORTANCE_DEFAULT),
            android.app.NotificationChannel("incoming_calls",tr("着信","Incoming call"),android.app.NotificationManager.IMPORTANCE_HIGH),
        ))
    }
    // One session owner for the activity and the optional foreground service.
    val chatModel by lazy { ChatModel(this) }
}
