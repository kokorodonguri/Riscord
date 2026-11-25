package io.github.kokorodonguri.riscord

/** 接続世代と保存処理を同じロックで扱い、古い保存をログアウト後に完了させない。 */
internal class SessionTokenGate {
    private var generation = 0L

    @Synchronized fun current(): Long = generation
    @Synchronized fun next(): Long = ++generation
    @Synchronized fun <T> store(action: () -> T): T = action()
    @Synchronized fun ifCurrent(expected: Long, action: () -> Unit): Boolean {
        if (generation != expected) return false
        action()
        return true
    }
    @Synchronized fun invalidateAndClear(clear: () -> Unit) {
        generation++
        clear()
    }
}
