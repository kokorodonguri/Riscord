package io.github.kokorodonguri.riscord

import android.app.*
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.IBinder

class CallService : Service() {
    private lateinit var audio:AudioManager
    private var oldMode=AudioManager.MODE_NORMAL
    private lateinit var focus:AudioFocusRequest
    override fun onCreate() {
        super.onCreate()
        val notifications=getSystemService(NotificationManager::class.java)
        notifications.createNotificationChannel(NotificationChannel("calls",tr("通話","Calls"),NotificationManager.IMPORTANCE_LOW))
        val open=PendingIntent.getActivity(this,0,Intent(this,MainActivity::class.java),PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val end=PendingIntent.getService(this,2,Intent(this,CallService::class.java).setAction("end"),PendingIntent.FLAG_IMMUTABLE)
        startForeground(20,Notification.Builder(this,"calls").setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(tr("Riscord 通話中","Riscord call in progress")).setContentText(tr("タップして通話に戻る","Tap to return to the call")).setContentIntent(open).setOngoing(true)
            .addAction(Notification.Action.Builder(null,tr("切断","Disconnect"),end).build()).build())
        audio=getSystemService(AudioManager::class.java)
        oldMode=audio.mode
        audio.mode=AudioManager.MODE_IN_COMMUNICATION
        focus=AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setOnAudioFocusChangeListener { change ->
                if(change==AudioManager.AUDIOFOCUS_LOSS) (application as RiscordApplication).chatModel.leaveVoice()
            }.build()
        if(audio.requestAudioFocus(focus)==AudioManager.AUDIOFOCUS_REQUEST_FAILED) {
            (application as RiscordApplication).chatModel.leaveVoice()
            stopSelf()
        }
    }
    override fun onStartCommand(intent:Intent?,flags:Int,startId:Int):Int {
        if(intent?.action=="end") {(application as RiscordApplication).chatModel.leaveVoice();stopSelf()}
        return START_NOT_STICKY
    }
    override fun onDestroy() {
        if(::audio.isInitialized) {if(::focus.isInitialized) audio.abandonAudioFocusRequest(focus); if(android.os.Build.VERSION.SDK_INT>=31) audio.clearCommunicationDevice(); audio.mode=oldMode}
        super.onDestroy()
    }
    override fun onBind(intent:Intent?):IBinder?=null
}
