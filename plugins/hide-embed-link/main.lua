local S = settings({}, { ja = "埋め込みのほうにリンクがあるので、同じ URL を二重に出さないようにします。", en = "The embed already links to it, so the URL isn't shown twice.", zh = "嵌入中已有链接，因此不会重复显示同一 URL。" })
return plugin(S)
