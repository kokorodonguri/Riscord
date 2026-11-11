package io.github.kokorodonguri.riscord

import android.app.*
import android.content.Intent
import android.os.IBinder

class ConnectionService : Service() {
    override fun onCreate() {
        super.onCreate()
        val manager=getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("connection","接続状態",NotificationManager.IMPORTANCE_LOW))
        val open=PendingIntent.getActivity(this,0,Intent(this,MainActivity::class.java),PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop=PendingIntent.getService(this,1,Intent(this,ConnectionService::class.java).setAction("stop"),PendingIntent.FLAG_IMMUTABLE)
        val notification=Notification.Builder(this,"connection").setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Riscord 接続を維持しています").setContentText("タップして開く").setContentIntent(open)
            .setOngoing(true).addAction(Notification.Action.Builder(null,"常駐を停止",stop).build()).build()
        startForeground(10,notification)
        (application as RiscordApplication).chatModel
    }
    override fun onStartCommand(intent: Intent?,flags: Int,startId:Int):Int {
        if(intent?.action=="stop") {
            (application as RiscordApplication).chatModel.setPersistent(false)
            stopSelf()
        }
        return START_NOT_STICKY
    }
    override fun onBind(intent:Intent?):IBinder?=null
}
