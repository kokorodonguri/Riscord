package io.github.kokorodonguri.riscord

/** Convert transport failures to actionable text without exposing exception details. */
internal fun userFacingError(error: Throwable): String {
    val message=error.message.orEmpty()
    return when {
        message.startsWith("Discord HTTP 403:") -> tr("この操作を行う権限がありません。","You don't have permission to do that.")
        message.startsWith("Discord HTTP 404:") -> tr("対象が見つかりません。画面を更新して再試行してください。","Not found. Refresh the screen and try again.")
        message.startsWith("Discord HTTP 400:") -> tr("入力内容を確認して再試行してください。","Check your input and try again.")
        // コアからは日本語の決まった文言で届くので、ここで表示用に直す
        message.startsWith("混雑しています。") ->
            if(isEnglish) Regex("(\\d+)").find(message)?.value?.let {"Too busy. Try again in $it seconds."} ?: "Too busy. Please try again later."
            else message.take(100)
        message == "添付は10MBまでです" || message == "ファイルが大きすぎます（10MBまで）" -> tr(message,"Attachments can be up to 10 MB")
        message == "ユーザーIDを入力してください" -> tr(message,"Enter a user ID")
        message == "マイクの許可が必要です" -> tr(message,"Microphone permission is required")
        message == "ファイルを開けませんでした" -> tr(message,"Couldn't open the file")
        else -> tr("操作に失敗しました。通信状態や入力内容を確認して再試行してください。","Something went wrong. Check your connection and input, then try again.")
    }
}
