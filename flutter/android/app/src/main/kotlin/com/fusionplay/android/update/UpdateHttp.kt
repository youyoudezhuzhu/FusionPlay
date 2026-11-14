package com.fusionplay.android.update

import java.io.IOException
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL

/** 更新相关的最小 HTTP 客户端：仅 HTTPS、手动跟随重定向、限制响应体积。 */
internal object UpdateHttp {
    fun requireHttps(url: String) {
        val scheme = runCatching { URI(url).scheme }.getOrNull()
        if (!scheme.equals("https", ignoreCase = true)) throw IOException("仅支持 HTTPS 地址")
    }

    /**
     * 打开一个 HTTPS 连接并跟随最多 [UpdateConfig.MAX_REDIRECTS] 次重定向。
     * 返回的连接已处于 2xx 状态，调用方负责读取与关闭。
     */
    fun open(
        url: String,
        connectTimeoutMs: Int,
        readTimeoutMs: Int,
        accept: String,
    ): HttpURLConnection {
        var current = url
        repeat(UpdateConfig.MAX_REDIRECTS + 1) {
            requireHttps(current)
            val connection = (URL(current).openConnection() as? HttpURLConnection)
                ?: throw IOException("不支持的更新地址")
            connection.instanceFollowRedirects = false
            connection.connectTimeout = connectTimeoutMs
            connection.readTimeout = readTimeoutMs
            connection.requestMethod = "GET"
            connection.setRequestProperty("User-Agent", UpdateConfig.USER_AGENT)
            connection.setRequestProperty("Accept", accept)
            connection.setRequestProperty("Accept-Encoding", "identity")
            val code = connection.responseCode
            if (code in 300..399) {
                val location = connection.getHeaderField("Location")
                    ?: throw IOException("重定向缺少 Location")
                val next = URL(URL(current), location).toString()
                connection.disconnect()
                current = next
                return@repeat
            }
            if (code !in 200..299) {
                connection.disconnect()
                throw IOException("HTTP $code")
            }
            return connection
        }
        throw IOException("重定向次数过多")
    }

    /** 读取最多 [maxBytes] 字节；超限视为异常响应。 */
    fun readLimited(input: InputStream, maxBytes: Int): ByteArray {
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(16 * 1024)
        var total = 0
        while (true) {
            val read = input.read(buffer)
            if (read < 0) break
            total += read
            if (total > maxBytes) throw IOException("响应体积超出上限")
            output.write(buffer, 0, read)
        }
        return output.toByteArray()
    }
}
