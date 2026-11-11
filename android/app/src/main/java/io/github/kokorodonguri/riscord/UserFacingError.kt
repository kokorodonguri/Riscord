package io.github.kokorodonguri.riscord

/** Convert transport failures to actionable text without exposing exception details. */
internal fun userFacingError(error: Throwable): String {
    val message=error.message.orEmpty()
    return when {
        message.startsWith("Discord HTTP 403:") -> "この操作を行う権限がありません。"
        message.startsWith("Discord HTTP 404:") -> "対象が見つかりません。画面を更新して再試行してください。"
        message.startsWith("Discord HTTP 400:") -> "入力内容を確認して再試行してください。"
        message.startsWith("混雑しています。") -> message.take(100)
        message == "添付は10MBまでです" || message == "ファイルが大きすぎます（10MBまで）" ||
        message == "ユーザーIDを入力してください" || message == "マイクの許可が必要です" ||
        message == "ファイルを開けませんでした" -> message
        else -> "操作に失敗しました。通信状態や入力内容を確認して再試行してください。"
    }
}
