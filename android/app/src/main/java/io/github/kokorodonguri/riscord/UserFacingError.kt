package io.github.kokorodonguri.riscord

/** Convert transport failures to actionable text without exposing exception details. */
internal fun userFacingError(error: Throwable): String {
    val message=error.message.orEmpty()
    return when {
        message.startsWith("Discord HTTP 403:") -> tr("この操作を行う権限がありません。","You don't have permission to do that.","你没有执行此操作的权限。")
        message.startsWith("Discord HTTP 404:") -> tr("対象が見つかりません。画面を更新して再試行してください。","Not found. Refresh the screen and try again.","未找到对象。请刷新页面后重试。")
        message.startsWith("Discord HTTP 400:") -> tr("入力内容を確認して再試行してください。","Check your input and try again.","请检查输入内容后重试。")
        // コアからは日本語の決まった文言で届くので、ここで表示用に直す
        message.startsWith("混雑しています。") ->
            when(currentLang) {
                Lang.JA -> message.take(100)
                Lang.EN -> Regex("(\\d+)").find(message)?.value?.let {"Too busy. Try again in $it seconds."} ?: "Too busy. Please try again later."
                Lang.ZH -> Regex("(\\d+)").find(message)?.value?.let {"服务器繁忙。请在 $it 秒后重试。"} ?: "服务器繁忙。请稍后重试。"
            }
        message == "添付は10MBまでです" || message == "ファイルが大きすぎます（10MBまで）" -> tr(message,"Attachments can be up to 10 MB","附件最大为 10 MB")
        message == "ユーザーIDを入力してください" -> tr(message,"Enter a user ID","请输入用户 ID")
        message == "マイクの許可が必要です" -> tr(message,"Microphone permission is required","需要麦克风权限")
        message == "ファイルを開けませんでした" -> tr(message,"Couldn't open the file","无法打开文件")
        else -> tr("操作に失敗しました。通信状態や入力内容を確認して再試行してください。","Something went wrong. Check your connection and input, then try again.","操作失败。请检查网络连接和输入内容后重试。")
    }
}
