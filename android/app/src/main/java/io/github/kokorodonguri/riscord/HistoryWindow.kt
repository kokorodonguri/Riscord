package io.github.kokorodonguri.riscord

/** A bounded, ordered window. Evict pages at the opposite end to the requested page. */
internal data class HistoryWindow<T>(val items: List<T>, val hasOlder: Boolean, val hasNewer: Boolean)

internal fun <T> mergeHistoryWindow(
    current: List<T>, fetched: List<T>, older: Boolean, newer: Boolean,
    hasOlder: Boolean, hasNewer: Boolean, arrivedDuringFetch: Boolean,
    limit: Int, pageSize: Int, id: (T) -> String,
): HistoryWindow<T> {
    fun key(item: T) = id(item).padStart(20, '0')
    val min = fetched.minOfOrNull(::key)
    val max = fetched.maxOfOrNull(::key)
    val retained = when {
        older || newer -> current
        hasNewer -> emptyList()
        else -> current.filter { min != null && max != null && (key(it) < min || key(it) > max) }
    }
    val merged = (fetched + retained).distinctBy(id).sortedBy(::key)
    return HistoryWindow(
        if (older) merged.take(limit) else merged.takeLast(limit),
        if (newer) hasOlder || merged.size > limit else fetched.size == pageSize,
        when {
            older -> hasNewer || merged.size > limit
            newer -> fetched.size == pageSize || arrivedDuringFetch
            else -> hasNewer && arrivedDuringFetch
        },
    )
}
