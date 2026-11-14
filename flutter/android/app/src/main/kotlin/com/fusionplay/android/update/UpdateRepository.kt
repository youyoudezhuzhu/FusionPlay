package com.fusionplay.android.update

import android.content.Context
import java.io.IOException

/** 一次更新检查的结果。 */
sealed interface UpdateCheck {
    data class Available(val manifest: UpdateManifest) : UpdateCheck
    data class UpToDate(val manifest: UpdateManifest) : UpdateCheck
    data class Unavailable(val message: String) : UpdateCheck
    data class Failure(val message: String) : UpdateCheck
}

/**
 * 从 Pages（主）与 raw.githubusercontent（兜底）拉取更新清单，取版本号最高者。
 * 结果在进程内缓存 5 分钟，用户手动检查时强制刷新。
 */
class UpdateRepository(private val context: Context) {
    private var cachedAt = 0L
    private var cached: UpdateCheck? = null

    @Synchronized
    fun check(force: Boolean): UpdateCheck {
        val now = System.currentTimeMillis()
        if (!force) {
            cached?.let { outcome ->
                if (now - cachedAt < UpdateConfig.CHECK_CACHE_MS) return outcome
            }
        }
        val outcome = fetch()
        cached = outcome
        cachedAt = now
        return outcome
    }

    private fun fetch(): UpdateCheck {
        val candidates = listOf(
            UpdateConfig.PAGES_MANIFEST_URL + "?t=" + System.currentTimeMillis(),
            UpdateConfig.RAW_MANIFEST_URL,
        )
        val manifests = mutableListOf<UpdateManifest>()
        var lastError: String? = null
        for (url in candidates) {
            try {
                val text = fetchManifestText(url)
                val manifest = UpdateManifestParser.parse(text)
                if (manifest == null) {
                    lastError = "更新清单格式错误"
                    continue
                }
                val error = UpdateManifestParser.errorOf(manifest)
                if (error != null) {
                    lastError = error
                    continue
                }
                manifests += manifest
            } catch (error: Exception) {
                lastError = error.message ?: "无法连接更新服务"
            }
        }
        val latest = manifests.maxByOrNull { it.versionCode }
            ?: return UpdateCheck.Unavailable(lastError ?: "更新源暂不可用")
        val current = currentVersionCode()
        return if (UpdateManifestParser.isNewer(latest, current)) {
            UpdateCheck.Available(latest)
        } else {
            UpdateCheck.UpToDate(latest)
        }
    }

    private fun fetchManifestText(url: String): String {
        UpdateHttp.requireHttps(url)
        val connection = UpdateHttp.open(
            url,
            UpdateConfig.MANIFEST_TIMEOUT_MS,
            UpdateConfig.MANIFEST_TIMEOUT_MS,
            "application/json, */*",
        )
        return try {
            connection.inputStream.use { stream ->
                UpdateHttp.readLimited(stream, UpdateConfig.MAX_MANIFEST_BYTES)
                    .toString(Charsets.UTF_8)
            }
        } finally {
            connection.disconnect()
        }
    }

    fun currentVersionCode(): Long {
        val info = context.packageManager.getPackageInfo(context.packageName, 0)
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
            info.longVersionCode
        } else {
            @Suppress("DEPRECATION")
            info.versionCode.toLong()
        }
    }
}
