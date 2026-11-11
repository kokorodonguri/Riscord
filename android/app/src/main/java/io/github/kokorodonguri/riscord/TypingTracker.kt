package io.github.kokorodonguri.riscord

/** Monotonic deadlines: wall-clock corrections must not extend a typing indicator. */
internal class TypingTracker(private val now:()->Long={System.nanoTime()/1_000_000}) {
    private data class Person(val name:String,val until:Long)
    private val people=linkedMapOf<String,Person>()
    private val sent=mutableMapOf<String,Long>()
    fun start(user:String,name:String) {people[user]=Person(name,now()+10_000)}
    fun remove(user:String) {people.remove(user)}
    fun clear() {people.clear()}
    fun reset() {clear();sent.clear()}
    fun label():String? {
        expire()
        val names=people.values.map {it.name}
        return when(names.size) {0->null;1->"${names[0]} が入力中…";2->"${names.joinToString("、")} が入力中…";else->"${names.size}人が入力中…"}
    }
    fun nextExpiry():Long? {expire();return people.values.minOfOrNull {it.until-now()}?.coerceAtLeast(1)}
    private fun expire() {val time=now();people.entries.removeAll {it.value.until<=time}}
    fun reserveSend(channel:String):Boolean {
        val time=now();val previous=sent[channel]
        if(previous!=null&&time-previous<8_000) return false
        sent[channel]=time
        return true
    }
}
