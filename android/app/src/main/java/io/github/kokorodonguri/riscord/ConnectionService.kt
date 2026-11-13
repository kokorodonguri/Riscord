package io.github.kokorodonguri.riscord

import android.app.*
import android.content.Intent
import android.os.IBinder

class ConnectionService : Service() {
    override fun onCreate() {
        super.onCreate()
        val manager=getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel("connection",tr("接続状態","Connection status","连接状态"),NotificationManager.IMPORTANCE_LOW))
        val open=PendingIntent.getActivity(this,0,Intent(this,MainActivity::class.java),PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val stop=PendingIntent.getService(this,1,Intent(this,ConnectionService::class.java).setAction("stop"),PendingIntent.FLAG_IMMUTABLE)
        val notification=Notification.Builder(this,"connection").setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(tr("Riscord 接続を維持しています","Riscord is staying connected","Riscord 正在保持连接")).setContentText(tr("タップして開く","Tap to open","点击打开")).setContentIntent(open)
            .setOngoing(true).addAction(Notification.Action.Builder(null,tr("常駐を停止","Stop","停止"),stop).build()).build()
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
